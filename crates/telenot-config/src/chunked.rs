//! Chunked, crash-safe binary config storage on top of an NVS-like blob store.
//!
//! # Format
//!
//! The config is persisted as **postcard**-encoded blobs in two alternating
//! chunk sets **A** and **B** plus one metadata blob:
//!
//! - `cfg_a0`, `cfg_a1`, … / `cfg_b0`, `cfg_b1`, … — sensor chunks. Each chunk
//!   holds up to [`CHUNK_SENSORS`] sensors as a postcard `Vec<Sensor>`; a chunk
//!   never exceeds [`CHUNK_BUF`] bytes.
//! - [`META_KEY`] — postcard-encoded [`Meta`]: format/schema version,
//!   generation counter, which set (A or B) is active, sensor/chunk counts, a
//!   CRC32 per chunk and the [`PanelSettings`].
//!
//! # Crash safety
//!
//! [`save`] always writes to the **inactive** set and switches over only in the
//! very last step, by writing the new meta blob. NVS blob writes are atomic per
//! key, so a crash/power loss at ANY point before the meta write leaves the old
//! meta — and with it the complete old chunk set — untouched: the next [`load`]
//! returns the previous config. A torn save merely leaves stale chunks in the
//! inactive set, which the next save clears before writing.
//!
//! Integrity of the active set is verified on load via the per-chunk CRC32
//! stored in the meta blob (CRC32/IEEE, table-less — chunks are ≤ ~4 KB, so
//! bitwise speed is irrelevant).
//!
//! # Why A/B instead of in-place?
//!
//! The `cfg` NVS partition has a limited rewrite budget: while a blob is
//! rewritten, old and new value coexist in flash. With a single chunk set, a
//! crash mid-rewrite could corrupt the only copy. With A/B, old and new set
//! coexist only until the next save (which reclaims the then-inactive set), and
//! the active set is never modified — the worst case is losing the *save*, never
//! the *config*.
//!
//! This module is pure host-testable logic: firmware plugs in its `EspNvs`
//! adapter via [`BlobStore`] (in a later commit); tests use a `HashMap` mock.

use serde::{Deserialize, Serialize};

use crate::{Config, PanelSettings, Sensor, SensorTable, CURRENT_SCHEMA_VERSION};

/// Sensors per chunk. 32 × ~130 B postcard ≈ 4 KB — comfortably below
/// [`CHUNK_BUF`] and small enough that a chunk rewrite fits any NVS page budget.
pub const CHUNK_SENSORS: usize = 32;
/// Key of the metadata blob.
pub const META_KEY: &str = "cfg_meta";
/// Read/size budget for one chunk blob.
pub const CHUNK_BUF: usize = 8 * 1024;
/// Read/size budget for the metadata blob.
pub const META_BUF: usize = 2 * 1024;
/// Version of the *storage* format (independent of the config schema version).
pub const FORMAT_VERSION: u16 = 1;

/// Abstraction over NVS-like blob stores (firmware: `EspNvs`; tests: `HashMap` mock).
pub trait BlobStore {
    /// Blob length, `None` = key missing.
    fn blob_len(&self, key: &str) -> Result<Option<usize>, StoreError>;
    /// Reads into `buf` (must be large enough), returns the length read; `None` = key missing.
    fn get_blob(&self, key: &str, buf: &mut [u8]) -> Result<Option<usize>, StoreError>;
    fn set_blob(&mut self, key: &str, val: &[u8]) -> Result<(), StoreError>;
    /// Remove a key; a missing key is OK (idempotent).
    fn remove(&mut self, key: &str) -> Result<(), StoreError>;
}

/// Storage error (store failure, corruption, budget violation).
#[derive(Debug)]
pub struct StoreError(pub String);

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Storage-Fehler: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

/// Metadata blob: describes which chunk set is active and how to verify it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// Storage format version ([`FORMAT_VERSION`]).
    pub format_version: u16,
    /// Monotonic save counter (diagnostics; strictly increasing across saves).
    pub generation: u32,
    /// `true` = set B (`cfg_b*`) is active, `false` = set A (`cfg_a*`).
    pub active_b: bool,
    /// Total number of sensors across all chunks.
    pub sensor_count: u32,
    /// Number of chunks in the active set.
    pub chunk_count: u32,
    /// CRC32 (IEEE) per chunk, indexed by chunk number.
    pub chunk_crc: Vec<u32>,
    /// Config schema version ([`crate::CURRENT_SCHEMA_VERSION`] at save time).
    pub schema_version: u32,
    /// Panel settings live in the meta blob (small, no chunking needed).
    pub panel: PanelSettings,
}

/// Key of chunk `idx` in set A (`set_b = false`) or B (`set_b = true`).
pub fn chunk_key(set_b: bool, idx: usize) -> String {
    format!("cfg_{}{idx}", if set_b { 'b' } else { 'a' })
}

/// CRC32/IEEE (reflected, init `0xFFFF_FFFF`, xorout `0xFFFF_FFFF`) — table-less;
/// performance is irrelevant for ≤ ~4 KB chunks.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Reads and decodes the metadata blob. `Ok(None)` = no meta present;
/// `Err` = store failure, oversized blob or undecodable bytes.
pub fn load_meta(store: &impl BlobStore) -> Result<Option<Meta>, StoreError> {
    let Some(len) = store.blob_len(META_KEY)? else {
        return Ok(None);
    };
    if len > META_BUF {
        return Err(StoreError(format!(
            "Meta-Blob {len} B überschreitet Budget {META_BUF} B"
        )));
    }
    let mut buf = vec![0u8; META_BUF];
    let Some(read) = store.get_blob(META_KEY, &mut buf)? else {
        return Ok(None);
    };
    let meta: Meta = postcard::from_bytes(&buf[..read])
        .map_err(|e| StoreError(format!("Meta nicht dekodierbar: {e}")))?;
    Ok(Some(meta))
}

/// Persists `cfg` crash-safely: chunks go to the currently **inactive** set, the
/// meta blob (written last) flips it active. Any error/crash before the meta
/// write leaves the previously active set fully intact.
pub fn save(store: &mut impl BlobStore, cfg: &Config) -> Result<(), StoreError> {
    // Missing OR broken meta = no active set → start fresh at A, generation 1.
    let old = load_meta(store).ok().flatten();
    let (target_b, generation) = match &old {
        Some(m) => (!m.active_b, m.generation.wrapping_add(1)),
        None => (false, 1),
    };

    // Clear ALL stale chunks of the target set: ascending until the first
    // missing key — deliberately past the new chunk_count, so shrinking never
    // leaves orphaned chunks behind (the load loop trusts chunk_count, but
    // orphans would silently resurrect on a format bug and waste NVS space).
    let mut i = 0usize;
    while store.blob_len(&chunk_key(target_b, i))?.is_some() {
        store.remove(&chunk_key(target_b, i))?;
        i += 1;
    }

    let sensor_count = cfg.sensors.len();
    let chunk_count = sensor_count.div_ceil(CHUNK_SENSORS);
    let mut chunk_crc = Vec::with_capacity(chunk_count);
    for ci in 0..chunk_count {
        // Transient owned chunk (≤ CHUNK_SENSORS sensors) — bounded peak memory.
        let batch: Vec<Sensor> = (ci * CHUNK_SENSORS..((ci + 1) * CHUNK_SENSORS).min(sensor_count))
            .map(|si| cfg.sensors.get(si).expect("index < len").to_sensor())
            .collect();
        let bytes = postcard::to_allocvec(&batch)
            .map_err(|e| StoreError(format!("Chunk {ci} nicht serialisierbar: {e}")))?;
        if bytes.len() > CHUNK_BUF {
            return Err(StoreError(format!(
                "Chunk {ci} mit {} B überschreitet Budget {CHUNK_BUF} B",
                bytes.len()
            )));
        }
        chunk_crc.push(crc32(&bytes));
        store.set_blob(&chunk_key(target_b, ci), &bytes)?;
    }

    let meta = Meta {
        format_version: FORMAT_VERSION,
        generation,
        active_b: target_b,
        sensor_count: sensor_count as u32,
        chunk_count: chunk_count as u32,
        chunk_crc,
        schema_version: cfg.schema_version,
        panel: cfg.panel.clone(),
    };
    let meta_bytes = postcard::to_allocvec(&meta)
        .map_err(|e| StoreError(format!("Meta nicht serialisierbar: {e}")))?;
    if meta_bytes.len() > META_BUF {
        return Err(StoreError(format!(
            "Meta mit {} B überschreitet Budget {META_BUF} B",
            meta_bytes.len()
        )));
    }
    // The commit point: only THIS write activates the new set.
    store.set_blob(META_KEY, &meta_bytes)
}

/// Loads the active config. `Ok(None)` = nothing stored yet. `Err` on store
/// failure, unknown format, newer schema, CRC mismatch or count mismatch —
/// never a partial result.
pub fn load(store: &impl BlobStore) -> Result<Option<Config>, StoreError> {
    let Some(meta) = load_meta(store)? else {
        return Ok(None);
    };
    if meta.format_version != FORMAT_VERSION {
        return Err(StoreError(format!(
            "Storage-Format {} unbekannt (erwartet {FORMAT_VERSION})",
            meta.format_version
        )));
    }
    if meta.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(StoreError(format!(
            "Schema-Version {} neuer als Firmware ({CURRENT_SCHEMA_VERSION})",
            meta.schema_version
        )));
    }
    if meta.chunk_crc.len() != meta.chunk_count as usize {
        return Err(StoreError(format!(
            "Meta inkonsistent: {} CRCs für {} Chunks",
            meta.chunk_crc.len(),
            meta.chunk_count
        )));
    }

    let mut sensors = SensorTable::new();
    let mut buf = vec![0u8; CHUNK_BUF];
    for ci in 0..meta.chunk_count as usize {
        let key = chunk_key(meta.active_b, ci);
        let Some(len) = store.blob_len(&key)? else {
            return Err(StoreError(format!("Chunk {ci} ({key}) fehlt")));
        };
        if len > CHUNK_BUF {
            return Err(StoreError(format!(
                "Chunk {ci} mit {len} B überschreitet Budget {CHUNK_BUF} B"
            )));
        }
        let Some(read) = store.get_blob(&key, &mut buf)? else {
            return Err(StoreError(format!("Chunk {ci} ({key}) fehlt")));
        };
        if crc32(&buf[..read]) != meta.chunk_crc[ci] {
            return Err(StoreError(format!("CRC-Fehler in Chunk {ci} ({key})")));
        }
        let batch: Vec<Sensor> = postcard::from_bytes(&buf[..read])
            .map_err(|e| StoreError(format!("Chunk {ci} nicht dekodierbar: {e}")))?;
        for s in &batch {
            sensors
                .push(s)
                .map_err(|e| StoreError(format!("Sensor-Tabelle: {e}")))?;
        }
    }

    if sensors.len() != meta.sensor_count as usize {
        return Err(StoreError(format!(
            "Sensorzahl {} passt nicht zur Meta ({})",
            sensors.len(),
            meta.sensor_count
        )));
    }

    Ok(Some(Config {
        schema_version: meta.schema_version,
        sensors,
        panel: meta.panel,
    }))
}

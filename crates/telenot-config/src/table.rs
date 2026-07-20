//! Compact sensor storage: fixed-size records + a blocked string arena.
//!
//! The naive `Vec<Sensor>` costs ~450–930 B heap per sensor (four individually
//! allocated `String`s) and, worse, exists in 3–4 simultaneous copies during a
//! commit — which is what capped the device at 200 sensors. `SensorTable` stores
//! the fixed fields in a 32-byte record and all strings in 8-KB arena blocks:
//! ~100 B per sensor in ONE allocation pattern the fragmented ESP32 heap can
//! always satisfy, and `try_reserve` turns "does not fit" into
//! [`ConfigError::TooLarge`] instead of an allocation abort.
//!
//! The JSON representation is **byte-identical** to the previous
//! `Vec<Sensor>` derive (same field order, same escaping) — pinned by tests.

use serde::de::{SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ConfigError, Polarity, Sensor, SensorKind};

/// Arena block size. Strings never cross a block boundary; a single field longer
/// than this is rejected as [`ConfigError::TooLarge`] (a multi-KB sensor name is
/// not a use case, and the bound keeps offset arithmetic trivial).
const BLOCK: usize = 8 * 1024;

const F_CONFIRMED: u8 = 1;
const F_SWITCHABLE: u8 = 1 << 1;
const F_SHOW_IN_HOMEKIT: u8 = 1 << 2;

/// String field slots per record (name, name_ha, topic).
const N_STR: usize = 3;
const S_NAME: usize = 0;
const S_NAME_HA: usize = 1;
const S_TOPIC: usize = 2;

/// Fixed part of one sensor: 29 payload bytes, padded to 32.
#[derive(Clone, Copy)]
struct SensorRec {
    offs: [u32; N_STR],
    lens: [u16; N_STR],
    address: u16,
    kind: SensorKind,
    polarity: Polarity,
    flags: u8,
}

/// Blocked string arena. Appending only; overwritten spans become waste that
/// [`SensorTable::compact`] reclaims.
#[derive(Default)]
struct StrArena {
    blocks: Vec<Box<[u8]>>,
    /// Fill level of the last block.
    used: usize,
    /// Bytes orphaned by setters (candidates for `compact`).
    waste: usize,
}

impl StrArena {
    fn alloc(&mut self, s: &str) -> Result<(u32, u16), ConfigError> {
        if s.len() > BLOCK || s.len() > u16::MAX as usize {
            return Err(ConfigError::TooLarge);
        }
        if self.blocks.is_empty() || self.used + s.len() > BLOCK {
            let mut block = Vec::new();
            block
                .try_reserve_exact(BLOCK)
                .map_err(|_| ConfigError::TooLarge)?;
            block.resize(BLOCK, 0);
            self.blocks.push(block.into_boxed_slice());
            self.used = 0;
        }
        let block_idx = self.blocks.len() - 1;
        let off = block_idx * BLOCK + self.used;
        self.blocks[block_idx][self.used..self.used + s.len()].copy_from_slice(s.as_bytes());
        self.used += s.len();
        Ok((off as u32, s.len() as u16))
    }

    fn get(&self, off: u32, len: u16) -> &str {
        let (off, len) = (off as usize, len as usize);
        let bytes = &self.blocks[off / BLOCK][off % BLOCK..off % BLOCK + len];
        // Only `alloc` writes spans, and it only ever copies from `&str`.
        core::str::from_utf8(bytes).expect("arena spans are written from &str")
    }

    fn bytes(&self) -> usize {
        self.blocks.len() * BLOCK
    }
}

/// Compact sensor inventory. Replaces `Vec<Sensor>` inside [`crate::Config`];
/// serializes byte-identically to it.
#[derive(Default)]
pub struct SensorTable {
    recs: Vec<SensorRec>,
    arena: StrArena,
}

/// Borrowed view of one sensor (record + arena).
#[derive(Clone, Copy)]
pub struct SensorRef<'a> {
    rec: &'a SensorRec,
    arena: &'a StrArena,
}

impl<'a> SensorRef<'a> {
    pub fn address(&self) -> u16 {
        self.rec.address
    }
    pub fn kind(&self) -> SensorKind {
        self.rec.kind
    }
    pub fn polarity(&self) -> Polarity {
        self.rec.polarity
    }
    pub fn confirmed(&self) -> bool {
        self.rec.flags & F_CONFIRMED != 0
    }
    pub fn switchable(&self) -> bool {
        self.rec.flags & F_SWITCHABLE != 0
    }
    pub fn show_in_homekit(&self) -> bool {
        self.rec.flags & F_SHOW_IN_HOMEKIT != 0
    }
    pub fn name(&self) -> &'a str {
        self.str_field(S_NAME)
    }
    pub fn name_ha(&self) -> &'a str {
        self.str_field(S_NAME_HA)
    }
    pub fn topic(&self) -> &'a str {
        self.str_field(S_TOPIC)
    }
    fn str_field(&self, slot: usize) -> &'a str {
        self.arena.get(self.rec.offs[slot], self.rec.lens[slot])
    }

    /// Materializes an owned [`Sensor`] (rare paths: single-sensor DTOs, tests).
    pub fn to_sensor(&self) -> Sensor {
        Sensor {
            address: self.address(),
            name: self.name().into(),
            name_ha: self.name_ha().into(),
            kind: self.kind(),
            topic: self.topic().into(),
            polarity: self.polarity(),
            confirmed: self.confirmed(),
            switchable: self.switchable(),
            show_in_homekit: self.show_in_homekit(),
        }
    }
}

/// Mutable access to one sensor. String setters append to the arena (the old
/// span becomes waste — run [`SensorTable::compact`] after bulk edits).
pub struct SensorMut<'a> {
    table: &'a mut SensorTable,
    idx: usize,
}

impl SensorMut<'_> {
    pub fn as_ref(&self) -> SensorRef<'_> {
        SensorRef {
            rec: &self.table.recs[self.idx],
            arena: &self.table.arena,
        }
    }
    pub fn set_name(&mut self, v: &str) -> Result<(), ConfigError> {
        self.set_str(S_NAME, v)
    }
    pub fn set_name_ha(&mut self, v: &str) -> Result<(), ConfigError> {
        self.set_str(S_NAME_HA, v)
    }
    pub fn set_topic(&mut self, v: &str) -> Result<(), ConfigError> {
        self.set_str(S_TOPIC, v)
    }
    pub fn set_kind(&mut self, v: SensorKind) {
        self.table.recs[self.idx].kind = v;
    }
    pub fn set_polarity(&mut self, v: Polarity) {
        self.table.recs[self.idx].polarity = v;
    }
    pub fn set_confirmed(&mut self, v: bool) {
        self.set_flag(F_CONFIRMED, v)
    }
    pub fn set_switchable(&mut self, v: bool) {
        self.set_flag(F_SWITCHABLE, v)
    }
    pub fn set_show_in_homekit(&mut self, v: bool) {
        self.set_flag(F_SHOW_IN_HOMEKIT, v)
    }

    fn set_flag(&mut self, bit: u8, v: bool) {
        let f = &mut self.table.recs[self.idx].flags;
        if v {
            *f |= bit;
        } else {
            *f &= !bit;
        }
    }
    fn set_str(&mut self, slot: usize, v: &str) -> Result<(), ConfigError> {
        let rec = &self.table.recs[self.idx];
        if self.table.arena.get(rec.offs[slot], rec.lens[slot]) == v {
            return Ok(());
        }
        let old_len = rec.lens[slot] as usize;
        let (off, len) = self.table.arena.alloc(v)?;
        let rec = &mut self.table.recs[self.idx];
        rec.offs[slot] = off;
        rec.lens[slot] = len;
        self.table.arena.waste += old_len;
        Ok(())
    }
}

impl SensorTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.recs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    /// Total arena bytes currently allocated (capacity, incl. waste + padding).
    pub fn arena_bytes(&self) -> usize {
        self.arena.bytes()
    }

    pub fn push(&mut self, s: &Sensor) -> Result<(), ConfigError> {
        self.recs
            .try_reserve(1)
            .map_err(|_| ConfigError::TooLarge)?;
        let mut offs = [0u32; N_STR];
        let mut lens = [0u16; N_STR];
        for (slot, v) in [
            (S_NAME, s.name.as_str()),
            (S_NAME_HA, s.name_ha.as_str()),
            (S_TOPIC, s.topic.as_str()),
        ] {
            let (off, len) = self.arena.alloc(v)?;
            offs[slot] = off;
            lens[slot] = len;
        }
        let mut flags = 0u8;
        if s.confirmed {
            flags |= F_CONFIRMED;
        }
        if s.switchable {
            flags |= F_SWITCHABLE;
        }
        if s.show_in_homekit {
            flags |= F_SHOW_IN_HOMEKIT;
        }
        self.recs.push(SensorRec {
            offs,
            lens,
            address: s.address,
            kind: s.kind,
            polarity: s.polarity,
            flags,
        });
        Ok(())
    }

    pub fn get(&self, idx: usize) -> Option<SensorRef<'_>> {
        self.recs.get(idx).map(|rec| SensorRef {
            rec,
            arena: &self.arena,
        })
    }

    pub fn get_mut(&mut self, idx: usize) -> Option<SensorMut<'_>> {
        (idx < self.recs.len()).then_some(SensorMut { table: self, idx })
    }

    /// First sensor with this address (addresses may repeat: OR-linked contacts).
    pub fn find(&self, address: u16) -> Option<SensorRef<'_>> {
        self.iter().find(|s| s.address() == address)
    }

    pub fn find_mut(&mut self, address: u16) -> Option<SensorMut<'_>> {
        let idx = self.recs.iter().position(|r| r.address == address)?;
        Some(SensorMut { table: self, idx })
    }

    pub fn iter(&self) -> impl Iterator<Item = SensorRef<'_>> {
        self.recs.iter().map(|rec| SensorRef {
            rec,
            arena: &self.arena,
        })
    }

    /// Keeps only sensors matching the predicate. Orphaned strings become waste —
    /// call [`Self::compact`] afterwards (or use both via a bulk edit path).
    pub fn retain(&mut self, mut keep: impl FnMut(SensorRef<'_>) -> bool) {
        let arena = &self.arena;
        let mut dropped = 0usize;
        self.recs.retain(|rec| {
            let k = keep(SensorRef { rec, arena });
            if !k {
                dropped += rec.lens.iter().map(|&l| l as usize).sum::<usize>();
            }
            k
        });
        self.arena.waste += dropped;
    }

    /// Rebuilds the arena without waste. Peak: old + new arena blocks (records
    /// are patched in place).
    pub fn compact(&mut self) -> Result<(), ConfigError> {
        if self.arena.waste == 0 {
            return Ok(());
        }
        let mut fresh = StrArena::default();
        for rec in &mut self.recs {
            for slot in 0..N_STR {
                let s = self.arena.get(rec.offs[slot], rec.lens[slot]);
                let (off, len) = fresh.alloc(s)?;
                rec.offs[slot] = off;
                rec.lens[slot] = len;
            }
        }
        self.arena = fresh;
        Ok(())
    }

    /// Bulk construction from owned sensors.
    pub fn from_sensors<'a>(
        sensors: impl IntoIterator<Item = &'a Sensor>,
    ) -> Result<Self, ConfigError> {
        let mut t = Self::new();
        for s in sensors {
            t.push(s)?;
        }
        Ok(t)
    }
}

impl Clone for SensorTable {
    /// Clone compacts implicitly (rebuilds via owned sensors of each record).
    fn clone(&self) -> Self {
        let mut t = SensorTable::new();
        for s in self.iter() {
            // Capacity was satisfiable for `self`; a clone of the same data
            // failing is an OOM-level condition where panicking is acceptable.
            t.push(&s.to_sensor()).expect("clone of existing table");
        }
        t
    }
}

impl core::fmt::Debug for SensorTable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SensorTable")
            .field("len", &self.len())
            .field("arena_bytes", &self.arena.bytes())
            .finish()
    }
}

impl PartialEq for SensorTable {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self.iter().zip(other.iter()).all(|(a, b)| {
                a.address() == b.address()
                    && a.kind() == b.kind()
                    && a.polarity() == b.polarity()
                    && a.confirmed() == b.confirmed()
                    && a.switchable() == b.switchable()
                    && a.show_in_homekit() == b.show_in_homekit()
                    && a.name() == b.name()
                    && a.name_ha() == b.name_ha()
                    && a.topic() == b.topic()
            })
    }
}
impl Eq for SensorTable {}

/// Borrowed serialization mirror of [`Sensor`] — field names and order MUST
/// match the `Sensor` derive exactly (pinned by the byte-identity test).
#[derive(Serialize)]
struct SensorSer<'a> {
    address: u16,
    name: &'a str,
    name_ha: &'a str,
    kind: SensorKind,
    topic: &'a str,
    polarity: Polarity,
    confirmed: bool,
    switchable: bool,
    show_in_homekit: bool,
}

impl Serialize for SensorTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for s in self.iter() {
            seq.serialize_element(&SensorSer {
                address: s.address(),
                name: s.name(),
                name_ha: s.name_ha(),
                kind: s.kind(),
                topic: s.topic(),
                polarity: s.polarity(),
                confirmed: s.confirmed(),
                switchable: s.switchable(),
                show_in_homekit: s.show_in_homekit(),
            })?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for SensorTable {
    /// Streams elements into the table — one transient [`Sensor`] at a time, a
    /// full `Vec<Sensor>` is never materialized (that peak is what OOMed the
    /// device at boot-load time).
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = SensorTable;
            fn expecting(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
                f.write_str("a sensor array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut t = SensorTable::new();
                while let Some(s) = seq.next_element::<Sensor>()? {
                    t.push(&s).map_err(serde::de::Error::custom)?;
                }
                Ok(t)
            }
        }
        deserializer.deserialize_seq(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec_size_is_bounded() -> usize {
        core::mem::size_of::<SensorRec>()
    }

    #[test]
    fn record_stays_within_32_bytes() {
        assert!(
            rec_size_is_bounded() <= 32,
            "SensorRec ist {} B — Budget 32",
            rec_size_is_bounded()
        );
    }
}

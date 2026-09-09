//! Slice 3: persistent storage on NVS.
//!
//! - [`NvsServices`] implements [`telenot_app::security::Services`] against NVS + PBKDF2.
//!   Login password and disarm PIN are stored as PBKDF2 hashes (no plaintext). The lockout
//!   counters are mirrored in NVS and survive a reboot.
//! - [`CfgStore`]: config in the dedicated 128 KB `cfg` NVS partition, authoritative
//!   format is the chunked A/B storage from `telenot_config::chunked`; a legacy JSON
//!   blob (`config`) is kept as dual-write for OTA-rollback safety
//!   (fallback: default NVS, if the partition table was flashed without `cfg`).
//!
//! HARDENING LATER: in this bench state Flash Encryption/encrypted NVS is NOT active.
//! The MQTT password must be readable back (the device connects to the broker itself) and is
//! therefore stored in plaintext in NVS — in the sealing phase it will move to encrypted NVS.
//! PIN/login are already stored only as hashes.

use esp_idf_svc::nvs::{EspCustomNvs, EspDefaultNvs, EspNvs, NvsPartitionId};
use esp_idf_svc::sys::EspError;
use pbkdf2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use pbkdf2::{Params, Pbkdf2};
use telenot_app::security::{PinCheck, Services};
use telenot_config::chunked::{self, BlobStore, StoreError};
use telenot_config::Config;

use crate::transport::PanelTarget;

// NVS keys.
const K_PW_HASH: &str = "pw_hash";
const K_PW_CHANGED: &str = "pw_changed";
const K_PIN_HASH: &str = "pin_hash";
const K_MQTT_PW: &str = "mqtt_pw";
const K_LL_FAILS: &str = "ll_fails";
const K_LL_UNTIL: &str = "ll_until";
const K_PL_FAILS: &str = "pl_fails";
const K_PL_UNTIL: &str = "pl_until";
const K_CONFIG: &str = "config";
const K_CONN: &str = "conn_tcp";
const K_BAUD: &str = "conn_baud";
const K_MQTT_CFG: &str = "mqtt_cfg";
const K_REMOTE_DISARM: &str = "remote_dis";
const K_OTA_CHECK: &str = "ota_check";
const K_BOOT_COUNT: &str = "boot_count";

// Lockout policy — identical to the host implementation in security.rs.
const LOGIN_MAX_FAILS: u32 = 5;
const LOGIN_LOCK_MS: u64 = 60_000;
const PIN_MAX_FAILS: u32 = 5;
const PIN_LOCK_MS: u64 = 300_000;

/// Legacy config blob buffer (JSON). Only used on the LEGACY path: reading a pre-chunked
/// `config` JSON blob for the one-time conversion, and as read budget while the dual-write
/// blob still exists. The authoritative storage is `telenot_config::chunked`.
/// 147 detectors produce ~30–35 KB JSON — the old 16 KB limit (and the 24 KB default NVS)
/// were the cause of "config gone after reboot".
const LEGACY_CONFIG_BUF: usize = 64 * 1024;

/// Size budget for the legacy dual-write JSON blob. Deliberately below the pre-chunked
/// firmware's 64 KB read buffer: a blob a rollback firmware could not even read would be
/// worse than no blob at all.
const LEGACY_JSON_MAX: usize = 48 * 1024;

/// PBKDF2 rounds. Pure-Rust SHA-256 does NOT use the HW SHA accelerator (~128 µs/round),
/// so kept conservative: 4096 rounds ≈ 0.5 s. For the 4-digit PIN, lockout carries the load
/// anyway. HARDENING LATER: switch to mbedTLS PBKDF2 (HW SHA, ~10× faster) in the sealing
/// phase and raise the round count significantly.
const PBKDF2_ROUNDS: u32 = 4096;

/// Monotonic boot time in ms. `pub(crate)`: also used by the OTA upload route (httpd) to
/// timestamp ring log entries.
pub(crate) fn now_ms() -> u64 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u64
}

fn random_bytes(buf: &mut [u8]) {
    unsafe {
        esp_idf_svc::sys::esp_fill_random(buf.as_mut_ptr() as *mut core::ffi::c_void, buf.len())
    }
}

fn hash_secret(secret: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    random_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    let params = Params {
        rounds: PBKDF2_ROUNDS,
        output_length: 32,
    };
    Ok(Pbkdf2
        .hash_password_customized(secret.as_bytes(), None, None, params, &salt)
        .map_err(|e| e.to_string())?
        .to_string())
}

fn verify_secret(secret: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Pbkdf2.verify_password(secret.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// Persistent failed-attempt counter (mirrors security::Lockout, but NVS-backed).
/// On reboot an active lockout window is conservatively renewed — a power cycle
/// therefore does not shorten the lockout (exact remaining time requires SNTP).
struct PersistLock {
    fails_key: &'static str,
    until_key: &'static str,
    fails: u32,
    locked_until_ms: u64,
}

impl PersistLock {
    fn load(
        nvs: &EspDefaultNvs,
        fails_key: &'static str,
        until_key: &'static str,
        window_ms: u64,
    ) -> Self {
        let fails = nvs.get_u32(fails_key).ok().flatten().unwrap_or(0);
        let was_locked = nvs.get_u64(until_key).ok().flatten().unwrap_or(0) > 0;
        // If a lockout was active on the last run, renew it conservatively.
        let locked_until_ms = if was_locked { now_ms() + window_ms } else { 0 };
        Self {
            fails_key,
            until_key,
            fails,
            locked_until_ms,
        }
    }

    fn persist(&self, nvs: &EspDefaultNvs) {
        let _ = nvs.set_u32(self.fails_key, self.fails);
        let _ = nvs.set_u64(self.until_key, self.locked_until_ms);
    }

    fn locked_for(&self, now: u64) -> Option<u64> {
        (self.locked_until_ms > now).then(|| (self.locked_until_ms - now).div_ceil(1000))
    }

    fn record_fail(&mut self, nvs: &EspDefaultNvs, now: u64, max: u32, window_ms: u64) {
        self.fails += 1;
        if self.fails >= max {
            self.locked_until_ms = now + window_ms;
            self.fails = 0;
        }
        self.persist(nvs);
    }

    fn reset(&mut self, nvs: &EspDefaultNvs) {
        self.fails = 0;
        self.locked_until_ms = 0;
        self.persist(nvs);
    }
}

pub struct NvsServices {
    nvs: EspDefaultNvs,
    token_counter: u64,
    login_lock: PersistLock,
    pin_lock: PersistLock,
}

impl NvsServices {
    /// `initial_password` = device initial password (sticker/dev default). Stored as a hash
    /// on the very first boot; afterwards the user-set password takes precedence.
    pub fn new(nvs: EspDefaultNvs, initial_password: &str) -> Result<Self, EspError> {
        // First-time init: if no password hash exists yet, store the initial hash and mark
        // pw_changed=0 (first-boot change required pending).
        if get_str(&nvs, K_PW_HASH).is_none() {
            match hash_secret(initial_password) {
                Ok(h) => {
                    let _ = nvs.set_str(K_PW_HASH, &h);
                    let _ = nvs.set_u32(K_PW_CHANGED, 0);
                    log::info!("NVS: Erst-Init — Initialpasswort hinterlegt");
                }
                Err(e) => log::error!("Argon2-Hash (Initialpasswort) fehlgeschlagen: {e}"),
            }
        }
        let login_lock = PersistLock::load(&nvs, K_LL_FAILS, K_LL_UNTIL, LOGIN_LOCK_MS);
        let pin_lock = PersistLock::load(&nvs, K_PL_FAILS, K_PL_UNTIL, PIN_LOCK_MS);
        Ok(Self {
            nvs,
            token_counter: 0,
            login_lock,
            pin_lock,
        })
    }
}

impl Services for NvsServices {
    fn now_ms(&self) -> u64 {
        now_ms()
    }

    fn new_token(&mut self) -> String {
        self.token_counter += 1;
        let mut rnd = [0u8; 16];
        random_bytes(&mut rnd);
        let mut s = String::with_capacity(32);
        for b in rnd {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    fn verify_login(&self, password: &str) -> bool {
        if password.is_empty() {
            return false;
        }
        match get_str(&self.nvs, K_PW_HASH) {
            Some(h) => verify_secret(password, &h),
            None => false,
        }
    }

    fn set_password(&mut self, new: &str) -> Result<(), String> {
        let h = hash_secret(new)?;
        self.nvs
            .set_str(K_PW_HASH, &h)
            .map_err(|e| e.to_string())?;
        self.nvs
            .set_u32(K_PW_CHANGED, 1)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn password_change_required(&self) -> bool {
        self.nvs.get_u32(K_PW_CHANGED).ok().flatten().unwrap_or(0) == 0
    }

    fn login_locked(&self) -> Option<u64> {
        self.login_lock.locked_for(now_ms())
    }

    fn note_login_fail(&mut self) {
        self.login_lock
            .record_fail(&self.nvs, now_ms(), LOGIN_MAX_FAILS, LOGIN_LOCK_MS);
    }

    fn note_login_ok(&mut self) {
        self.login_lock.reset(&self.nvs);
    }

    fn set_pin(&mut self, pin: &str) -> Result<(), String> {
        let h = hash_secret(pin)?;
        self.nvs
            .set_str(K_PIN_HASH, &h)
            .map_err(|e| e.to_string())?;
        self.pin_lock.reset(&self.nvs);
        Ok(())
    }

    fn pin_set(&self) -> bool {
        get_str(&self.nvs, K_PIN_HASH).is_some()
    }

    fn verify_pin(&mut self, pin: &str) -> PinCheck {
        let now = now_ms();
        if let Some(retry_after_s) = self.pin_lock.locked_for(now) {
            return PinCheck::Locked { retry_after_s };
        }
        let Some(stored) = get_str(&self.nvs, K_PIN_HASH) else {
            return PinCheck::NoPin;
        };
        if verify_secret(pin, &stored) {
            self.pin_lock.reset(&self.nvs);
            PinCheck::Ok
        } else {
            self.pin_lock
                .record_fail(&self.nvs, now, PIN_MAX_FAILS, PIN_LOCK_MS);
            PinCheck::Wrong
        }
    }

    fn set_mqtt_password(&mut self, pw: &str) {
        let _ = self.nvs.set_str(K_MQTT_PW, pw);
    }

    fn mqtt_password_set(&self) -> bool {
        get_str(&self.nvs, K_MQTT_PW).is_some()
    }

    fn mqtt_password(&self) -> Option<String> {
        get_str(&self.nvs, K_MQTT_PW)
    }
}

/// Reads an NVS string (up to 256 bytes) as a `String`.
fn get_str(nvs: &EspDefaultNvs, key: &str) -> Option<String> {
    let mut buf = [0u8; 256];
    match nvs.get_str(key, &mut buf) {
        Ok(Some(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Thin [`chunked::BlobStore`] adapter over `EspNvs`. All `EspNvs` blob methods take
/// `&self`, so holding a shared reference is sufficient even for the `&mut self`
/// trait methods.
struct NvsBlobStore<'a, P: NvsPartitionId>(&'a EspNvs<P>);

impl<P: NvsPartitionId> BlobStore for NvsBlobStore<'_, P> {
    fn blob_len(&self, key: &str) -> Result<Option<usize>, StoreError> {
        self.0
            .blob_len(key)
            .map_err(|e| StoreError(format!("blob_len({key}): {e}")))
    }

    fn get_blob(&self, key: &str, buf: &mut [u8]) -> Result<Option<usize>, StoreError> {
        self.0
            .get_blob(key, buf)
            .map(|opt| opt.map(|bytes| bytes.len()))
            .map_err(|e| StoreError(format!("get_blob({key}): {e}")))
    }

    fn set_blob(&mut self, key: &str, val: &[u8]) -> Result<(), StoreError> {
        self.0
            .set_blob(key, val)
            .map_err(|e| StoreError(format!("set_blob({key}): {e}")))?;
        // Verify each bounded blob before acknowledging the save, including the meta commit.
        let mut readback = vec![0; val.len()];
        let stored = self
            .0
            .get_blob(key, &mut readback)
            .map_err(|e| StoreError(format!("readback({key}): {e}")))?;
        if stored != Some(val) {
            return Err(StoreError(format!(
                "Readback-Pruefung fehlgeschlagen: {key}"
            )));
        }
        Ok(())
    }

    fn remove(&mut self, key: &str) -> Result<(), StoreError> {
        // EspNvs::remove is already idempotent: a missing key yields Ok(false)
        // (ESP_ERR_NVS_NOT_FOUND is mapped, not propagated) — exactly the trait contract.
        self.0
            .remove(key)
            .map(|_existed| ())
            .map_err(|e| StoreError(format!("remove({key}): {e}")))
    }
}

/// Outcome of the legacy dual-write in [`CfgStore::save`].
///
/// The dual-write exists so an OTA **rollback** to a pre-chunked firmware finds a
/// current JSON config under the old `config` key. Above the legacy size budget
/// ([`LEGACY_JSON_MAX`]) the blob is removed instead of silently going stale — a
/// rollback then boots unconfigured (visible) rather than with an outdated inventory
/// (invisible).
pub enum DualWrite {
    /// Legacy JSON blob was (re)written — a rollback firmware sees the current config.
    Kept,
    /// Config exceeds the legacy budget — the legacy blob was removed.
    Dropped,
}

/// Storage location for the sensor config. Normal case is the dedicated 128 KB `cfg` partition;
/// if the partition table was flashed without `cfg`, the store falls back to the default NVS
/// (only ~16 KB usable) rather than aborting boot with an error.
pub enum CfgStore {
    Big(EspCustomNvs),
    Fallback(EspDefaultNvs),
}

impl CfgStore {
    pub fn is_fallback(&self) -> bool {
        matches!(self, CfgStore::Fallback(_))
    }

    /// Loads the config (or `Config::default` if none present or invalid).
    ///
    /// Order: chunked A/B storage (authoritative) → legacy JSON blob (with one-time
    /// conversion to chunked) → `Config::default()`. A chunked load *error* never
    /// aborts — it is logged and the legacy path is tried, so a corrupted chunked
    /// state cannot cost more than what the legacy blob still holds.
    pub fn load(&self) -> Config {
        match self {
            CfgStore::Big(nvs) => load_any(nvs),
            CfgStore::Fallback(nvs) => load_any(nvs),
        }
    }

    /// Saves the config: chunked A/B storage first (authoritative, fail-closed — an
    /// error here means nothing was applied), then the legacy JSON dual-write
    /// (see [`DualWrite`]; its errors are only logged, never fatal).
    pub fn save(&self, cfg: &Config) -> Result<DualWrite, String> {
        match self {
            CfgStore::Big(nvs) => save_any(nvs, cfg),
            CfgStore::Fallback(nvs) => save_any(nvs, cfg),
        }
    }

    /// One-time migration: if the `cfg` partition is empty but the old default NVS contains
    /// a valid config blob (state before the partition switch), import it — otherwise an
    /// existing device boots unconfigured after the update.
    pub fn migrate_from_default(&self, old: &EspDefaultNvs) {
        let CfgStore::Big(new) = self else { return };
        if matches!(new.blob_len(K_CONFIG), Ok(Some(_))) {
            return; // cfg partition already has a config
        }
        // Old limit was 16 KB — the default NVS could never hold more.
        let mut buf = vec![0u8; 16 * 1024];
        let Ok(Some(bytes)) = old.get_blob(K_CONFIG, &mut buf) else {
            return;
        };
        if Config::from_json(bytes).is_err() {
            return;
        }
        match new.set_blob(K_CONFIG, bytes) {
            Ok(_) => log::info!(
                "Config aus Default-NVS in cfg-Partition migriert ({} B)",
                bytes.len()
            ),
            Err(e) => log::error!("Config-Migration fehlgeschlagen: {e}"),
        }
    }
}

fn load_any<P: NvsPartitionId>(nvs: &EspNvs<P>) -> Config {
    // (a)/(b) Chunked storage is authoritative. An error is logged but NEVER aborts the
    // load — the legacy blob below may still hold a usable config.
    match chunked::load(&NvsBlobStore(nvs)) {
        Ok(Some(cfg)) => {
            log::info!("Config geladen (chunked, {} Sensoren)", cfg.sensors.len());
            return cfg;
        }
        Ok(None) => {}
        Err(e) => log::error!("Chunked-Config laden fehlgeschlagen: {e} — versuche Legacy-JSON"),
    }
    // (c) Legacy JSON blob (pre-chunked firmware state).
    if let Some(cfg) = load_legacy(nvs) {
        // One-time conversion into the chunked storage; a failure only logs — the config
        // itself is valid and gets used either way. The legacy blob is deliberately NOT
        // removed (rollback safety; the next save refreshes it as dual-write).
        match chunked::save(&mut NvsBlobStore(nvs), &cfg) {
            Ok(()) => log::info!(
                "Legacy-JSON-Config nach chunked konvertiert ({} Sensoren)",
                cfg.sensors.len()
            ),
            Err(e) => log::error!("Einmal-Konvertierung nach chunked fehlgeschlagen: {e}"),
        }
        return cfg;
    }
    // (d) Nothing stored.
    Config::default()
}

/// Legacy path: reads the pre-chunked JSON blob under `config`.
/// `None` = missing, oversized or invalid (details logged).
fn load_legacy<P: NvsPartitionId>(nvs: &EspNvs<P>) -> Option<Config> {
    // Allocate exactly to blob size (rather than blanket LEGACY_CONFIG_BUF) — saves boot heap.
    let len = match nvs.blob_len(K_CONFIG) {
        Ok(Some(l)) if l <= LEGACY_CONFIG_BUF => l,
        Ok(Some(l)) => {
            log::warn!("NVS-Config unerwartet groß ({l} B > {LEGACY_CONFIG_BUF}) → Default");
            return None;
        }
        _ => return None,
    };
    let mut buf = vec![0u8; len];
    match nvs.get_blob(K_CONFIG, &mut buf) {
        Ok(Some(bytes)) => match Config::from_json(bytes) {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("NVS-Config ungültig ({e:?}) → Default");
                None
            }
        },
        _ => None,
    }
}

fn save_any<P: NvsPartitionId>(nvs: &EspNvs<P>, cfg: &Config) -> Result<DualWrite, String> {
    // (a) Authoritative chunked save — fail-closed: on error nothing is applied,
    // exactly like the previous JSON-only save.
    chunked::save(&mut NvsBlobStore(nvs), cfg).map_err(|e| e.to_string())?;

    // (b) Legacy dual-write for OTA-rollback safety (see [`DualWrite`]). All failures
    // on this path are logged only — the authoritative save above already succeeded.
    match cfg.to_json() {
        Ok(json) if json.len() <= LEGACY_JSON_MAX => {
            if let Err(e) = nvs.set_blob(K_CONFIG, json.as_bytes()) {
                log::error!("Legacy-Dual-Write fehlgeschlagen: {e}");
            }
            Ok(DualWrite::Kept)
        }
        other => {
            // TooLarge/serialization error OR above the legacy budget: remove the blob
            // instead of letting it silently go stale.
            if let Err(e) = other {
                log::warn!("Legacy-Dual-Write: JSON nicht erzeugbar ({e}) — Blob wird entfernt");
            }
            if let Err(e) = nvs.remove(K_CONFIG) {
                log::error!("Legacy-Blob entfernen fehlgeschlagen: {e}");
            }
            Ok(DualWrite::Dropped)
        }
    }
}

/// Persisted panel target from the setup step "connection", so the bridge reconnects to the
/// real panel after a reboot. Value format in the (historical) key `conn_tcp`:
/// `"internal"` = internal RS232, otherwise `ip:port` (USR-TCP232) —
/// existing devices with a saved TCP target remain compatible.
pub fn load_conn(nvs: &EspDefaultNvs) -> Option<PanelTarget> {
    let s = get_str(nvs, K_CONN)?;
    if s == "internal" {
        return Some(PanelTarget::Internal);
    }
    let (ip, port) = s.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    (!ip.is_empty() && port != 0).then(|| PanelTarget::Tcp(ip.to_string(), port))
}

pub fn save_conn(nvs: &EspDefaultNvs, target: &PanelTarget) {
    let s = match target {
        PanelTarget::Internal => "internal".to_string(),
        PanelTarget::Tcp(ip, port) => format!("{ip}:{port}"),
    };
    let _ = nvs.set_str(K_CONN, &s);
}

/// Persisted serial baud rate (setup step "connection"). Default 9600 (complex/GMS lite);
/// GMS plus needs 115200. Absent/invalid key → default, so existing devices are unchanged.
pub fn load_baud(nvs: &EspDefaultNvs) -> u32 {
    match nvs.get_u32(K_BAUD) {
        Ok(Some(b)) if telenot_app::BAUD_ALLOWED.contains(&b) => b,
        _ => 9600,
    }
}

pub fn save_baud(nvs: &EspDefaultNvs, baud: u32) {
    let _ = nvs.set_u32(K_BAUD, baud);
}

/// Persisted remote-disarm master switch (security step). Without persistence every reboot
/// would reset it fail-closed to OFF — disarm via HA/UI would never work despite a PIN being
/// set ("DENIED Remote-Disarm nicht aktiviert").
pub fn load_remote_disarm(nvs: &EspDefaultNvs) -> bool {
    matches!(nvs.get_u8(K_REMOTE_DISARM), Ok(Some(1)))
}

pub fn save_remote_disarm(nvs: &EspDefaultNvs, on: bool) {
    let _ = nvs.set_u8(K_REMOTE_DISARM, on as u8);
}

/// Daily update check (default ON — outbound only, installation is manual).
pub fn load_ota_check(nvs: &EspDefaultNvs) -> bool {
    nvs.get_u8(K_OTA_CHECK)
        .ok()
        .flatten()
        .map(|v| v != 0)
        .unwrap_or(true)
}

pub fn save_ota_check(nvs: &EspDefaultNvs, on: bool) {
    let _ = nvs.set_u8(K_OTA_CHECK, on as u8);
}

/// Increment and return the boot counter (diagnostics: makes silent reboot loops visible —
/// a rapidly climbing counter with short uptime is the smoke signal).
pub fn bump_boot_count(nvs: &EspDefaultNvs) -> u32 {
    let n = nvs
        .get_u32(K_BOOT_COUNT)
        .ok()
        .flatten()
        .unwrap_or(0)
        .wrapping_add(1);
    let _ = nvs.set_u32(K_BOOT_COUNT, n);
    n
}

/// Persisted MQTT broker settings (non-secret; the password is stored separately as `mqtt_pw`).
/// Stored as a JSON blob so the bridge reconnects to the same broker after a reboot —
/// `MqttSettings` is NOT part of the `Config` and would otherwise be lost.
pub fn load_mqtt(nvs: &EspDefaultNvs) -> Option<telenot_app::MqttSettings> {
    // Blob (not set_str): a TOFU-pinned cert PEM (~2 KB) exceeds the NVS string limit.
    let mut buf = vec![0u8; 8 * 1024];
    match nvs.get_blob(K_MQTT_CFG, &mut buf) {
        Ok(Some(bytes)) => serde_json::from_slice(bytes).ok(),
        _ => None,
    }
}

pub fn save_mqtt(nvs: &EspDefaultNvs, m: &telenot_app::MqttSettings) {
    if let Ok(json) = serde_json::to_vec(m) {
        let _ = nvs.set_blob(K_MQTT_CFG, &json);
    }
}

//! serde DTOs = wire contract with the Preact frontend (`web/src/main.jsx`, `window.API`
//! plus mock data there; contract documented in `docs/REST-CONTRACT.md`). Enums are
//! snake_case and addresses are u16 numbers (no hex strings).

use serde::{Deserialize, Serialize};
use telenot_config::{Polarity, SensorKind, Severity};

// ───────────────────────── Device info ─────────────────────────

/// Static device identity (S0). Provided by the daemon/firmware.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub model: String,
    pub fw: String,
    /// Build identification beyond the CalVer version: git short hash (`*` = dirty tree)
    /// plus commit time — distinguishes multiple dev builds of the same version.
    /// Empty when unavailable (host builds without git).
    #[serde(default)]
    pub fw_build: String,
    pub serial: String,
    pub mac: String,
    pub ip: String,
    pub schema: u32,
    pub fingerprint: String,
    /// A persisted config with sensors exists (publicly queryable so the UI can distinguish
    /// a "configured device" from first-time setup after a reboot — without a session).
    #[serde(default)]
    pub configured: bool,
}

impl Default for DeviceInfo {
    fn default() -> Self {
        DeviceInfo {
            model: "EMA-Bridge (Host)".into(),
            fw: env!("CARGO_PKG_VERSION").into(),
            fw_build: String::new(),
            serial: "HOST-DEV".into(),
            mac: "00:00:00:00:00:00".into(),
            ip: "http://127.0.0.1".into(),
            schema: telenot_config::CURRENT_SCHEMA_VERSION,
            fingerprint: "—".into(),
            configured: false,
        }
    }
}

// ───────────────────────── Sensors ─────────────────────────

/// Derived confirmation status for the UI (NOT in the persistent schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorStatus {
    Unconfirmed,
    Confirmed,
    Excluded,
}

/// Sensor view for the table = persistent `Sensor` plus view-only/derived fields
/// (`raw_name`, `dup_of`, `status`, `include`) that are NEVER persisted.
#[derive(Debug, Clone, Serialize)]
pub struct ApiSensor {
    pub address: u16,
    pub name: String,
    pub name_ha: String,
    pub kind: SensorKind,
    pub topic: String,
    pub polarity: Polarity,
    pub confirmed: bool,
    /// Switch authorization (`OUTPUT_ON/OFF` allowlist) — meaningful only for outputs.
    pub switchable: bool,
    /// Mirror as a HomeKit accessory (direct HomeKit mode).
    pub show_in_homekit: bool,
    /// Original name from the panel (read-only).
    pub raw_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dup_of: Option<u16>,
    pub status: SensorStatus,
    pub include: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Counts {
    pub confirmed: usize,
    pub unconfirmed: usize,
    pub excluded: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SensorsResp {
    /// Total count (pagination: `sensors` is only the page starting at `offset`).
    pub total: usize,
    pub offset: usize,
    pub sensors: Vec<ApiSensor>,
    pub counts: Counts,
}

/// Partial sensor update (S4 inline edit / drawer).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SensorPatch {
    pub name: Option<String>,
    pub name_ha: Option<String>,
    pub kind: Option<SensorKind>,
    pub polarity: Option<Polarity>,
    pub topic: Option<String>,
    pub confirmed: Option<bool>,
    pub include: Option<bool>,
    pub switchable: Option<bool>,
    pub show_in_homekit: Option<bool>,
}

/// Bulk operation on multiple addresses (S4 batch actions + "confirm group").
#[derive(Debug, Clone, Deserialize)]
pub struct BulkReq {
    pub addresses: Vec<u16>,
    pub op: BulkOp,
    #[serde(default)]
    pub kind: Option<SensorKind>,
    #[serde(default)]
    pub polarity: Option<Polarity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkOp {
    Confirm,
    Exclude,
    Include,
    SetKind,
    SetPolarity,
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkResp {
    pub updated: usize,
}

// ───────────────────────── Connection (S2) ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnKind {
    Internal,
    Tcp,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionDto {
    #[serde(rename = "type")]
    pub kind: ConnKind,
    pub ip: String,
    pub port: u16,
    pub baud: u32,
    pub panel_kind: telenot_config::PanelKind,
    pub gms_variant: telenot_config::GmsVariant,
    pub hiplex_cmds_verified: bool,
    /// Eager-send (fire immediately vs. wait for the panel poll window). See
    /// `PanelSettings::eager_send`. Default ON.
    pub eager_send: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionPut {
    #[serde(rename = "type")]
    pub kind: ConnKind,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub port: u16,
    /// Absent/0 (old web bundles) = keep the current baud.
    #[serde(default)]
    pub baud: u32,
    /// Absent (old web bundles) = keep the current panel selection.
    #[serde(default)]
    pub panel_kind: Option<telenot_config::PanelKind>,
    #[serde(default)]
    pub gms_variant: Option<telenot_config::GmsVariant>,
    /// hiplex command gate — see `PanelSettings::hiplex_cmds_verified`.
    #[serde(default)]
    pub hiplex_cmds_verified: Option<bool>,
    /// Eager-send toggle — see `PanelSettings::eager_send`. Absent = keep current.
    #[serde(default)]
    pub eager_send: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnCheckDto {
    /// `ok` | `error`.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_frame_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel_address: Option<u16>,
    pub detail: String,
}

// ───────────────────────── Scan (S3) ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPhase {
    Idle,
    Belegt,
    Naming,
    Done,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanDto {
    pub phase: ScanPhase,
    pub total: usize,
    pub scanned: usize,
    pub named: usize,
    pub elapsed: u64,
    pub remaining: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<u16>,
    pub feed: Vec<ApiSensor>,
}

// ───────────────────────── MQTT (S5) ─────────────────────────

/// HomeKit pairing info for the setup web UI: setup code + `X-HM://…` payload (→ QR code).
/// Set by the firmware (homekit.rs) once HAP is running; `None` outside HomeKit mode.
#[derive(Debug, Clone, Serialize)]
pub struct HomekitPair {
    pub code: String,
    pub payload: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MqttDto {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: String,
    pub password_set: bool,
    pub topic_root: String,
    pub ha_discovery: bool,
    /// Is a self-signed broker certificate pinned (TOFU)?
    pub pinned: bool,
    /// Direct HomeKit mode active (MQTT off, HAP on — started live).
    pub homekit_mode: bool,
    /// Is HomeKit allowed to disarm? (Default OFF = fail-closed, arm-only.)
    pub homekit_disarm: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MqttPut {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    #[serde(default)]
    pub username: String,
    pub topic_root: String,
    #[serde(default)]
    pub ha_discovery: bool,
    /// Empty/missing = unchanged. Written write-only into the SecretStore.
    #[serde(default)]
    pub password: Option<String>,
    /// Direct HomeKit mode (started live).
    #[serde(default)]
    pub homekit_mode: bool,
    /// Is HomeKit allowed to disarm? (Default OFF = fail-closed.)
    #[serde(default)]
    pub homekit_disarm: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MqttTestDto {
    pub ok: bool,
    /// `ok` | `connect` | `tls` | `auth`.
    pub detail: String,
    pub label: String,
}

// ───────────────────────── Security (S6) ─────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SecurityDto {
    pub pin_set: bool,
    pub remote_disarm: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PinPut {
    pub pin: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PinResp {
    pub pin_set: bool,
    pub weak: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteDisarmPut {
    pub enabled: bool,
    #[serde(default)]
    pub acknowledged: bool,
}

// ───────────────────────── Session (S1) ─────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct LoginReq {
    pub password: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginResp {
    pub csrf_token: String,
    pub password_change_required: bool,
    pub device: DeviceInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PasswordReq {
    pub new_password: String,
    /// Current password. Optional on the first-boot mandatory change (the login already
    /// proved the initial password); **required** and verified on later rotations.
    #[serde(default)]
    pub current_password: Option<String>,
}

// ───────────────────────── Diagnostics (S7) ─────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SerialDiag {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_frame_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MqttDiag {
    pub status: String,
    pub reconnects: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub last_pub: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeapDiag {
    pub free: u64,
    pub largest_free_block: u64,
    pub low: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsDto {
    pub serial: SerialDiag,
    pub mqtt: MqttDiag,
    /// `None` on the host (no ESP32 heap) → field omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heap: Option<HeapDiag>,
    pub firmware_version: String,
    pub schema_version: u32,
    pub uptime_s: u64,
    pub reset_reason: String,
    /// Boots since first commissioning (0 on the host).
    pub boot_count: u32,
    /// Heap dropped below the warning threshold since boot (latch).
    pub heap_low: bool,
    /// Remaining setup-window time (HTTP access) in seconds. `None` on the host sim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_window_s_remaining: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntryDto {
    pub seq: u64,
    pub t_ms: u64,
    pub level: String,
    pub msg: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogResp {
    pub entries: Vec<LogEntryDto>,
    pub dropped: u64,
}

// ───────────────────────── Debug capture (GMS capture for foreign panels) ─────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct CaptureStartReq {
    /// "listen" | "listen_ack" | "discover".
    pub mode: String,
}

/// A seen record type with its count (live interpretation: "which GMS records are arriving").
#[derive(Debug, Clone, Serialize)]
pub struct RecTypeCount {
    pub satztyp: u8,
    /// Hex representation (e.g. "0x24") for the UI.
    pub hex: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureStatusDto {
    pub active: bool,
    /// "listen" | "listen_ack" | "discover".
    pub mode: String,
    /// Human-readable read/write declaration (answers "is this reading or writing to my panel?").
    pub sends: String,
    pub bytes_total: u64,
    pub frames_ok: u32,
    pub frames_err: u32,
    pub rec_types: Vec<RecTypeCount>,
    pub elapsed_s: u64,
    /// Bytes currently held in the ring (for download).
    pub buf_used: usize,
    pub buf_cap: usize,
}

// ───────────────────────── Live state (header pulse) ─────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SensorStateDto {
    pub address: u16,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolarityObservedDto {
    pub address: u16,
    pub polarity: Polarity,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateDto {
    pub arm_state: String,
    pub availability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intern_ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extern_ready: Option<bool>,
    pub sensor_states: Vec<SensorStateDto>,
    /// Suggested polarity from the last observation window (S4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity_observed: Option<PolarityObservedDto>,
    /// Letztes Befehls-Ergebnis (`OK …`/`NAK …`/`TIMEOUT …`/`FEHLER …`/`DENIED …`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_result: Option<CommandResultDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandResultDto {
    pub text: String,
    pub ms_ago: u64,
}

// ───────────────────────── Review / Commit (S8) ─────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct WarningDto {
    /// Stable code (config issue like `sensor_unconfirmed`/`polarity_unconfirmed` or
    /// state-based `mqtt_never_tested`/`remote_disarm_active`) → maps to `s8.w.*`.
    pub code: String,
    pub severity: Severity,
    pub count: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewDto {
    pub counts: Counts,
    pub mqtt_target: String,
    pub mqtt_tested: bool,
    pub ha_discovery: bool,
    pub remote_disarm: bool,
    pub schema_version: u32,
    pub warnings: Vec<WarningDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitReq {
    #[serde(default)]
    pub warnings_acknowledged: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitResp {
    pub rebooting: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanCancelReq {
    #[serde(default)]
    pub keep_partial: bool,
}

/// Control command (live test board / MQTT): arm/disarm/reset or `BYPASS_ON`/`BYPASS_OFF`.
/// `pin` for security-reducing commands (fail-closed), `mb` (1–128) only for bypass.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandReq {
    pub cmd: String,
    #[serde(default)]
    pub pin: Option<String>,
    #[serde(default)]
    pub mb: Option<u16>,
    /// GMS output address (only for `OUTPUT_ON`/`OUTPUT_OFF`).
    #[serde(default)]
    pub addr: Option<u16>,
    /// Sicherungsbereich for arm commands (multi-area panels; absent/1 = area 1).
    #[serde(default)]
    pub area: Option<u8>,
}

// ───────────────────────── OTA ─────────────────────────

/// `GET /api/v1/ota` — upload progress, slot overview, and self-test state.
#[derive(Debug, Clone, Serialize)]
pub struct OtaStatusDto {
    /// `idle` | `receiving` | `ready_to_reboot` | `failed`
    pub state: &'static str,
    pub received: usize,
    pub total: Option<usize>,
    pub progress_pct: u8,
    pub error: Option<&'static str>,
    pub new_version: Option<String>,
    pub running_slot: Option<crate::ota::SlotInfo>,
    pub boot_slot: Option<crate::ota::SlotInfo>,
    /// Firmware is running in self-test (PENDING_VERIFY) — rolls back on panic/power-cycle.
    pub pending_verify: bool,
    pub self_test_s_remaining: Option<u32>,
    /// Newer version reported by the update check (only set when genuinely newer).
    pub latest_version: Option<String>,
    pub update_check: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OtaSettingsPut {
    pub update_check: bool,
}

// ───────────────────────── Backup ─────────────────────────

/// Full settings backup (export/import, `/api/v1/backup`). Deliberately excludes secrets:
/// PIN, login password, MQTT password, and HomeKit pairing/code are write-only or
/// device-bound and must be re-entered after import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupDto {
    /// Format version of this backup (for future migrations).
    pub backup_version: u32,
    /// Sensor configuration (carries its own `schema_version`).
    pub config: telenot_config::Config,
    /// MQTT/HomeKit settings — `homekit_code` is cleared on export.
    pub mqtt: crate::app::MqttSettings,
    pub conn: BackupConn,
    pub remote_disarm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConn {
    #[serde(rename = "type")]
    pub kind: ConnKind,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub port: u16,
    /// 0/absent (older backups) = keep the device's current baud on import.
    #[serde(default)]
    pub baud: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupImportResp {
    /// Anzahl importierter Sensoren.
    pub sensors: usize,
}

// ───────────────────────── Fehler-Envelope ─────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

//! The `App` state: everything the HTTP handlers read/write. Intentionally separate from
//! `Runtime` (serial owner): the daemon mirrors `Runtime::snapshot()` into [`App::live`] and
//! drains [`App::intents`]; handlers never touch the core directly.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use telenot_config::{Config, SensorTable, CURRENT_SCHEMA_VERSION};

use crate::dto::{ApiSensor, ConnKind, Counts, DeviceInfo, SensorStatus};
use crate::dup;
use crate::mqtt::MqttTestResult;
use crate::runtime::{Intent, LiveSnapshot};
use crate::security::Services;

/// Editable sensor inventory of the setup session: ONE compact [`SensorTable`] plus
/// index-parallel view state. Created lazily (copy-on-write from the persisted config)
/// or moved in from a scan result — the steady state holds no second sensor copy.
#[derive(Debug)]
pub struct EditSession {
    pub table: SensorTable,
    /// Parallel to the table indices: excluded from the configuration
    /// (UI concept, NOT persisted — dropped on commit).
    pub excluded: Vec<bool>,
    /// Only filled after a scan (installer raw names).
    pub raw_names: BTreeMap<u16, String>,
    pub dup_of: BTreeMap<u16, u16>,
}

impl EditSession {
    /// Session from a (discovery or file) config + raw names. Moves the sensor table
    /// (no copy); generic bus slots are hidden by default (no HA clutter).
    pub fn from_scan(config: Config, raw_names: BTreeMap<u16, String>) -> Self {
        let dup_input: Vec<(u16, String, _)> = config
            .sensors
            .iter()
            .map(|s| (s.address(), s.name().to_string(), s.kind()))
            .collect();
        let dup_of = dup::compute_dup_map(&dup_input);
        // Hide generic bus slots (BT-Adr./BuildSec-Adr.) by default → no HA clutter.
        let excluded = config
            .sensors
            .iter()
            .map(|s| {
                let raw_name = raw_names
                    .get(&s.address())
                    .map(String::as_str)
                    .unwrap_or_else(|| s.name());
                telenot_config::is_generic_bus_label(raw_name)
            })
            .collect();
        EditSession {
            table: config.sensors,
            excluded,
            raw_names,
            dup_of,
        }
    }

    /// Session from the PERSISTED config (copy-on-write on first edit). Unlike
    /// [`Self::from_scan`], nothing is auto-hidden: the persisted config is a
    /// deliberate selection.
    pub fn from_persisted(config: &Config) -> Self {
        EditSession {
            excluded: vec![false; config.sensors.len()],
            table: config.sensors.clone(),
            raw_names: BTreeMap::new(),
            dup_of: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Index of the first sensor with this address (addresses may repeat: OR-linked contacts).
    pub fn find_idx(&self, address: u16) -> Option<usize> {
        self.table.iter().position(|s| s.address() == address)
    }

    /// API view of the sensor at `idx` (panics on out-of-range — callers index via
    /// `find_idx`/`len`).
    pub fn api_at(&self, idx: usize) -> ApiSensor {
        let s = self.table.get(idx).expect("idx within session table");
        let excluded = self.excluded[idx];
        let status = if excluded {
            SensorStatus::Excluded
        } else if s.confirmed() {
            SensorStatus::Confirmed
        } else {
            SensorStatus::Unconfirmed
        };
        ApiSensor {
            address: s.address(),
            name: s.name().to_string(),
            name_ha: s.name_ha().to_string(),
            kind: s.kind(),
            topic: s.topic().to_string(),
            location: s.location().to_string(),
            polarity: s.polarity(),
            confirmed: s.confirmed(),
            switchable: s.switchable(),
            show_in_homekit: s.show_in_homekit(),
            raw_name: self
                .raw_names
                .get(&s.address())
                .cloned()
                .unwrap_or_else(|| s.name().to_string()),
            dup_of: self.dup_of.get(&s.address()).copied(),
            status,
            include: !excluded,
        }
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts {
            confirmed: 0,
            unconfirmed: 0,
            excluded: 0,
        };
        for (s, &ex) in self.table.iter().zip(&self.excluded) {
            if ex {
                c.excluded += 1;
            } else if s.confirmed() {
                c.confirmed += 1;
            } else {
                c.unconfirmed += 1;
            }
        }
        c
    }

    /// Known rooms (for dropdowns): distinct, non-empty locations, sorted.
    pub fn rooms(&self) -> Vec<String> {
        let mut set: Vec<String> = self
            .table
            .iter()
            .map(|s| s.location().to_string())
            .filter(|l| !l.trim().is_empty())
            .collect();
        set.sort();
        set.dedup();
        set
    }

    /// Validates the session as the commit gate would see it: the included inventory
    /// (excluded entries filtered) + the given panel settings — without materializing
    /// a second [`SensorTable`].
    pub fn issues(&self, panel: &telenot_config::PanelSettings) -> Vec<telenot_config::Issue> {
        let included = self.excluded.iter().filter(|&&ex| !ex).count();
        let mut issues = telenot_config::validate_inventory(
            CURRENT_SCHEMA_VERSION,
            included,
            self.table
                .iter()
                .zip(&self.excluded)
                .filter(|(_, &ex)| !ex)
                .map(|(s, _)| s),
        );
        issues.extend(telenot_config::validate_panel(panel));
        issues
    }

    /// Consumes the session into the config to be persisted: excluded sensors are
    /// dropped in place (no second table). INFALLIBLE — a failed `compact` only
    /// means leftover arena waste, never a lost commit.
    pub fn into_config(mut self, panel: telenot_config::PanelSettings) -> Config {
        let mut i = 0;
        let excluded = std::mem::take(&mut self.excluded);
        self.table.retain(|_| {
            let keep = !excluded[i];
            i += 1;
            keep
        });
        let _ = self.table.compact();
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: self.table,
            panel,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MqttSettings {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// UNUSED since the insecure "do not verify" mode was removed (it never worked on
    /// ESP-IDF: esp-mqtt has no chain-verification opt-out). Kept only so persisted
    /// configs and backups from older versions keep deserializing.
    #[serde(default = "default_verify_cert")]
    pub verify_cert: bool,
    pub username: String,
    pub topic_root: String,
    pub ha_discovery: bool,
    /// TOFU-pinned broker certificate (PEM). When set, TLS connections are verified against
    /// EXACTLY this cert (self-signed but MITM-safe). `None` = public CA bundle.
    #[serde(default)]
    pub pinned_cert: Option<String>,
    /// **Direct HomeKit** mode instead of MQTT: the ESP32 speaks HAP itself (no MQTT client).
    /// Started **live** when enabled (no reboot needed); persisted for the boot case.
    #[serde(default)]
    pub homekit_mode: bool,
    /// May HomeKit **disarm** the panel? Default OFF (fail-closed → HomeKit can only arm).
    /// Enabling is a deliberate setup decision: the paired HomeKit device then serves as
    /// the authorization token (HomeKit does not pass a per-action PIN). See
    /// [`crate::homekit_authorized`].
    #[serde(default)]
    pub homekit_disarm: bool,
    /// Device-specific HomeKit pairing code (`xxx-xx-xxx`). Empty = randomly generated and
    /// persisted on the first HAP start (NOT the public Espressif sample code). Treated as
    /// write-only: shown only in the setup web UI, never exposed via MQTT or other channels.
    #[serde(default)]
    pub homekit_code: String,
}

fn default_verify_cert() -> bool {
    true
}

impl Default for MqttSettings {
    fn default() -> Self {
        MqttSettings {
            host: String::new(),
            port: 8883,
            tls: true,
            verify_cert: true,
            username: String::new(),
            topic_root: "telenot/v1".into(),
            ha_discovery: true,
            pinned_cert: None,
            // Default HomeKit: freshly flashed devices start directly in HomeKit ("flash & happy").
            // MQTT is the advanced path. Already-persisted configs keep their mode.
            homekit_mode: true,
            homekit_disarm: false,
            homekit_code: String::new(),
        }
    }
}

/// Baud rates the serial layer accepts (9600 = complex/GMS lite, 115200 = GMS plus).
pub const BAUD_ALLOWED: [u32; 2] = [9600, 115_200];

#[derive(Debug, Clone)]
pub struct ConnSettings {
    pub kind: ConnKind,
    pub ip: String,
    pub port: u16,
    /// Serial baud rate (internal RS232). 9600 = complex/GMS lite; 115200 = GMS plus.
    /// For TCP targets the USR-TCP232 sets its own serial baud — this value is applied
    /// to the internal UART only.
    pub baud: u32,
}

impl Default for ConnSettings {
    fn default() -> Self {
        ConnSettings {
            kind: ConnKind::Internal,
            ip: String::new(),
            port: 0,
            baud: 9600,
        }
    }
}

/// State of an MQTT connection test (async; the worker writes the result back).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestState {
    Idle,
    Running,
    Done,
}

#[derive(Debug, Clone)]
pub struct MqttTestView {
    pub state: TestState,
    pub result: Option<MqttTestResult>,
    /// Broker certificate presented during the test (set on untrusted TLS for TOFU
    /// pinning AND on success for fingerprint inspection). The frontend shows the
    /// fingerprint/issuer; `pem` stays on-device for pinning.
    pub cert: Option<PendingCert>,
}

impl Default for MqttTestView {
    fn default() -> Self {
        MqttTestView {
            state: TestState::Idle,
            result: None,
            cert: None,
        }
    }
}

/// A broker certificate presented during the TLS test (for trust-on-first-use).
#[derive(Debug, Clone)]
pub struct PendingCert {
    /// SHA-256 fingerprint (hex, `AA:BB:…`).
    pub sha256: String,
    pub subject: String,
    pub issuer: String,
    /// PEM — on-device only, NEVER included in an HTTP response (used only for pinning).
    pub pem: String,
}

/// Result of a connection probe (real TCP connect to the configured target).
#[derive(Debug, Clone)]
pub struct ConnCheckView {
    pub state: TestState,
    pub ok: bool,
    pub detail: String,
    pub last_frame_ms: Option<u64>,
}

impl Default for ConnCheckView {
    fn default() -> Self {
        ConnCheckView {
            state: TestState::Idle,
            ok: false,
            detail: String::new(),
            last_frame_ms: None,
        }
    }
}

impl ConnCheckView {
    /// Condenses a TCP probe (connect + listen window) into a result. `ok` only when the
    /// panel actually delivers bytes — a reachable port is not enough (the USR-TCP232
    /// accepts connections even without an attached/sending GMS). Single source of truth
    /// for the host daemon and firmware; detail strings match the frontend i18n keys.
    pub fn from_probe(connected: bool, data_seen: bool) -> Self {
        ConnCheckView {
            state: TestState::Done,
            ok: connected && data_seen,
            detail: match (connected, data_seen) {
                (false, _) => "connect",
                (true, false) => "no_frame",
                (true, true) => "reachable",
            }
            .into(),
            last_frame_ms: (connected && data_seen).then_some(0),
        }
    }
}

/// A ring-log entry.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub seq: u64,
    pub t_ms: u64,
    pub level: &'static str,
    pub msg: String,
}

/// Low-churn ring buffer for diagnostic logs (shared by the web UI and later MQTT).
#[derive(Debug)]
pub struct RingLog {
    entries: VecDeque<LogLine>,
    cap: usize,
    next_seq: u64,
    dropped: u64,
}

impl RingLog {
    pub fn new(cap: usize) -> Self {
        RingLog {
            entries: VecDeque::new(),
            cap,
            next_seq: 1,
            dropped: 0,
        }
    }
    pub fn push(&mut self, t_ms: u64, level: &'static str, msg: String) {
        // Cap message length → bounded heap budget (cap × ~250 B worst-case
        // instead of unbounded; ESP32 stability trumps log completeness).
        const MAX_MSG: usize = 200;
        let msg = if msg.len() > MAX_MSG {
            // Walk back to a valid UTF-8 boundary (at most 3 B for 4-byte chars).
            // `cut == 0` is impossible (UTF-8 never starts with a continuation byte), but
            // the lower bound makes it robust against future changes to MAX_MSG.
            let mut cut = MAX_MSG;
            while cut > 0 && !msg.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}…", &msg[..cut])
        } else {
            msg
        };
        let seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(LogLine {
            seq,
            t_ms,
            level,
            msg,
        });
        while self.entries.len() > self.cap {
            self.entries.pop_front();
            self.dropped += 1;
        }
    }
    /// Entries with `seq > since`. Returns (entries, total dropped so far).
    pub fn since(&self, since: u64) -> (Vec<&LogLine>, u64) {
        (
            self.entries.iter().filter(|e| e.seq > since).collect(),
            self.dropped,
        )
    }
}

/// Behaviour of the debug capture — an escalation ladder from "send nothing" to "actively
/// query". **None** of the modes ever changes panel state (no arm/disarm/bypass/switching);
/// they differ only in HOW MUCH the device sends to the (foreign) panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Sends NOTHING — only records what the panel sends on its own. Maximum
    /// safety on an unknown panel.
    Listen,
    /// Sends only the FT1.2 acknowledgement (ACK) so the panel sees the link as healthy
    /// and reveals more — but no commands whatsoever.
    ListenAck,
    /// Additionally runs the occupied/text scan (read-only queries 0x10/0x0C/0x54) →
    /// maps addresses and names of the foreign panel. No switching.
    Discover,
}

impl CaptureMode {
    /// Parse from the wire string of the `/debug/capture/start` route (not `FromStr` to
    /// avoid trait confusion — deliberately fallible/`Option`).
    pub fn parse(s: &str) -> Option<CaptureMode> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "listen" => CaptureMode::Listen,
            "listen_ack" | "ack" => CaptureMode::ListenAck,
            "discover" => CaptureMode::Discover,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureMode::Listen => "listen",
            CaptureMode::ListenAck => "listen_ack",
            CaptureMode::Discover => "discover",
        }
    }
    /// Human-readable read/write declaration for the UI — answers "is this reading or writing
    /// to my panel?".
    pub fn sends_desc(self) -> &'static str {
        match self {
            CaptureMode::Listen => "liest nur — sendet nichts an die Anlage",
            CaptureMode::ListenAck => "liest — sendet nur FT1.2-Quittungen (kein Schalten)",
            CaptureMode::Discover => "liest — sendet Lese-Abfragen (Belegt/Text), kein Schalten",
        }
    }
    /// Does this mode suppress all transmissions to the panel? (Only `Listen` does.)
    pub fn suppresses_tx(self) -> bool {
        matches!(self, CaptureMode::Listen)
    }
}

/// Raw capture of the received GMS byte stream for diagnosing foreign panels. The buffer is
/// the **exact replay format** (`telenot-sim replay`): raw RX byte stream without headers. A
/// separate [`FrameDecoder`] counts valid/invalid frames and record types in parallel — as a
/// live trust indicator ("GMS data is arriving") without burdening the hot loop.
pub struct CaptureState {
    pub active: bool,
    pub mode: CaptureMode,
    /// Ring of the LAST `cap` received bytes (the decoder re-syncs itself during replay).
    buf: Vec<u8>,
    cap: usize,
    pub bytes_total: u64,
    pub started_ms: u64,
    /// Stop timestamp — freezes the displayed elapsed time (`None` = still running).
    pub ended_ms: Option<u64>,
    // Frame statistics alongside recording (monotonically increasing over the whole session, not just the ring).
    stat_dec: telenot_protocol::FrameDecoder,
    pub frames_ok: u32,
    pub frames_err: u32,
    pub rec_types: BTreeMap<u8, u32>,
}

impl Default for CaptureState {
    fn default() -> Self {
        CaptureState {
            active: false,
            mode: CaptureMode::Listen,
            buf: Vec::new(),
            // 32 KB = generous for a 2–3 min capture (~7 KB/arm cycle reference),
            // but only occupied during an active debug session (ESP32 heap budget).
            cap: 32 * 1024,
            bytes_total: 0,
            started_ms: 0,
            ended_ms: None,
            stat_dec: telenot_protocol::FrameDecoder::new(),
            frames_ok: 0,
            frames_err: 0,
            rec_types: BTreeMap::new(),
        }
    }
}

impl CaptureState {
    /// Starts a fresh capture in the given mode (discards any previous capture).
    pub fn start(&mut self, mode: CaptureMode, now: u64) {
        self.active = true;
        self.mode = mode;
        self.buf.clear();
        self.bytes_total = 0;
        self.started_ms = now;
        self.ended_ms = None;
        self.stat_dec = telenot_protocol::FrameDecoder::new();
        self.frames_ok = 0;
        self.frames_err = 0;
        self.rec_types.clear();
    }

    /// Stops the capture (buffer is kept for download). Freezes the elapsed time at `now`.
    pub fn stop(&mut self, now: u64) {
        self.active = false;
        self.ended_ms = Some(now);
    }

    /// Displayed elapsed time in seconds: counts up to `now` while active, frozen after stop,
    /// and **0 if never started** (otherwise idle state would show the boot uptime, because
    /// `started_ms` would be 0).
    pub fn elapsed_s(&self, now: u64) -> u64 {
        if self.active {
            now.saturating_sub(self.started_ms) / 1000
        } else if let Some(end) = self.ended_ms {
            end.saturating_sub(self.started_ms) / 1000
        } else {
            0
        }
    }

    /// Record raw RX bytes (no-op if inactive). Keeps the ring at `cap` and counts
    /// frames/record types alongside. Called from the serial loop with each chunk.
    pub fn record(&mut self, chunk: &[u8]) {
        if !self.active || chunk.is_empty() {
            return;
        }
        self.bytes_total += chunk.len() as u64;
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > self.cap {
            let drop = self.buf.len() - self.cap;
            self.buf.drain(0..drop);
        }
        self.stat_dec.feed(chunk);
        while let Some(ev) = self.stat_dec.next_frame() {
            match ev {
                Ok(frame) => {
                    self.frames_ok += 1;
                    for rec in frame.records().filter_map(|r| r.ok()) {
                        *self.rec_types.entry(rec.satztyp).or_insert(0) += 1;
                    }
                }
                Err(_) => self.frames_err += 1,
            }
        }
    }

    /// Current buffer contents (for the `capture.bin` download).
    pub fn bytes(&self) -> Vec<u8> {
        self.buf.clone()
    }

    pub fn buf_used(&self) -> usize {
        self.buf.len()
    }
    pub fn cap(&self) -> usize {
        self.cap
    }
}

/// HTTP-side setup session (sensor edits + settings).
#[derive(Debug, Default)]
pub struct SetupState {
    /// Active edit session (lazy: `None` = no edits yet, reads serve from
    /// [`App::persisted`]; materialized copy-on-write via [`App::edit`]).
    pub session: Option<EditSession>,
    /// Has the sensor list already been populated (from a scan or --config)?
    pub seeded: bool,
    pub mqtt: MqttSettings,
    pub conn: ConnSettings,
    /// Panel selection (kind/GMS variant/command gate/area names) — part of the
    /// persisted config; carried through seed/commit so a sensor commit can never
    /// reset it.
    pub panel: telenot_config::PanelSettings,
    pub remote_disarm: bool,
    pub mqtt_tested: bool,
}

impl SetupState {
    /// Populate the sensor session from a (discovery or file) config and raw names.
    /// Moves the sensor table into the session (no copy).
    pub fn seed(&mut self, config: Config, raw_names: BTreeMap<u16, String>) {
        self.panel = config.panel.clone();
        self.session = Some(EditSession::from_scan(config, raw_names));
        self.seeded = true;
    }

    /// Populate the sensor session from the PERSISTED config (boot/import). Unlike
    /// [`Self::seed`], there is no auto-hiding of generic bus slots: the persisted config
    /// contains only intentionally included entries — re-hiding would, for example,
    /// suppress enabled keypad panic buttons after every reboot. `seeded` stays untouched
    /// (its meaning remains "a scan result was taken over").
    pub fn seed_persisted(&mut self, config: Config) {
        self.panel = config.panel.clone();
        let n = config.sensors.len();
        self.session = Some(EditSession {
            table: config.sensors,
            excluded: vec![false; n],
            raw_names: BTreeMap::new(),
            dup_of: BTreeMap::new(),
        });
    }
}

/// Complete HTTP-side state. Held behind `Arc<Mutex<App>>` by HTTP threads and the serial
/// owner (briefly).
pub struct App {
    pub device: DeviceInfo,
    pub setup: SetupState,
    /// Currently active/persisted config — the SAME `Arc` the core holds (steady state:
    /// ONE sensor table). Reads serve from here until an edit session materializes.
    pub persisted: Arc<Config>,
    /// Live state mirrored from the serial owner.
    pub live: LiveSnapshot,
    pub mqtt_test: MqttTestView,
    pub conn_check: ConnCheckView,
    pub ring: RingLog,
    /// Raw GMS byte-stream capture (debugging foreign panels). Filled by the serial loop,
    /// read by the `/debug/capture` routes.
    pub capture: CaptureState,
    /// Produced by handlers, drained and routed by the daemon.
    pub intents: Vec<Intent>,
    pub services: Box<dyn Services>,
    pub session: Option<String>,
    pub csrf: Option<String>,
    /// Actual MQTT connection state (mirrored from the sink). Replaces the plain "tested"
    /// flag in diagnostics.
    pub mqtt_connected: bool,
    /// Firmware heap in bytes `(free, largest_free_block, low_watermark)`. `None` on the host.
    pub heap: Option<(u64, u64, u64)>,
    /// Reset reason of the last boot (esp_reset_reason, e.g. "poweron"/"panic"/"task_wdt").
    pub boot_reason: &'static str,
    /// Boot count since first commissioning (NVS counter) — makes silent reboot loops visible.
    pub boot_count: u32,
    /// Latch: heap dropped below the warning threshold (once per boot, for diagnostics/HA alert).
    pub heap_low: bool,
    /// HomeKit pairing info (setup code + QR payload), set by the firmware once HAP is running.
    pub homekit_pair: Option<crate::dto::HomekitPair>,
    /// Remaining setup-window time (HTTP access) in seconds. Mirrored by the firmware loop
    /// each iteration; `None` on the host sim (no time-limited window).
    pub setup_window_s_remaining: Option<u32>,
    /// Request to reconcile the HomeKit detector set live (read and cleared by the HAP thread).
    pub homekit_reconcile: bool,
    /// OTA upload state (maintained by the streaming handler, read by `GET /ota`).
    pub ota: crate::ota::OtaState,
    /// Slot overview `(running, boot)` — set by the firmware at startup, mocked by the sim.
    pub ota_slots: Option<(crate::ota::SlotInfo, crate::ota::SlotInfo)>,
    /// Is this firmware still in the self-test phase (PENDING_VERIFY, rollback possible)?
    pub ota_pending_verify: bool,
    /// Remaining self-test time in seconds (mirrored by the loop, like setup_window).
    pub ota_self_test_s: Option<u32>,
    /// Daily update check (outbound-only, GitHub release manifest). Installation is
    /// always manual — the check only shows "version X available".
    pub update_check: bool,
    /// Newer version found by the check (`None` = none known / check disabled).
    pub ota_latest: Option<String>,
}

impl App {
    pub fn new(device: DeviceInfo, services: Box<dyn Services>) -> Self {
        App {
            device,
            setup: SetupState::default(),
            persisted: Arc::new(Config::default()),
            live: LiveSnapshot::default(),
            mqtt_test: MqttTestView::default(),
            conn_check: ConnCheckView::default(),
            // 256 × ≤250 B ≈ 28 KB worst-case (previously 512 unbounded ≈ 55 KB+) —
            // sufficient as a diagnostic history while keeping the heap budget bounded.
            ring: RingLog::new(256),
            capture: CaptureState::default(),
            intents: Vec::new(),
            services,
            session: None,
            csrf: None,
            mqtt_connected: false,
            heap: None,
            boot_reason: "unknown",
            boot_count: 0,
            heap_low: false,
            homekit_pair: None,
            setup_window_s_remaining: None,
            homekit_reconcile: false,
            ota: crate::ota::OtaState::default(),
            ota_slots: None,
            ota_pending_verify: false,
            ota_self_test_s: None,
            update_check: true,
            ota_latest: None,
        }
    }

    /// Sensor count of the current view (edit session if present, else persisted config).
    pub fn sensors_len(&self) -> usize {
        match &self.setup.session {
            Some(s) => s.len(),
            None => self.persisted.sensors.len(),
        }
    }

    /// One page of the sensor view. Without a session the persisted config is served
    /// read-only (nothing excluded, no raw names/dup info).
    pub fn sensor_page(&self, offset: usize, limit: usize) -> Vec<ApiSensor> {
        match &self.setup.session {
            Some(sess) => (offset..(offset + limit).min(sess.len()))
                .map(|i| sess.api_at(i))
                .collect(),
            None => self
                .persisted
                .sensors
                .iter()
                .skip(offset)
                .take(limit)
                .map(|s| ApiSensor {
                    address: s.address(),
                    name: s.name().to_string(),
                    name_ha: s.name_ha().to_string(),
                    kind: s.kind(),
                    topic: s.topic().to_string(),
                    location: s.location().to_string(),
                    polarity: s.polarity(),
                    confirmed: s.confirmed(),
                    switchable: s.switchable(),
                    show_in_homekit: s.show_in_homekit(),
                    raw_name: s.name().to_string(),
                    dup_of: None,
                    status: if s.confirmed() {
                        SensorStatus::Confirmed
                    } else {
                        SensorStatus::Unconfirmed
                    },
                    include: true,
                })
                .collect(),
        }
    }

    pub fn sensor_counts(&self) -> Counts {
        match &self.setup.session {
            Some(s) => s.counts(),
            None => {
                let confirmed = self
                    .persisted
                    .sensors
                    .iter()
                    .filter(|s| s.confirmed())
                    .count();
                Counts {
                    confirmed,
                    unconfirmed: self.persisted.sensors.len() - confirmed,
                    excluded: 0,
                }
            }
        }
    }

    pub fn sensor_rooms(&self) -> Vec<String> {
        match &self.setup.session {
            Some(s) => s.rooms(),
            None => {
                let mut set: Vec<String> = self
                    .persisted
                    .sensors
                    .iter()
                    .map(|s| s.location().to_string())
                    .filter(|l| !l.trim().is_empty())
                    .collect();
                set.sort();
                set.dedup();
                set
            }
        }
    }

    /// Edit session for mutations — materialized copy-on-write from the persisted
    /// config on first use (`setup.panel` stays as it is).
    pub fn edit(&mut self) -> &mut EditSession {
        if self.setup.session.is_none() {
            self.setup.session = Some(EditSession::from_persisted(&self.persisted));
        }
        self.setup.session.as_mut().expect("just materialized")
    }

    /// Validation of the working state: session (included inventory + setup panel) or,
    /// without one, the persisted config. Infallible — no second table is built.
    pub fn working_issues(&self) -> Vec<telenot_config::Issue> {
        match &self.setup.session {
            Some(s) => s.issues(&self.setup.panel),
            None => self.persisted.validate(),
        }
    }

    /// Consumes the session into the commit config (excluded sensors dropped in place).
    /// Without a session: clone of the persisted config with the (possibly edited)
    /// setup panel applied — settings steps may have changed it.
    pub fn take_commit_config(&mut self) -> Config {
        match self.setup.session.take() {
            Some(sess) => sess.into_config(self.setup.panel.clone()),
            None => {
                let mut cfg = (*self.persisted).clone();
                cfg.panel = self.setup.panel.clone();
                cfg
            }
        }
    }

    /// NON-consuming working config (export/HomeKit apply): builds a fresh table from
    /// the included session entries (one transient `Sensor` at a time). Fails with
    /// [`telenot_config::ConfigError::TooLarge`] if the inventory does not fit.
    pub fn working_config(&self) -> Result<Config, telenot_config::ConfigError> {
        match &self.setup.session {
            Some(sess) => {
                let mut table = SensorTable::new();
                for (s, _) in sess.table.iter().zip(&sess.excluded).filter(|(_, &ex)| !ex) {
                    table.push(&s.to_sensor())?;
                }
                Ok(Config {
                    schema_version: CURRENT_SCHEMA_VERSION,
                    sensors: table,
                    panel: self.setup.panel.clone(),
                })
            }
            None => {
                let mut cfg = (*self.persisted).clone();
                cfg.panel = self.setup.panel.clone();
                Ok(cfg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telenot_config::{Polarity, Sensor, SensorKind};

    fn sensor(address: u16, name: &str) -> Sensor {
        Sensor {
            address,
            name: name.into(),
            name_ha: name.into(),
            kind: SensorKind::Magnetkontakt,
            topic: format!("t_{address}"),
            location: String::new(),
            polarity: Polarity::ActiveLow,
            confirmed: true,
            switchable: false,
            show_in_homekit: false,
        }
    }

    fn cfg(sensors: Vec<Sensor>) -> Config {
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: telenot_config::SensorTable::from_sensors(&sensors).unwrap(),
            panel: Default::default(),
        }
    }

    #[test]
    fn ringlog_caps_entries_and_counts_dropped() {
        let mut r = RingLog::new(256);
        for i in 0..300 {
            r.push(i, "info", format!("msg {i}"));
        }
        let (lines, dropped) = r.since(0);
        assert_eq!(lines.len(), 256, "capacity capped");
        assert_eq!(dropped, 44, "dropped entries counted");
        assert_eq!(lines[0].msg, "msg 44", "oldest surviving entry");
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn capture_mode_parsing_and_tx_policy() {
        assert_eq!(CaptureMode::parse("listen"), Some(CaptureMode::Listen));
        assert_eq!(CaptureMode::parse("ACK"), Some(CaptureMode::ListenAck));
        assert_eq!(CaptureMode::parse("discover"), Some(CaptureMode::Discover));
        assert_eq!(CaptureMode::parse("nope"), None);
        // Only `Listen` suppresses all transmissions.
        assert!(CaptureMode::Listen.suppresses_tx());
        assert!(!CaptureMode::ListenAck.suppresses_tx());
        assert!(!CaptureMode::Discover.suppresses_tx());
    }

    #[test]
    fn capture_records_only_when_active_and_counts_frames() {
        // Real 0x24 block-status frame (identical to the runtime fixtures).
        let frame = hex("6846466873023a2400050002ffffffffffffde9e9e9e9e9e9e9efffdffffffffffffffffffffffffffff7fffffffffffffffffffffffffffffffffffffffffffffff0656999999ffffff7e16");
        let mut c = CaptureState::default();

        // Inactive: nothing is recorded.
        c.record(&frame);
        assert_eq!(c.bytes_total, 0);
        assert_eq!(c.buf_used(), 0);

        c.start(CaptureMode::Listen, 1_000);
        c.record(&frame);
        assert_eq!(c.bytes_total, frame.len() as u64);
        assert_eq!(c.buf_used(), frame.len());
        assert_eq!(c.frames_ok, 1, "valid frame counted");
        assert_eq!(c.frames_err, 0);
        assert!(
            c.rec_types.contains_key(&0x24),
            "block-status record type recognised: {:?}",
            c.rec_types
        );

        // The buffer is exactly the replay format (raw RX byte stream).
        assert_eq!(c.bytes(), frame, "download buffer = raw bytes");

        c.stop(5_000);
        assert!(!c.active);
        c.record(&frame); // no growth after stop
        assert_eq!(c.bytes_total, frame.len() as u64);
    }

    #[test]
    fn capture_elapsed_freezes_after_stop() {
        let mut c = CaptureState::default();
        // Idle (never started): 0 — NOT the boot uptime.
        assert_eq!(c.elapsed_s(476_000), 0, "idle shows 0, not the uptime");
        c.start(CaptureMode::Listen, 1_000);
        assert_eq!(c.elapsed_s(4_000), 3, "counts while active (now − start)");
        c.stop(6_000);
        // After stop the elapsed time is frozen — a later `now` no longer changes it.
        assert_eq!(c.elapsed_s(9_999), 5, "frozen at stop − start");
        assert_eq!(c.elapsed_s(50_000), 5, "stays frozen");
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)] // Test intentionally shrinks only the cap.
    fn capture_ring_keeps_last_bytes_under_cap() {
        let mut c = CaptureState::default();
        c.cap = 64; // small cap for the test
        c.start(CaptureMode::Listen, 0);
        let chunk = vec![0xABu8; 50];
        c.record(&chunk);
        c.record(&chunk); // 100 bytes > 64 → ring keeps the last 64
        assert_eq!(c.bytes_total, 100, "total counter is monotonic");
        assert_eq!(c.buf_used(), 64, "ring capped at cap");
    }

    #[test]
    fn ringlog_truncates_long_messages_on_char_boundary() {
        let mut r = RingLog::new(8);
        // 300 × 'ü' (2 bytes each) — the 200-byte boundary falls mid-character.
        r.push(0, "warn", "ü".repeat(300));
        let (lines, _) = r.since(0);
        let msg = &lines[0].msg;
        assert!(msg.ends_with('…'), "truncation marked");
        assert!(
            msg.len() <= 200 + '…'.len_utf8(),
            "budget bounded: {} B",
            msg.len()
        );
        // Must remain valid UTF-8 (no panic on slicing) — the String type guarantees this,
        // the test ensures we actually reach this point.
        assert!(msg.chars().all(|c| c == 'ü' || c == '…'));
    }

    #[test]
    fn ringlog_since_filters_by_sequence() {
        let mut r = RingLog::new(8);
        for i in 0..5 {
            r.push(i, "info", format!("m{i}"));
        }
        let (all, _) = r.since(0);
        let last_seq = all.last().unwrap().seq;
        let (tail, _) = r.since(last_seq - 1);
        assert_eq!(tail.len(), 1, "only entries AFTER since_seq");
        assert_eq!(tail[0].seq, last_seq);
    }

    #[test]
    fn seed_hides_generic_bus_slots_but_seed_persisted_does_not() {
        // The keypad-panic-button reboot bug: seed() hides generic bus slots
        // ("BT-Adr…") — but the PERSISTED config is a deliberate selection and must
        // not be re-hidden on the boot seed.
        let bt = sensor(0x00B7, "BT-Adr.1 - Z BT Freip. Taste");
        let mk = sensor(0x0042, "MK Haustür");

        let mut setup = SetupState::default();
        setup.seed(cfg(vec![bt.clone(), mk.clone()]), BTreeMap::new());
        let sess = setup.session.as_ref().expect("seed creates a session");
        let idx = sess.find_idx(0x00B7).expect("BT slot present");
        assert!(sess.excluded[idx], "discovery seed hides BT slots");

        let mut setup = SetupState::default();
        setup.seed_persisted(cfg(vec![bt, mk]));
        let sess = setup
            .session
            .as_ref()
            .expect("seed_persisted creates a session");
        assert!(
            sess.excluded.iter().all(|&ex| !ex),
            "boot seed respects the persisted selection (nothing hidden)"
        );
    }
}

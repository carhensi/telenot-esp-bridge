//! Sans-IO "brain" of the serial side: owns `Core` + `FrameDecoder` + `Discovery` and
//! drives the scan and the polarity observation window. Receives raw bytes/ticks/intents,
//! produces [`Action`]s (executed by the daemon/firmware), and publishes a [`LiveSnapshot`]
//! that HTTP handlers READ ONLY. No I/O here.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use telenot_config::{Config, Polarity};
use telenot_core::{decode_panel_text, Action, Availability, Core, CoreOptions, Discovery, Tick};
use telenot_protocol::{encode_belegt_query, encode_text_query, Frame, FrameDecoder, Function};

use crate::dto::ScanPhase;
use crate::mqtt::MqttTarget;
use crate::polarity::PolarityObservation;

/// Duration of a polarity observation window.
const POLARITY_WINDOW_MS: u64 = 60_000;
/// Timeout before an unanswered discovery query is retransmitted.
const SCAN_RETRY_MS: u64 = 4_000;
/// Initial estimate per address still to query (used until the adaptive rate kicks in). Real
/// measured ~6.5 s/address (panel poll cycle + timeouts for unnamed addresses).
const SCAN_SECS_PER_ADDR: u64 = 6;

/// Inputs from the HTTP path to the serial/worker side. Drained and routed by the daemon:
/// scan/polarity/reload → [`Runtime`]; `TestMqtt` is executed by the daemon in a worker.
#[derive(Debug, Clone)]
pub enum Intent {
    StartScan,
    CancelScan {
        keep_partial: bool,
    },
    ObservePolarity {
        address: u16,
    },
    TestMqtt(MqttTarget),
    ReloadConfig(Arc<Config>),
    /// Control command (arm/disarm/bypass) from the live test board. PIN gate and
    /// execution are handled by the daemon (security-reducing = fail-closed), not the runtime.
    Command {
        cmd: crate::BridgeCommand,
        pin: Option<String>,
    },
    /// Control command from **HomeKit** (paired device = authorization token, no per-action
    /// PIN). Authorized firmware-side via [`crate::homekit_authorized`] (disarm only when
    /// setup-enabled). Separate variant so the HomeKit path never shares the PIN/remote-disarm
    /// path.
    HomekitCommand {
        cmd: crate::BridgeCommand,
    },
    /// Start HAP live (HomeKit enabled in setup). Firmware-side via `homekit::start`
    /// (idempotent); the host sim ignores it.
    StartHomekit,
    /// Real TCP probe of the configured connection target (daemon worker).
    CheckConnection {
        ip: String,
        port: u16,
    },
    /// Reboot the device (e.g. to apply an MQTT↔HomeKit mode switch). Firmware-side via
    /// `esp_restart`; the host sim ignores it.
    Reboot,
    /// Extend the setup window (HTTP access) by another window — like device button BUT1,
    /// but from the web interface. Firmware-side in `app_loop` (with an absolute cap since
    /// boot); the host sim ignores it.
    ExtendSetupWindow,
    /// Apply the HomeKit detector set LIVE to the current config (pending → applied, no reboot).
    /// Firmware-side, signals the HAP thread to reconcile bridged accessories (add/remove →
    /// SDK bumps c# automatically); the host sim ignores it.
    ApplyHomekit,
    /// Start a debug capture (diagnosing foreign panels). Handled by the serial loop (which
    /// owns the `App::capture` buffer + TX suppression); `Discover` additionally triggers a scan.
    CaptureStart {
        mode: crate::app::CaptureMode,
    },
    /// Stop the debug capture (buffer is kept for download).
    CaptureStop,
}

/// Scan progress (metadata without feed).
#[derive(Debug, Clone)]
pub struct ScanMeta {
    pub phase: ScanPhase,
    pub total: usize,
    /// Addresses already queried (progress bar). Unnamed occupied addresses also count —
    /// so the bar advances even when no name is found.
    pub scanned: usize,
    pub named: usize,
    pub elapsed: u64,
    pub remaining: u64,
    pub current: Option<u16>,
}

impl Default for ScanMeta {
    fn default() -> Self {
        ScanMeta {
            phase: ScanPhase::Idle,
            total: 0,
            scanned: 0,
            named: 0,
            elapsed: 0,
            remaining: 0,
            current: None,
        }
    }
}

/// Live state published by the serial owner. HTTP handlers read this only.
/// `&'static str` instead of String: the snapshot is mirrored every 80 ms —
/// two persistent allocations per tick would fragment the ESP32 heap over time.
#[derive(Debug, Clone)]
pub struct LiveSnapshot {
    pub arm_state: &'static str,
    pub availability: &'static str,
    pub last_frame_ms_ago: Option<u64>,
    pub intern_ready: Option<bool>,
    pub extern_ready: Option<bool>,
    pub sensor_states: Vec<(u16, bool)>,
    pub scan: ScanMeta,
    /// Current discovery state (for the live feed in S3): config + raw names.
    /// Arc instead of clone: the snapshot is mirrored at the poll rate (~80 ms) — rebuilding
    /// the config every iteration fragmented the heap (crash late in a scan, OOM).
    pub discovered: Option<std::sync::Arc<(Config, BTreeMap<u16, String>)>>,
    /// Result of the last polarity observation window.
    pub polarity_result: Option<(u16, Polarity)>,
    /// Last command result (`command_result` payload, age in ms) — so the live test board
    /// can show "executed/denied/timeout" rather than just "sent".
    pub command_result: Option<(String, u64)>,
}

impl Default for LiveSnapshot {
    fn default() -> Self {
        LiveSnapshot {
            arm_state: "unknown",
            availability: "online",
            last_frame_ms_ago: None,
            intern_ready: None,
            extern_ready: None,
            sensor_states: Vec::new(),
            scan: ScanMeta::default(),
            discovered: None,
            polarity_result: None,
            command_result: None,
        }
    }
}

/// `ScanView` is an alias for scan metadata (re-export convenience).
pub type ScanView = ScanMeta;

pub struct Runtime {
    core: Core,
    decoder: FrameDecoder,
    discovery: Discovery,
    disarm_enabled: bool,

    // Scan-Pacing
    scanning: bool,
    scan_done: bool,
    scan_started_ms: u64,
    text_queued: bool,
    queries: VecDeque<u16>,
    awaiting_since_ms: Option<u64>,
    current: Option<u16>,
    scan_result: Option<(Config, BTreeMap<u16, String>)>,

    // Liveness / Zeit
    last_now: Tick,
    last_frame_ms: Option<Tick>,

    // Polarity
    obs: Option<PolarityObservation>,
    last_polarity_result: Option<(u16, Polarity)>,

    // Last command result (payload + timestamp), mirrored from command_result actions.
    last_command_result: Option<(String, Tick)>,

    // Cached discovery state for the snapshot: rebuilt ONLY when (named, pending) changes —
    // never on every 80 ms poll tick (heap fragmentation → scan crash).
    discovered_cache: Option<std::sync::Arc<(Config, BTreeMap<u16, String>)>>,
    discovered_rev: (usize, usize),
    // Final counts (total, named) after the discovery is released — for the scan display.
    scan_final: Option<(usize, usize)>,
}

impl Runtime {
    pub fn new(config: Arc<Config>, disarm_enabled: bool) -> Self {
        Runtime {
            core: Core::new(config, CoreOptions { disarm_enabled }),
            decoder: FrameDecoder::new(),
            discovery: Discovery::new(),
            disarm_enabled,
            scanning: false,
            scan_done: false,
            scan_started_ms: 0,
            text_queued: false,
            queries: VecDeque::new(),
            awaiting_since_ms: None,
            current: None,
            scan_result: None,
            last_now: 0,
            last_frame_ms: None,
            obs: None,
            last_polarity_result: None,
            last_command_result: None,
            discovered_cache: None,
            discovered_rev: (usize::MAX, usize::MAX),
            scan_final: None,
        }
    }

    /// Mirror `command_result` publishes into the snapshot state (non-invasive: actions
    /// continue unchanged towards MQTT).
    fn note_command_results(&mut self, now: Tick, actions: &[Action]) {
        for a in actions {
            if let Action::Publish { topic, payload, .. } = a {
                if topic == "command_result" {
                    self.last_command_result = Some((payload.clone(), now));
                }
            }
        }
    }

    /// Make a rejection BEFORE the core (PIN/authorization gate) visible in the snapshot.
    pub fn note_command_denied(&mut self, now: Tick, reason: &str) {
        self.last_command_result = Some((format!("DENIED {reason}"), now));
    }

    /// Process raw bytes from the EMA side → actions to execute.
    pub fn feed(&mut self, now: Tick, bytes: &[u8]) -> Vec<Action> {
        self.last_now = now;
        let mut actions = Vec::new();
        self.decoder.feed(bytes);
        while let Some(ev) = self.decoder.next_frame() {
            match ev {
                Ok(frame) => {
                    self.last_frame_ms = Some(now);
                    self.observe_polarity(&frame, now, &mut actions);
                    if self.scanning {
                        self.ingest_discover(&frame, &mut actions);
                    }
                    // FT1.2 transmit discipline: the slave may ONLY send when the master polls
                    // with SEND_NORM. During a scan, place the discovery query in exactly that
                    // window (it replaces the ACK there) — otherwise the panel won't respond.
                    // No poll / nothing to send → normal ACK/processing via the core.
                    let is_poll = matches!(frame.function(), Some(Function::SendNorm));
                    if self.scanning && is_poll && self.advance_scan(now, &mut actions) {
                        // Query occupies the transmit window — no additional ACK.
                    } else {
                        for a in self.core.on_frame(now, &frame) {
                            actions.push(a);
                        }
                    }
                }
                Err(e) => actions.push(Action::Log(format!("FRAME-ERR {e:?}"))),
            }
        }
        self.note_command_results(now, &actions);
        actions
    }

    /// Periodic tick (liveness deadline + window expiry).
    pub fn tick(&mut self, now: Tick) -> Vec<Action> {
        self.last_now = now;
        let mut actions = self.core.on_tick(now);
        if let Some(obs) = &self.obs {
            if obs.expired(now) {
                actions.push(Action::Log(format!(
                    "Polaritäts-Fenster 0x{:04X} ohne Auslösung abgelaufen",
                    obs.address
                )));
                self.obs = None;
            }
        }
        self.note_command_results(now, &actions);
        actions
    }

    /// Execute an authorized command (unused during setup; for the live path).
    pub fn on_command(&mut self, now: Tick, cmd: crate::BridgeCommand) -> Vec<Action> {
        let actions = match cmd {
            crate::BridgeCommand::Arm(arm) => self.core.on_command(now, arm),
            crate::BridgeCommand::ArmArea { cmd, area } => {
                self.core.on_command_area(now, cmd, area)
            }
            crate::BridgeCommand::Bypass { mb, sperren } => self.core.on_bypass(now, mb, sperren),
            crate::BridgeCommand::Output { addr, on } => self.core.on_output(now, addr, on),
        };
        self.note_command_results(now, &actions);
        actions
    }

    /// Set the panel's date/time (GMS record type 0x50). The I/O layer supplies **local**
    /// time (weekday Mon=0…Sun=6, for the panel's DST scheme) — useful after SNTP sync and
    /// after `event=restart`. Yields to active commands.
    pub fn set_panel_time(&mut self, now: Tick, dt: telenot_protocol::DateTime) -> Vec<Action> {
        self.core.on_set_time(now, dt)
    }

    /// Reload the config AND refresh the core master switch `disarm_enabled` (e.g. when
    /// remote disarm was enabled during setup). Keeps the core's own fail-closed gate
    /// (telenot-core) consistent with the setup setting.
    pub fn reload(&mut self, cfg: Arc<Config>, disarm_enabled: bool) {
        self.disarm_enabled = disarm_enabled;
        self.core = Core::new(cfg, CoreOptions { disarm_enabled });
    }

    /// Apply an intent (scan/polarity/reload). `TestMqtt` is ignored by the runtime.
    pub fn apply_intent(&mut self, now: Tick, intent: Intent) {
        match intent {
            Intent::StartScan => {
                self.discovery = Discovery::new();
                self.scanning = true;
                self.scan_done = false;
                self.scan_started_ms = now;
                self.text_queued = false;
                self.queries.clear();
                self.awaiting_since_ms = None;
                self.current = None;
                self.scan_result = None;
                self.scan_final = None;
                self.discovered_cache = None;
                self.discovered_rev = (usize::MAX, usize::MAX);
            }
            Intent::CancelScan { keep_partial } => {
                if keep_partial {
                    self.scan_done = true;
                    self.scan_result = Some((
                        self.discovery
                            .into_config_for(self.core.profile(), &self.core.config().panel),
                        self.discovery.raw_names().clone(),
                    ));
                } else {
                    self.discovery = Discovery::new();
                    self.scan_done = false;
                }
                self.scanning = false;
                self.current = None;
            }
            Intent::ObservePolarity { address } => {
                self.obs = Some(PolarityObservation::new(address, now, POLARITY_WINDOW_MS));
            }
            Intent::ReloadConfig(cfg) => {
                self.core = Core::new(
                    cfg,
                    CoreOptions {
                        disarm_enabled: self.disarm_enabled,
                    },
                );
            }
            Intent::TestMqtt(_) => { /* executed by the daemon in a worker */ }
            Intent::Command { .. } => { /* executed by the daemon with PIN gate */ }
            Intent::HomekitCommand { .. } => { /* authorized and executed firmware-side */ }
            Intent::StartHomekit => { /* firmware-side via homekit::start; host ignores */ }
            Intent::CheckConnection { .. } => { /* executed by the daemon in a worker */ }
            Intent::Reboot => { /* firmware-side in app_loop via esp_restart; host ignores */ }
            Intent::ExtendSetupWindow => { /* firmware-side in app_loop (capped); host ignores */ }
            Intent::ApplyHomekit => { /* firmware-side reconciled in the HAP thread; host ignores */
            }
            Intent::CaptureStart { .. } | Intent::CaptureStop => { /* handled in the serial loop
                 (App::capture + TX gate); Discover also starts a scan there */
            }
        }
    }

    /// Active config (borrowed from the core — don't keep a separate copy in app_loop).
    pub fn config(&self) -> &Config {
        self.core.config()
    }

    /// Toggle the remote-disarm gate live (without a config reload).
    pub fn set_disarm_enabled(&mut self, on: bool) {
        self.disarm_enabled = on;
        self.core.set_disarm_enabled(on);
    }

    /// Take the completed scan result exactly once (to seed the setup sensor list).
    /// Afterwards the discovery raw data (names, occupied set, cache — ~50 KB combined)
    /// is released immediately: the heap is needed right after for the large `/sensors`
    /// responses and the commit (OOM evidence in the field: 147-sensor scan ok,
    /// crash only when serialising the sensor list). Final counts are kept for the UI.
    pub fn take_scan_result(&mut self) -> Option<(Config, BTreeMap<u16, String>)> {
        let r = self.scan_result.take();
        if r.is_some() {
            self.scan_final = Some((
                self.discovery.occupied().len(),
                self.discovery.named_count(),
            ));
            self.discovery = Discovery::new();
            self.discovered_cache = None;
            self.queries.clear();
        }
        r
    }

    /// Publish the current live state.
    pub fn snapshot(&mut self) -> LiveSnapshot {
        let phase = if self.scan_done {
            ScanPhase::Done
        } else if !self.scanning {
            ScanPhase::Idle
        } else if !self.discovery.belegt_complete() {
            ScanPhase::Belegt
        } else {
            ScanPhase::Naming
        };
        let elapsed = if self.scanning || self.scan_done {
            self.last_now.saturating_sub(self.scan_started_ms) / 1000
        } else {
            0
        };
        // Total occupied addresses is only known after the occupied phase. After the
        // discovery is released (seed done), the numbers come from scan_final.
        let total = if let Some((t, _)) = self.scan_final {
            t
        } else if self.discovery.belegt_complete() {
            self.discovery.occupied().len()
        } else {
            0
        };
        // Addresses already queried (including unnamed) — for the progress bar + remaining-time projection.
        let scanned = total.saturating_sub(self.queries.len());
        let named = self
            .scan_final
            .map(|(_, n)| n)
            .unwrap_or_else(|| self.discovery.named_count());
        let discovered = if self.scan_final.is_some() {
            // Discovery released; sensor list now comes from GET /sensors.
            None
        } else if self.scanning || self.scan_done {
            // Rebuild only on progress — the snapshot runs at 80 ms, into_config()
            // allocates the full sensor list (~40 KB per call at ~150 detectors).
            let rev = (self.discovery.named_count(), self.queries.len());
            if self.discovered_cache.is_none() || self.discovered_rev != rev {
                self.discovered_cache = Some(std::sync::Arc::new((
                    self.discovery
                        .into_config_for(self.core.profile(), &self.core.config().panel),
                    self.discovery.raw_names().clone(),
                )));
                self.discovered_rev = rev;
            }
            self.discovered_cache.clone()
        } else {
            None
        };
        LiveSnapshot {
            arm_state: self.core.arm_state().as_str(),
            availability: match self.core.availability() {
                Availability::Online => "online",
                Availability::Unavailable => "offline",
            },
            last_frame_ms_ago: self.last_frame_ms.map(|t| self.last_now.saturating_sub(t)),
            intern_ready: self.core.intern_ready(),
            extern_ready: self.core.extern_ready(),
            sensor_states: self
                .core
                .sensor_states()
                .iter()
                .map(|(&a, &v)| (a, v))
                .collect(),
            scan: ScanMeta {
                phase,
                total,
                // Queried = occupied minus still-pending text queries (all at Done).
                scanned,
                named,
                elapsed,
                // Remaining time is SELF-CORRECTING: project from the real observed rate
                // (elapsed/scanned) once a few addresses are through — automatically
                // correct regardless of panel/transport. Before that (nothing queried yet),
                // use the constant default. The fixed 3 s/addr was ~2× too optimistic (real ~6.5 s/addr).
                remaining: {
                    let left = self.queries.len() as u64;
                    if scanned >= 3 && elapsed > 0 {
                        left * elapsed / scanned as u64
                    } else {
                        left * SCAN_SECS_PER_ADDR
                    }
                },
                current: self.current,
            },
            discovered,
            polarity_result: self.last_polarity_result,
            command_result: self
                .last_command_result
                .as_ref()
                .map(|(payload, at)| (payload.clone(), self.last_now.saturating_sub(*at))),
        }
    }

    // ── internal ──────────────────────────────────────────────

    fn observe_polarity(&mut self, frame: &Frame, now: Tick, actions: &mut Vec<Action>) {
        let Some(obs) = &mut self.obs else { return };
        for rec in frame.records().filter_map(|r| r.ok()) {
            if let Some(bs) = rec.as_block_status() {
                if matches!(bs.adresserweiterung, 0x01 | 0x02) {
                    if let Some(raw) = bs.raw_bit(obs.address) {
                        obs.observe(raw);
                    }
                }
            }
        }
        if obs.done() || obs.expired(now) {
            if let Some(p) = obs.result {
                self.last_polarity_result = Some((obs.address, p));
                actions.push(Action::Log(format!(
                    "Polarität 0x{:04X} beobachtet: {p:?}",
                    obs.address
                )));
            }
            self.obs = None;
        }
    }

    fn ingest_discover(&mut self, frame: &Frame, _actions: &mut Vec<Action>) {
        let mut got_response = false;

        for rec in frame.records().filter_map(|r| r.ok()) {
            if let Some(bs) = rec.as_block_status() {
                if matches!(bs.adresserweiterung, 0x71 | 0x72) {
                    self.discovery.ingest_belegt(&bs);
                    if self.discovery.belegt_complete() {
                        got_response = true;
                    }
                }
            }
        }

        // Text response: 0x0C carries the address, 0x54 the plain-text name.
        let addr = frame
            .records()
            .filter_map(|r| r.ok())
            .find_map(|r| r.as_bereich_meldebereich().map(|bm| bm.address()));
        let name = frame
            .records()
            .filter_map(|r| r.ok())
            .find_map(|r| r.as_ascii().map(decode_panel_text));
        if let (Some(a), Some(n)) = (addr, name) {
            self.discovery.ingest_text(a, &n);
            got_response = true;
        }

        // "No detection point here" — a valid, panel-documented response for an address
        // the 0x24 occupancy scan flagged occupied but that has no physical component.
        // Counts as a response so the scan advances immediately instead of waiting out
        // the full SCAN_RETRY_MS timeout on that address.
        if frame.records().filter_map(|r| r.ok()).any(|r| {
            r.as_fehler()
                .is_some_and(|f| f.fehlercode.is_not_occupied())
        }) {
            got_response = true;
        }

        // Once both occupied telegrams are in: queue text queries.
        if self.discovery.belegt_complete() && !self.text_queued {
            self.text_queued = true;
            for a in self.discovery.occupied() {
                self.queries.push_back(a);
            }
        }

        if got_response {
            self.awaiting_since_ms = None;
        }
    }

    /// Places ONE discovery query into the transmit window if one is due. Returns `true`
    /// if a frame was sent (the caller may then suppress the ACK for that poll).
    fn advance_scan(&mut self, now: Tick, actions: &mut Vec<Action>) -> bool {
        let timed_out = self
            .awaiting_since_ms
            .map(|t| now.saturating_sub(t) > SCAN_RETRY_MS)
            .unwrap_or(false);
        if self.awaiting_since_ms.is_some() && !timed_out {
            return false;
        }

        if !self.discovery.belegt_complete() {
            let mut b = [0u8; 32];
            if let Ok(n) = encode_belegt_query(&mut b) {
                actions.push(Action::SendFrame(b[..n].to_vec()));
                self.awaiting_since_ms = Some(now);
                return true;
            }
            false
        } else if let Some(addr) = self.queries.pop_front() {
            self.current = Some(addr);
            let mut t = [0u8; 32];
            if let Ok(k) = encode_text_query(addr, &mut t) {
                actions.push(Action::SendFrame(t[..k].to_vec()));
                self.awaiting_since_ms = Some(now);
                return true;
            }
            false
        } else if self.text_queued {
            self.scanning = false;
            self.scan_done = true;
            self.current = None;
            self.scan_result = Some((
                self.discovery
                    .into_config_for(self.core.profile(), &self.core.config().panel),
                self.discovery.raw_names().clone(),
            ));
            actions.push(Action::Log(format!(
                "Discovery fertig: {} benannte Sensoren",
                self.discovery.named_count()
            )));
            false
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BridgeCommand;
    use telenot_core::{Action, ArmCommand};

    // Real frame fixtures (identical to crates/telenot-core/tests/core.rs):
    // disarmed + intern-arm-ready; SEND_NORM poll; CONFIRM_ACK from the panel.
    const READY_DISARMED: &str = "6846466873023a2400050002ffffffffffffde9e9e9e9e9e9e9efffdffffffffffffffffffffffffffff7fffffffffffffffffffffffffffffffffffffffffffffff0656999999ffffff7e16";
    const SEND_NORM: &str = "6802026840024216";
    const CONF_ACK: &str = "6802026800020216";

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    fn rt() -> Runtime {
        Runtime::new(
            Arc::new(Config {
                schema_version: telenot_config::CURRENT_SCHEMA_VERSION,
                sensors: telenot_config::SensorTable::new(),
                panel: Default::default(),
            }),
            false,
        )
    }

    /// Raw frame with a single Fehler record (satztyp 0x11, payload `[geraet, code]`).
    fn fehler_frame(code: u8) -> Frame {
        use telenot_protocol::{encode_frame, satztyp};
        let user_data = [0x73u8, 0x02, 0x02, satztyp::FEHLER, 0x00, code];
        let mut out = [0u8; 32];
        let n = encode_frame(&user_data, &mut out).expect("frame");
        let mut d = FrameDecoder::new();
        d.feed(&out[..n]);
        d.next_frame().unwrap().unwrap()
    }

    #[test]
    fn ingest_discover_advances_past_not_occupied_response() {
        // A "not occupied" text-query response must count as a response — otherwise the
        // scan waits out the full SCAN_RETRY_MS on an address that already answered.
        let mut r = rt();
        r.scanning = true;
        r.awaiting_since_ms = Some(0);
        r.ingest_discover(&fehler_frame(0x19 /* NichtBelegt */), &mut Vec::new());
        assert!(
            r.awaiting_since_ms.is_none(),
            "not-occupied response clears the await, scan advances immediately"
        );
    }

    #[test]
    fn scan_result_taken_once_then_discovery_freed() {
        // After the seed the discovery is released (heap!), but the final counts and
        // Done phase are preserved for the UI (scan_final).
        let mut r = rt();
        r.apply_intent(0, Intent::StartScan);
        assert_eq!(r.snapshot().scan.phase, ScanPhase::Belegt);

        r.apply_intent(10, Intent::CancelScan { keep_partial: true });
        let s = r.snapshot();
        assert_eq!(s.scan.phase, ScanPhase::Done);
        assert!(s.discovered.is_some(), "feed source present until seed");

        assert!(r.take_scan_result().is_some(), "result exactly once");
        assert!(r.take_scan_result().is_none(), "second take is empty");

        let s = r.snapshot();
        assert_eq!(s.scan.phase, ScanPhase::Done, "phase stays Done");
        assert!(
            s.discovered.is_none(),
            "discovery released after seed (no ~50 KB ballast)"
        );
    }

    #[test]
    fn denied_commands_surface_in_snapshot() {
        let mut r = rt();
        r.tick(1_000);
        r.note_command_denied(1_000, "Remote-Disarm nicht aktiviert");
        r.tick(4_000);
        let s = r.snapshot();
        let (text, ms_ago) = s.command_result.expect("result mirrored");
        assert_eq!(text, "DENIED Remote-Disarm nicht aktiviert");
        assert_eq!(ms_ago, 3_000, "age derived from last_now");
    }

    #[test]
    fn command_flows_through_poll_window_and_ack_into_snapshot() {
        let mut r = rt();
        // Establish readiness (intern ready) — otherwise the core rejects the arm.
        r.feed(0, &bytes(READY_DISARMED));
        let acts = r.on_command(10, BridgeCommand::Arm(ArmCommand::ArmHome));
        assert!(
            !acts.iter().any(|a| matches!(a, Action::SendFrame(_))),
            "queuing does not send yet"
        );
        // SEND_NORM poll: command occupies the transmit window.
        let acts = r.feed(20, &bytes(SEND_NORM));
        assert!(
            acts.iter().any(|a| matches!(a, Action::SendFrame(_))),
            "command goes out in the poll window"
        );
        // Panel ACK → OK lands in the snapshot for the live board.
        r.feed(30, &bytes(CONF_ACK));
        let s = r.snapshot();
        let (text, _) = s.command_result.expect("OK mirrored");
        assert!(text.starts_with("OK ArmHome"), "was: {text}");
    }
}

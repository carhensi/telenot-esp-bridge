//! Sans-IO core of the Telenot bridge — **no I/O, no hardware, no time source**.
//!
//! All security-critical logic lives here and is deterministically testable on the host:
//! feed events + a monotonic time (`now: Tick`) and receive [`Action`]s for the firmware to
//! execute. This makes the 3 s ACK loop, fail-safe deadlines, arm-state derivation,
//! armed_night reconciliation, and the command reducer verifiable without hardware.
//!
//! Design invariants:
//! - **Snapshot = truth:** arm state is derived EXCLUSIVELY from the cyclic block status
//!   (0x24), never optimistically from a sent command.
//! - **Fail-safe:** if the serial stream goes silent → `Unavailable`, never stale state.
//! - **Disarm default-off / fail-closed:** remote disarm only when explicitly enabled.

mod discovery;
pub mod profile;
pub use discovery::Discovery;
pub use profile::{PanelKind, PanelProfile, COMPLEX400};

use std::collections::BTreeMap;
use std::sync::Arc;
use telenot_config::Config;
use telenot_protocol::{
    encode_command_02, encode_conf_ack, encode_set_datetime, BlockStatus, DateTime, Frame,
    Function, MeldungsArt, ART_AUSGANG_AUS, ART_AUSGANG_EIN, ART_EXTERN_SCHARF, ART_INTERN_SCHARF,
    ART_MB_ENTSPERREN, ART_MB_SPERREN, ART_RUECKSETZEN, ART_UNSCHARF, ERW_AUSGAENGE,
};

/// Monotonic time in milliseconds since boot (provided by the firmware).
pub type Tick = u64;

/// Poll interval of the complex 400 (observed on the real panel). Basis for the liveness deadline.
pub const POLL_INTERVAL_MS: Tick = 3_000;
/// Serial liveness deadline: after 3 missed polls the stream is considered dead.
pub const SERIAL_DEADLINE_MS: Tick = 3 * POLL_INTERVAL_MS;
/// Hysteresis: this many valid frames after "stale" before returning to "online".
pub const RECOVERY_FRAMES: u32 = 2;
/// ACK timeout for a sent command (the other side must respond within 3 s).
pub const COMMAND_ACK_TIMEOUT_MS: Tick = 3_000;
/// Maximum send attempts for a command (first attempt + retries).
pub const COMMAND_MAX_ATTEMPTS: u8 = 3;
/// Maximum age of a queued command without a command window. In normal operation the
/// output telegram arrives every t_poll = 3 s — if it stays absent (misbehaving panel) while
/// the serial stream is alive, the command must not silently stall.
pub const PENDING_MAX_AGE_MS: Tick = 3 * POLL_INTERVAL_MS;

/// Canonical **complex 400H** area-status addresses (verified via pcap).
/// Legacy aliases of [`COMPLEX400`] — the profile is the source of truth
/// (equality guarded by `profile_matches_legacy_constants` below).
pub const ADDR_UNSCHARF: u16 = 0x0530;
pub const ADDR_INTERN_SCHARF: u16 = 0x0531;
pub const ADDR_EXTERN_SCHARF: u16 = 0x0532;
pub const ADDR_ALARM: u16 = 0x0533;
/// "arm-home ready" / "arm-away ready" area 1 (area-status block).
/// Active = panel is ready to arm (all relevant contacts closed).
pub const ADDR_INTERN_BEREIT: u16 = 0x0535;
pub const ADDR_EXTERN_BEREIT: u16 = 0x0536;
/// Base address for "detection area N bypassed": 0x05F0 + (N-1), N = 1…128.
pub const ADDR_MB_GESPERRT: u16 = 0x05F0;
/// Maximum detection area number on the complex (detection areas 1–128).
pub const MB_MAX: u8 = 128;

/// Already-authorized command arriving via the MQTT path (PIN/mTLS checked by the firmware).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmCommand {
    ArmAway,
    ArmHome,
    ArmNight,
    Disarm,
    Reset,
}

/// Derived panel arm state (from the block status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmState {
    Unknown,
    Disarmed,
    ArmedHome,
    ArmedNight,
    ArmedAway,
    Triggered,
}

impl ArmState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArmState::Unknown => "unknown",
            ArmState::Disarmed => "DISARMED",
            ArmState::ArmedHome => "ARMED_HOME",
            ArmState::ArmedNight => "ARMED_NIGHT",
            ArmState::ArmedAway => "ARMED_AWAY",
            ArmState::Triggered => "TRIGGERED",
        }
    }
}

impl ArmState {
    /// Severity rank for the multi-area aggregate (higher wins): a single triggered
    /// area must dominate the top-level state.
    fn severity(self) -> u8 {
        match self {
            ArmState::Unknown => 0,
            ArmState::Disarmed => 1,
            ArmState::ArmedHome => 2,
            ArmState::ArmedNight => 3,
            ArmState::ArmedAway => 4,
            ArmState::Triggered => 5,
        }
    }
}

/// Derived state of one Sicherungsbereich (multi-area panels; complex today: area 1 only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AreaState {
    pub arm: ArmState,
    /// Arm-home readiness bit of this area; `None` = not yet known.
    pub intern_bereit: Option<bool>,
    /// Arm-away readiness bit of this area; `None` = not yet known.
    pub extern_bereit: Option<bool>,
}

impl Default for AreaState {
    fn default() -> Self {
        AreaState {
            arm: ArmState::Unknown,
            intern_bereit: None,
            extern_bereit: None,
        }
    }
}

/// Availability (fail-safe). `Unavailable` = serial stream has gone silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Online,
    Unavailable,
}

/// Side effects to be executed by the firmware. The core itself does nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Write raw bytes to the UART (CONFIRM_ACK or command telegram).
    SendFrame(Vec<u8>),
    /// MQTT publish.
    Publish {
        topic: String,
        payload: String,
        retain: bool,
    },
    /// Persist the armed_night flag to NVS.
    PersistNightFlag(bool),
    /// Diagnostic/log message (never security-critical payloads or PIN).
    Log(String),
}

/// Core configuration.
#[derive(Default)]
pub struct CoreOptions {
    /// Remote disarm allowed? Default `false` (fail-closed).
    pub disarm_enabled: bool,
}

/// Der sans-IO-Kern.
pub struct Core {
    /// Shared with the app side (steady state holds ONE sensor table, not two copies).
    config: Arc<Config>,
    opts: CoreOptions,
    /// Panel-specific address map (selected from `config.panel.kind`).
    profile: &'static PanelProfile,
    /// Published top-level state: area 1 on single-area panels, severity aggregate on
    /// multi-area panels. Cache for change detection on the `state` topic.
    arm_state: ArmState,
    availability: Availability,
    night_flag: bool,
    /// Derived per-area states (arm + readiness), keyed by 1-based area id.
    areas: BTreeMap<u8, AreaState>,
    /// Last logical "active" state per sensor address (change detection).
    sensor_state: BTreeMap<u16, bool>,
    /// Bypass state per detection area (readback from `mb_gesperrt_base`+). Only
    /// observed transitions or bypassed areas are published (no boot topic flood).
    mb_bypassed: BTreeMap<u16, bool>,
    last_frame_at: Option<Tick>,
    recovery_count: u32,
    /// Queued command + prior send attempts + queue timestamp; sent in the next
    /// command window (pause after the SECOND status telegram).
    pending: Option<Pending>,
    /// Sent command awaiting CONFIRM_ACK (collision handling + ACK timeout).
    in_flight: Option<InFlight>,
}

/// Internal: what goes over the wire as a transaction — an authorized arm command or a
/// maintenance telegram (set time). Both share the window/ACK/retry discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxCmd {
    Arm {
        cmd: ArmCommand,
        area: u8,
    },
    SetTime(DateTime),
    /// Bypass detection area (`sperren=true`, 0x51) / unbypass (0xD1).
    Bypass {
        mb: u16,
        sperren: bool,
    },
    /// Switch output on (`0x00`) / off (`0x80`).
    Output {
        addr: u16,
        on: bool,
    },
    /// Trigger/clear Schaltaktion `n` (hiplex; same wire format as outputs at the
    /// profile's Schaltaktion base).
    Schaltaktion {
        n: u8,
        on: bool,
    },
}

impl TxCmd {
    /// Short label for `command_result`/logs (keeps legacy payloads like `OK ArmHome`
    /// stable; area 1 stays unsuffixed, other areas append ` B{n}`).
    fn label(&self) -> String {
        match self {
            TxCmd::Arm { cmd, area } => {
                let base = match cmd {
                    ArmCommand::ArmAway => "ArmAway",
                    ArmCommand::ArmHome => "ArmHome",
                    ArmCommand::ArmNight => "ArmNight",
                    ArmCommand::Disarm => "Disarm",
                    ArmCommand::Reset => "Reset",
                };
                if *area == 1 {
                    base.into()
                } else {
                    format!("{base} B{area}")
                }
            }
            TxCmd::SetTime(_) => "SetTime".into(),
            TxCmd::Bypass { mb, sperren: true } => format!("BypassOn MB{mb}"),
            TxCmd::Bypass { mb, sperren: false } => format!("BypassOff MB{mb}"),
            TxCmd::Output { addr, on: true } => format!("OutputOn 0x{addr:04X}"),
            TxCmd::Output { addr, on: false } => format!("OutputOff 0x{addr:04X}"),
            TxCmd::Schaltaktion { n, on: true } => format!("SchaltaktionOn S{n}"),
            TxCmd::Schaltaktion { n, on: false } => format!("SchaltaktionOff S{n}"),
        }
    }
}

/// A command waiting for the next command window.
struct Pending {
    cmd: TxCmd,
    attempts: u8,
    queued_at: Tick,
}

/// A sent command awaiting CONFIRM_ACK. Raw bytes are kept so a timeout retry repeats
/// the exact same telegram.
struct InFlight {
    cmd: TxCmd,
    frame: Vec<u8>,
    sent_at: Tick,
    /// Enqueue timestamp — for latency reporting in `command_result` (window wait time).
    queued_at: Tick,
    attempts: u8,
}

impl Core {
    pub fn new(config: Arc<Config>, opts: CoreOptions) -> Self {
        let profile = profile::from_config_kind(config.panel.kind);
        Core {
            config,
            opts,
            profile,
            arm_state: ArmState::Unknown,
            availability: Availability::Online,
            night_flag: false,
            areas: BTreeMap::new(),
            sensor_state: BTreeMap::new(),
            mb_bypassed: BTreeMap::new(),
            last_frame_at: None,
            recovery_count: 0,
            pending: None,
            in_flight: None,
        }
    }

    pub fn arm_state(&self) -> ArmState {
        self.arm_state
    }

    /// Active panel profile (address map/topology).
    pub fn profile(&self) -> &'static PanelProfile {
        self.profile
    }

    /// Active config (borrowed — do not keep another copy in ESP32 memory).
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Toggle the remote-disarm gate at runtime (security step in setup) without a full
    /// config reload.
    pub fn set_disarm_enabled(&mut self, on: bool) {
        self.opts.disarm_enabled = on;
    }
    pub fn availability(&self) -> Availability {
        self.availability
    }
    pub fn night_flag(&self) -> bool {
        self.night_flag
    }

    /// Last logical "active" state per confirmed sensor address (read-only, for the REST/live
    /// view). Mirrors the cyclic block status — never optimistic.
    pub fn sensor_states(&self) -> &BTreeMap<u16, bool> {
        &self.sensor_state
    }

    /// Arm-home readiness of area 1 (complex: panel bit 0x0535); `None` = not yet known.
    pub fn intern_ready(&self) -> Option<bool> {
        self.areas.get(&1).and_then(|a| a.intern_bereit)
    }

    /// Arm-away readiness of area 1 (complex: panel bit 0x0536); `None` = not yet known.
    pub fn extern_ready(&self) -> Option<bool> {
        self.areas.get(&1).and_then(|a| a.extern_bereit)
    }

    /// Per-area derived states (multi-area panels; complex populates area 1 only).
    pub fn area_states(&self) -> &BTreeMap<u8, AreaState> {
        &self.areas
    }

    /// On boot: restore the armed_night flag from NVS. Applied on the first fresh status
    /// (reconciliation in [`derive_arm_state`]).
    pub fn restore_night_flag(&mut self, flag: bool) {
        self.night_flag = flag;
    }

    /// A validated frame from the serial layer.
    pub fn on_frame(&mut self, now: Tick, frame: &Frame) -> Vec<Action> {
        let mut actions = Vec::new();
        self.note_alive(now, &mut actions);

        // Error telegram (0x11) embedded in a CONFIRM_ACK = rejected command.
        // Must not be swallowed silently — report to HA as command_result.
        let mut had_fehler = false;
        for rec in frame.records().filter_map(|r| r.ok()) {
            if let Some(f) = rec.as_fehler() {
                had_fehler = true;
                actions.push(command_result(format!("FEHLER {:?}", f.fehlercode)));
                actions.push(Action::Log(format!(
                    "Zentrale lehnte Befehl ab: {:?}",
                    f.fehlercode
                )));
            }
        }

        match frame.function() {
            Some(Function::SendNorm) => {
                // Panel SEND while our command is unacknowledged = collision;
                // our telegram was ignored → re-queue (bounded).
                self.requeue_on_collision(now, &mut actions);
                // FT1.2 send discipline: the SEND_NORM poll window is the RELIABLE send slot
                // of the real panel (confirmed by Discovery queries responding the same way).
                // The pure command window after the output telegram collided
                // reproducibly on the real panel ("TIMEOUT ArmHome: Kollision").
                // A pending command replaces the ACK here.
                if !self.flush_pending(now, &mut actions) {
                    actions.push(ack());
                }
            }
            Some(Function::SendNdat) => {
                self.requeue_on_collision(now, &mut actions);
                actions.push(ack());
                let outputs_seen = self.process_records(frame, &mut actions);
                // Command window: only in the pause AFTER the second status telegram
                // (outputs, address extension 0x02) — never between the two telegrams.
                if outputs_seen {
                    self.flush_pending(now, &mut actions);
                }
            }
            Some(Function::ConfirmAck) => {
                self.resolve_in_flight(now, true, had_fehler, &mut actions);
            }
            Some(Function::ConfirmNak) => {
                self.resolve_in_flight(now, false, had_fehler, &mut actions);
            }
            _ => {}
        }
        actions
    }

    /// An authorized command from the MQTT path (area 1 — legacy single-area path).
    pub fn on_command(&mut self, now: Tick, cmd: ArmCommand) -> Vec<Action> {
        self.on_command_area(now, cmd, 1)
    }

    /// An authorized command targeting a specific Sicherungsbereich (1-based; multi-area
    /// panels). Queued and sent in the next command window.
    pub fn on_command_area(&mut self, now: Tick, cmd: ArmCommand, area: u8) -> Vec<Action> {
        let count = self.profile.areas.count;
        if area == 0 || area > count {
            return vec![
                command_result(format!(
                    "DENIED {cmd:?}: Bereich {area} außerhalb 1–{count}"
                )),
                Action::Log(format!("Befehl {cmd:?} verworfen: Bereich {area} ungültig")),
            ];
        }
        if cmd == ArmCommand::Disarm && !self.opts.disarm_enabled {
            // fail-closed: remote disarm not enabled — report visibly, never silently.
            return vec![
                command_result("DENIED disarm: Remote-Disarm nicht aktiviert".into()),
                Action::Log("Disarm verworfen: Remote-Disarm nicht aktiviert (fail-closed)".into()),
            ];
        }
        if cmd == ArmCommand::ArmNight && area != 1 {
            // armed_night is a virtual state (arm-home + persisted flag) and the flag is
            // global — per-area night would silently degrade to plain arm-home.
            return vec![
                command_result(format!("DENIED ArmNight B{area}: nur Bereich 1")),
                Action::Log("ArmNight verworfen: virtueller Nacht-Modus nur für Bereich 1".into()),
            ];
        }
        let log = if area == 1 {
            format!("Befehl {cmd:?} eingereiht")
        } else {
            format!("Befehl {cmd:?} (Bereich {area}) eingereiht")
        };
        self.queue(now, TxCmd::Arm { cmd, area }, log)
    }

    /// Bypass/unbypass a detection area. **Bypassing reduces security**
    /// and therefore follows the same fail-closed policy as remote disarm (`disarm_enabled`
    /// master switch; PIN check is done by the authorization layer above). Unbypassing
    /// increases security and is — like arm — always allowed.
    pub fn on_bypass(&mut self, now: Tick, mb: u16, sperren: bool) -> Vec<Action> {
        let mb_max = self.profile.mb_max;
        if mb == 0 || mb > mb_max {
            return vec![Action::Log(format!(
                "Bypass verworfen: Meldebereich {mb} außerhalb 1–{mb_max}"
            ))];
        }
        if sperren && !self.opts.disarm_enabled {
            return vec![Action::Log(
                "Bypass verworfen: Remote-Steuerung sicherheitsreduzierender Befehle \
                 nicht aktiviert (fail-closed)"
                    .into(),
            )];
        }
        self.queue(
            now,
            TxCmd::Bypass { mb, sperren },
            format!(
                "Bypass eingereiht: MB{mb} {}",
                if sperren { "sperren" } else { "entsperren" }
            ),
        )
    }

    /// Bypass state per detection area (readback `mb_gesperrt_base`+); empty until the
    /// first snapshot.
    pub fn mb_bypassed(&self) -> &BTreeMap<u16, bool> {
        &self.mb_bypassed
    }

    /// Switch an output on/off. **Fail-closed allowlist:** only outputs
    /// confirmed AND marked `switchable` in the config (explicit user approval in physical
    /// setup) — everything else is rejected. Switch state always comes from the readback
    /// (snapshot = truth).
    pub fn on_output(&mut self, now: Tick, addr: u16, on: bool) -> Vec<Action> {
        // Address check in addition to the allowlist: even a hand-edited config must never
        // direct the output path at arm/bypass addresses (system status block).
        let allowed = self.profile.is_switchable_addr(addr)
            && self
                .config
                .sensors
                .iter()
                .any(|s| s.address() == addr && s.confirmed() && s.switchable());
        if !allowed {
            return vec![Action::Log(format!(
                "Output 0x{addr:04X} verworfen: nicht in der Schalt-Allowlist (fail-closed)"
            ))];
        }
        self.queue(
            now,
            TxCmd::Output { addr, on },
            format!(
                "Output eingereiht: 0x{addr:04X} {}",
                if on { "ein" } else { "aus" }
            ),
        )
    }

    /// Trigger/clear a Schaltaktion (hiplex). Same wire format as an output command, at
    /// the profile's Schaltaktion base. **Fail-closed:** requires (a) a profile with a
    /// known Schaltaktion base (unknown until a capture names it), (b) the remote-control
    /// master switch (`disarm_enabled` — Schaltaktionen can drive sirens/locks), and
    /// (c) the hiplex command gate in [`Self::queue`].
    pub fn on_schaltaktion(&mut self, now: Tick, n: u8, on: bool) -> Vec<Action> {
        let Some(base) = self.profile.schaltaktion_base else {
            return vec![
                command_result(format!(
                    "DENIED Schaltaktion S{n}: Basisadresse unbekannt (Capture ausstehend)"
                )),
                Action::Log(
                    "Schaltaktion verworfen: Basisadresse im Profil unbekannt — erst am \
                     Capture kartieren"
                        .into(),
                ),
            ];
        };
        let max = self.profile.schaltaktion_max;
        if n == 0 || n > max {
            return vec![Action::Log(format!(
                "Schaltaktion verworfen: S{n} außerhalb 1–{max}"
            ))];
        }
        if !self.opts.disarm_enabled {
            return vec![Action::Log(
                "Schaltaktion verworfen: Remote-Steuerung nicht aktiviert (fail-closed)".into(),
            )];
        }
        let _ = base; // encoded in flush_pending from the profile
        self.queue(
            now,
            TxCmd::Schaltaktion { n, on },
            format!(
                "Schaltaktion eingereiht: S{n} {}",
                if on { "ein" } else { "aus" }
            ),
        )
    }

    /// Set the panel date/time (VdS 2465 record type 0x50) — e.g. periodically after SNTP sync or on
    /// `event=restart`. Low priority: yields to a pending arm command and is retried at
    /// the next sync opportunity.
    pub fn on_set_time(&mut self, now: Tick, dt: DateTime) -> Vec<Action> {
        if self.pending.is_some() || self.in_flight.is_some() {
            return vec![Action::Log(
                "Zeitstellen übersprungen: Befehls-Transaktion aktiv".into(),
            )];
        }
        self.queue(
            now,
            TxCmd::SetTime(dt),
            format!(
                "Zeitstellen eingereiht: {:02}.{:02}.{:02} {:02}:{:02}:{:02} (Wochentag {})",
                dt.tag, dt.monat, dt.jahr, dt.stunde, dt.minute, dt.sekunde, dt.jh_or_weekday
            ),
        )
    }

    /// Periodic tick: checks the serial liveness deadline and command ACK timeout.
    pub fn on_tick(&mut self, now: Tick) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.availability == Availability::Online {
            // `None` = no frame ever received since boot (e.g. flashed/booted without panel)
            // → go offline after the deadline; otherwise `availability` would stay "online" forever.
            let stale = match self.last_frame_at {
                Some(last) => now.saturating_sub(last) > SERIAL_DEADLINE_MS,
                None => now > SERIAL_DEADLINE_MS,
            };
            if stale {
                self.go_unavailable(&mut actions);
            }
        }
        // Queued command without a command window (output telegram absent, serial alive):
        // must not stall silently — discard visibly.
        if let Some(p) = &self.pending {
            if now.saturating_sub(p.queued_at) > PENDING_MAX_AGE_MS {
                let p = self.pending.take().expect("just checked");
                fail_command(
                    p.cmd,
                    "kein Befehlsfenster (Ausgänge-Telegramm fehlt)",
                    &mut actions,
                );
            }
        }

        // Unacknowledged command: repeat identically after 3 s, bounded —
        // then fail visibly rather than swallow silently.
        if let Some(inf) = &mut self.in_flight {
            if now.saturating_sub(inf.sent_at) > COMMAND_ACK_TIMEOUT_MS {
                if inf.attempts >= COMMAND_MAX_ATTEMPTS {
                    let cmd = inf.cmd;
                    self.in_flight = None;
                    fail_command(cmd, "keine Quittung der Zentrale", &mut actions);
                } else {
                    inf.attempts += 1;
                    inf.sent_at = now;
                    actions.push(Action::SendFrame(inf.frame.clone()));
                    actions.push(Action::Log(format!(
                        "Befehl {} unquittiert — Wiederholung (Versuch {})",
                        inf.cmd.label(),
                        inf.attempts
                    )));
                }
            }
        }
        actions
    }

    // --- private --------------------------------------------------------

    /// Queue a command for the next command window (one pending command; last wins).
    ///
    /// **hiplex command gate:** the hiplex address map is forum-sourced and unverified
    /// until the first real capture (`profile/hiplex_provisional.rs`). Firing telegrams
    /// at guessed addresses on a live alarm panel is the one real risk of the provisional
    /// profile — every address-bearing command is therefore rejected VISIBLY until the
    /// user sets `panel.hiplex_cmds_verified` after the capture diff. Address-free
    /// maintenance (SetTime, record 0x50) and the entire read path are never gated.
    fn queue(&mut self, now: Tick, cmd: TxCmd, log: String) -> Vec<Action> {
        if self.profile.kind == PanelKind::Hiplex8400
            && !self.config.panel.hiplex_cmds_verified
            && !matches!(cmd, TxCmd::SetTime(_))
        {
            return vec![
                command_result(format!(
                    "DENIED {}: hiplex-Befehlsadressen unverifiziert",
                    cmd.label()
                )),
                Action::Log(
                    "Befehl verworfen: hiplex-Adressen noch nicht am Capture verifiziert — \
                     hiplex_cmds_verified erst nach dem Abgleich setzen (fail-closed)"
                        .into(),
                ),
            ];
        }
        self.pending = Some(Pending {
            cmd,
            attempts: 0,
            queued_at: now,
        });
        vec![Action::Log(log)]
    }

    fn note_alive(&mut self, now: Tick, actions: &mut Vec<Action>) {
        self.last_frame_at = Some(now);
        if self.availability == Availability::Unavailable {
            self.recovery_count += 1;
            if self.recovery_count >= RECOVERY_FRAMES {
                self.availability = Availability::Online;
                self.recovery_count = 0;
                actions.push(Action::Publish {
                    topic: "availability".into(),
                    payload: "online".into(),
                    retain: true,
                });
            }
        }
    }

    fn go_unavailable(&mut self, actions: &mut Vec<Action>) {
        self.availability = Availability::Unavailable;
        self.recovery_count = 0;
        // Fail-safe: discard pending/in-flight commands — an arm/disarm fired minutes later
        // (after serial recovery) would be a security risk.
        if let Some(p) = self.pending.take() {
            fail_command(p.cmd, "Serial offline — Befehl verworfen", actions);
        }
        if let Some(inf) = self.in_flight.take() {
            fail_command(inf.cmd, "Serial offline — Befehl verworfen", actions);
        }
        // Invalidate cache: after recovery the fresh snapshot must republish everything.
        // Otherwise HA would stay stuck on the retained `state=unavailable` if the area
        // returns to the same arm state (new == cached → no publish).
        self.arm_state = ArmState::Unknown;
        self.areas.clear();
        self.sensor_state.clear();
        self.mb_bypassed.clear();
        actions.push(Action::Publish {
            topic: "availability".into(),
            payload: "offline".into(),
            retain: true,
        });
        actions.push(Action::Publish {
            topic: "state".into(),
            payload: "unavailable".into(),
            retain: true,
        });
        // Multi-area panels: the per-area topics are retained too — mark them all.
        if self.profile.areas.count > 1 {
            for area in 1..=self.profile.areas.count {
                actions.push(Action::Publish {
                    topic: format!("area/{area}/state"),
                    payload: "unavailable".into(),
                    retain: true,
                });
            }
        }
        actions.push(Action::Log("Serial-Strom verstummt → unavailable".into()));
    }

    /// Processes the records of a SEND_NDAT. Returns `true` if the telegram contained the
    /// output block status (address extension 0x02) — the second status telegram of the poll
    /// cycle and the start of the command window (observed on the real panel).
    fn process_records(&mut self, frame: &Frame, actions: &mut Vec<Action>) -> bool {
        let mut outputs_seen = false;
        for rec in frame.records().filter_map(|r| r.ok()) {
            if let Some(bs) = rec.as_block_status() {
                outputs_seen |= bs.adresserweiterung == ERW_AUSGAENGE;
                self.apply_block_status(&bs, actions);
            } else if let Some(m) = rec.as_meldung() {
                self.handle_spontaneous(m.address(), m.kind(), m.is_active(), actions);
            }
        }
        outputs_seen
    }

    /// Collision resolution: if the GMS receives a panel SEND while our command is
    /// unacknowledged, the panel telegram had priority — ours is considered discarded and
    /// re-queued (bounded).
    fn requeue_on_collision(&mut self, now: Tick, actions: &mut Vec<Action>) {
        let Some(inf) = self.in_flight.take() else {
            return;
        };
        if inf.attempts >= COMMAND_MAX_ATTEMPTS {
            fail_command(inf.cmd, "Kollision, Versuche erschöpft", actions);
        } else {
            actions.push(Action::Log(format!(
                "Befehl {} kollidierte mit Panel-Telegramm — erneut eingereiht",
                inf.cmd.label()
            )));
            self.pending = Some(Pending {
                cmd: inf.cmd,
                attempts: inf.attempts,
                queued_at: now,
            });
        }
    }

    /// Panel acknowledgement for our command: ACK = accepted (unless an embedded 0x11
    /// error record was present), NAK = rejected.
    fn resolve_in_flight(
        &mut self,
        now: Tick,
        ack: bool,
        had_fehler: bool,
        actions: &mut Vec<Action>,
    ) {
        let Some(inf) = self.in_flight.take() else {
            return;
        };
        if had_fehler {
            // FEHLER command_result already published (0x11 scan in on_frame).
            actions.push(Action::Log(format!(
                "Befehl {} von der Zentrale abgelehnt (Fehlersatz)",
                inf.cmd.label()
            )));
        } else if ack {
            actions.push(command_result(format!("OK {}", inf.cmd.label())));
            // Latency breakdown goes to the diagnostic log only (UI stays clean):
            // window = wait for the panel's send window (protocol, avg ~1 s),
            // ack = panel processed/switched (ms).
            actions.push(Action::Log(format!(
                "Befehl {}: Fenster {} ms, Quittung {} ms",
                inf.cmd.label(),
                inf.sent_at.saturating_sub(inf.queued_at),
                now.saturating_sub(inf.sent_at),
            )));
        } else {
            actions.push(command_result(format!("NAK {}", inf.cmd.label())));
            actions.push(Action::Log(format!(
                "Befehl {} per CONFIRM_NAK abgelehnt",
                inf.cmd.label()
            )));
        }
    }

    /// Apply a snapshot: update sensor states and derive the arm state.
    fn apply_block_status(&mut self, bs: &BlockStatus, actions: &mut Vec<Action>) {
        // Only real status blocks (inputs 0x01 / outputs 0x02) drive state.
        // Occupancy/Discovery responses (0x71/0x72) must NOT corrupt live status.
        if !matches!(bs.adresserweiterung, 0x01 | 0x02) {
            return;
        }
        // Track states of ALL configured sensors in this block — including unconfirmed ones,
        // so the live test board shows something during polarity/door-open tests.
        // Only confirmed sensors are published to MQTT/HA (no HA noise).
        // Collect only TRANSITIONS (clone topic only after change check — avoids allocating
        // for every unchanged sensor on every 3 s poll).
        let updates: Vec<(u16, Option<String>, bool)> = self
            .config
            .sensors
            .iter()
            .filter_map(|s| {
                let raw = bs.raw_bit(s.address())?;
                let active = s.polarity().is_active(raw)?;
                if self.sensor_state.get(&s.address()) == Some(&active) {
                    return None;
                }
                let topic = s.confirmed().then(|| format!("sensor/{}/state", s.topic()));
                Some((s.address(), topic, active))
            })
            .collect();

        for (addr, topic, active) in updates {
            self.sensor_state.insert(addr, active);
            if let Some(topic) = topic {
                actions.push(Action::Publish {
                    topic,
                    payload: if active { "ON" } else { "OFF" }.into(),
                    retain: true,
                });
            }
        }

        // Derive arm state + arm readiness per area from blocks that contain area status.
        let count = self.profile.areas.count;
        let mut any_area_seen = false;
        for area in 1..=count {
            if bs.raw_bit(self.profile.addr_unscharf(area)).is_none() {
                continue;
            }
            any_area_seen = true;
            let new_state = self.derive_arm_state(bs, area, actions);
            let changed = {
                let entry = self.areas.entry(area).or_default();
                let changed = entry.arm != new_state;
                entry.arm = new_state;
                changed
            };
            // Per-area topics only on multi-area panels — single-area (complex) keeps
            // exactly the historical topic set.
            if changed && count > 1 {
                actions.push(Action::Publish {
                    topic: format!("area/{area}/state"),
                    payload: new_state.as_str().into(),
                    retain: true,
                });
            }
            self.update_readiness(bs, area, actions);
        }
        if any_area_seen {
            // Top-level `state`: area 1 (single-area) / severity aggregate (multi-area).
            let top = self.top_level_state();
            if top != self.arm_state {
                self.arm_state = top;
                actions.push(Action::Publish {
                    topic: "state".into(),
                    payload: top.as_str().into(),
                    retain: true,
                });
            }
        }

        // Bypass readback (0x05F0+): "snapshot = truth" also applies to the bypass feature —
        // state comes from the cyclic status, never from the sent command.
        self.update_mb_bypassed(bs, actions);
    }

    /// Tracks "detection area N bypassed" and publishes transitions as `mb/<n>/bypassed`.
    /// First observations are only published when bypassed (no topic boot flood;
    /// HA interprets missing topics as OFF).
    fn update_mb_bypassed(&mut self, bs: &BlockStatus, actions: &mut Vec<Action>) {
        // Walk only the overlap of the block window and the bypass range — instead of
        // blindly checking all areas on the 3 s hot path.
        let base = self.profile.mb_gesperrt_base;
        let block_end = bs.base_address().saturating_add(bs.status.len() as u16 * 8);
        let lo = bs.base_address().max(base);
        let hi = block_end.min(base + self.profile.mb_max);
        for addr in lo..hi {
            let mb = (addr - base) + 1;
            let Some(bypassed) = bs.is_active(addr) else {
                continue;
            };
            let prev = self.mb_bypassed.insert(mb, bypassed);
            let report = match prev {
                Some(old) => old != bypassed,
                None => bypassed,
            };
            if report {
                actions.push(Action::Publish {
                    topic: format!("mb/{mb}/bypassed"),
                    payload: if bypassed { "ON" } else { "OFF" }.into(),
                    retain: true,
                });
            }
        }
    }

    /// Tracks the panel's "ready" bits per area and publishes them so HA can gate the
    /// arm button. Single-area panels keep the historical `ready/*` topics; multi-area
    /// panels publish `area/{id}/ready/*`.
    fn update_readiness(&mut self, bs: &BlockStatus, area: u8, actions: &mut Vec<Action>) {
        let multi = self.profile.areas.count > 1;
        for (addr, field, suffix) in [
            (self.profile.addr_intern_bereit(area), 0u8, "intern"),
            (self.profile.addr_extern_bereit(area), 1u8, "extern"),
        ] {
            let ready = bs.is_active(addr);
            let entry = self.areas.entry(area).or_default();
            let slot = if field == 0 {
                &mut entry.intern_bereit
            } else {
                &mut entry.extern_bereit
            };
            if *slot != ready {
                *slot = ready;
                let topic = if multi {
                    format!("area/{area}/ready/{suffix}")
                } else {
                    format!("ready/{suffix}")
                };
                actions.push(Action::Publish {
                    topic,
                    payload: if ready == Some(true) { "yes" } else { "no" }.into(),
                    retain: true,
                });
            }
        }
    }

    /// Top-level `state` payload: area 1 on single-area panels; on multi-area panels the
    /// severity aggregate (a single triggered/armed area dominates), so single-panel
    /// dashboards and HomeKit keep working.
    fn top_level_state(&self) -> ArmState {
        if self.profile.areas.count == 1 {
            self.areas
                .get(&1)
                .map(|a| a.arm)
                .unwrap_or(ArmState::Unknown)
        } else {
            self.areas
                .values()
                .map(|a| a.arm)
                .max_by_key(|s| s.severity())
                .unwrap_or(ArmState::Unknown)
        }
    }

    /// Names of currently open/triggered door/window contacts (for arm-reject reason).
    fn open_contacts(&self) -> String {
        let names: Vec<&str> = self
            .config
            .sensors
            .iter()
            .filter(|s| {
                matches!(
                    s.kind(),
                    telenot_config::SensorKind::Magnetkontakt
                        | telenot_config::SensorKind::Schliesskontakt
                ) && self.sensor_state.get(&s.address()) == Some(&true)
            })
            .map(|s| s.name())
            .collect();
        if names.is_empty() {
            "—".into()
        } else {
            names.join(", ")
        }
    }

    /// Derives one area's arm state from the area status. The virtual night mode
    /// (armed_night = arm-home + persisted flag) is bound to area 1 — only there the
    /// flag is reconciled (panel = truth: only apply when armed-home, otherwise discard).
    fn derive_arm_state(
        &mut self,
        bs: &BlockStatus,
        area: u8,
        actions: &mut Vec<Action>,
    ) -> ArmState {
        let p = self.profile;
        let active = |addr: u16| bs.is_active(addr).unwrap_or(false);

        if active(p.addr_alarm(area)) {
            return ArmState::Triggered;
        }
        if active(p.addr_extern_scharf(area)) {
            if area == 1 {
                self.clear_night_flag_if_set(actions);
            }
            return ArmState::ArmedAway;
        }
        if active(p.addr_intern_scharf(area)) {
            return if area == 1 && self.night_flag {
                ArmState::ArmedNight
            } else {
                ArmState::ArmedHome
            };
        }
        if active(p.addr_unscharf(area)) {
            if area == 1 {
                self.clear_night_flag_if_set(actions);
            }
            return ArmState::Disarmed;
        }
        ArmState::Unknown
    }

    fn clear_night_flag_if_set(&mut self, actions: &mut Vec<Action>) {
        if self.night_flag {
            self.night_flag = false;
            actions.push(Action::PersistNightFlag(false));
        }
    }

    fn handle_spontaneous(
        &mut self,
        address: u16,
        kind: MeldungsArt,
        active: bool,
        actions: &mut Vec<Action>,
    ) {
        // Event overlay: publish alarms/sabotage as diagnostic events (no state owner).
        if matches!(
            kind,
            MeldungsArt::Einbruch
                | MeldungsArt::Ueberfall
                | MeldungsArt::Sabotage
                | MeldungsArt::Brand
        ) && active
        {
            actions.push(Action::Publish {
                topic: "event".into(),
                payload: format!("{kind:?}@0x{address:04X}"),
                retain: false,
            });
        }

        // Fault/technical detection points (battery/mains/transmission path/fault/technical/
        // shutdown): must NOT be silently discarded — publish as retained diagnostic state
        // (`diag/<type>` ON/OFF, message-type bit 7 = active/on) plus a spontaneous `event`,
        // so HA can show battery/mains/transmission/fault entities (see MQTT contract).
        if let Some(slug) = diag_slug(kind) {
            actions.push(Action::Publish {
                topic: format!("diag/{slug}"),
                payload: if active { "ON" } else { "OFF" }.into(),
                retain: true,
            });
            actions.push(Action::Publish {
                topic: "event".into(),
                payload: format!(
                    "{kind:?}@0x{address:04X}={}",
                    if active { "ein" } else { "aus" }
                ),
                retain: false,
            });
        }

        // Panel restart (0x53): publish + re-sync hint. After a restart the firmware
        // should re-verify occupancy/area status.
        if kind == MeldungsArt::Neustart {
            actions.push(Action::Publish {
                topic: "event".into(),
                payload: "restart".into(),
                retain: false,
            });
            actions.push(Action::Log(
                "Zentrale-Neustart erkannt — Re-Sync empfohlen".into(),
            ));
        }
    }

    /// Sends the pending command if the line is free. Returns `true` if a frame was sent
    /// (occupies the ACK slot in the SEND_NORM window).
    fn flush_pending(&mut self, now: Tick, actions: &mut Vec<Action>) -> bool {
        // Only ONE command transaction on the line at a time.
        if self.in_flight.is_some() {
            return false;
        }
        let Some(Pending {
            cmd,
            attempts: prior_attempts,
            queued_at,
        }) = self.pending.take()
        else {
            return false;
        };

        let mut buf = [0u8; 32];
        let encoded = match cmd {
            TxCmd::Arm { cmd: arm, area } => {
                // Pre-arm gate: the panel will not arm if it does not report "ready" (open
                // contact). We do not send at all and report why — instead of sending a
                // telegram the panel would ignore.
                let area_state = self.areas.get(&area).copied().unwrap_or_default();
                let readiness = match arm {
                    ArmCommand::ArmHome | ArmCommand::ArmNight => {
                        Some((area_state.intern_bereit, "intern"))
                    }
                    ArmCommand::ArmAway => Some((area_state.extern_bereit, "extern")),
                    ArmCommand::Disarm | ArmCommand::Reset => None,
                };
                if let Some((ready, what)) = readiness {
                    if ready != Some(true) {
                        actions.push(command_result(format!(
                            "REJECTED {arm:?}: {what} nicht scharfschaltbereit (offen: {})",
                            self.open_contacts()
                        )));
                        actions.push(Action::Log(format!(
                            "Arm {arm:?} verworfen: {what}_bereit = {ready:?}"
                        )));
                        return false;
                    }
                }

                let p = self.profile;
                let (addr, meldungsart) = match arm {
                    ArmCommand::ArmAway => (p.addr_extern_scharf(area), ART_EXTERN_SCHARF),
                    ArmCommand::ArmHome | ArmCommand::ArmNight => {
                        (p.addr_intern_scharf(area), ART_INTERN_SCHARF)
                    }
                    ArmCommand::Disarm => (p.addr_unscharf(area), ART_UNSCHARF),
                    ArmCommand::Reset => (p.addr_alarm(area), ART_RUECKSETZEN),
                };

                // armed_night is virtual: arm-home + flag, bound to area 1 (ArmNight for
                // other areas is rejected at queue time). Only area-1 arms may touch the
                // flag — an arm on area 2 must not clear a night mode running on area 1.
                if area == 1 {
                    let want_night = arm == ArmCommand::ArmNight;
                    if want_night != self.night_flag {
                        self.night_flag = want_night;
                        actions.push(Action::PersistNightFlag(want_night));
                    }
                }

                encode_command_02(addr, ERW_AUSGAENGE, meldungsart, &mut buf)
            }
            TxCmd::SetTime(dt) => encode_set_datetime(&dt, &mut buf),
            TxCmd::Bypass { mb, sperren } => {
                // Address "detection area N bypassed", message type 0x51 / 0xD1.
                let addr = self.profile.addr_mb_gesperrt(mb);
                let art = if sperren {
                    ART_MB_SPERREN
                } else {
                    ART_MB_ENTSPERREN
                };
                encode_command_02(addr, ERW_AUSGAENGE, art, &mut buf)
            }
            TxCmd::Output { addr, on } => {
                // Message type 0x00 = on, 0x80 = off.
                let art = if on { ART_AUSGANG_EIN } else { ART_AUSGANG_AUS };
                encode_command_02(addr, ERW_AUSGAENGE, art, &mut buf)
            }
            TxCmd::Schaltaktion { n, on } => {
                // Validated in on_schaltaktion; base presence re-checked fail-closed.
                match self.profile.schaltaktion_base {
                    Some(base) => {
                        let art = if on { ART_AUSGANG_EIN } else { ART_AUSGANG_AUS };
                        encode_command_02(base + (n as u16 - 1), ERW_AUSGAENGE, art, &mut buf)
                    }
                    None => {
                        actions.push(Action::Log(
                            "Schaltaktion ohne Basisadresse im Sendepfad verworfen".into(),
                        ));
                        return false;
                    }
                }
            }
        };

        match encoded {
            Ok(n) => {
                let frame = buf[..n].to_vec();
                actions.push(Action::SendFrame(frame.clone()));
                self.in_flight = Some(InFlight {
                    cmd,
                    frame,
                    sent_at: now,
                    queued_at,
                    attempts: prior_attempts + 1,
                });
                true
            }
            Err(_) => {
                actions.push(Action::Log("Command-Encoding fehlgeschlagen".into()));
                false
            }
        }
    }
}

/// `command_result` publish (not retained — results are events, not state).
fn command_result(payload: String) -> Action {
    Action::Publish {
        topic: "command_result".into(),
        payload,
        retain: false,
    }
}

/// Visible command failure: never swallow silently (alarm panel — the user must know that
/// their arm/disarm did NOT go through).
fn fail_command(cmd: TxCmd, why: &str, actions: &mut Vec<Action>) {
    actions.push(command_result(format!("TIMEOUT {}: {why}", cmd.label())));
    actions.push(Action::Log(format!(
        "Befehl {} gescheitert: {why}",
        cmd.label()
    )));
}

fn ack() -> Action {
    let mut buf = [0u8; 8];
    let n = encode_conf_ack(&mut buf).expect("CONF_ACK passt");
    Action::SendFrame(buf[..n].to_vec())
}

/// Diagnostic topic suffix (`diag/<slug>`) for spontaneous fault/technical messages, or `None`
/// for types that do not get a steady-state diagnostic entity (alarms/area messages).
fn diag_slug(kind: MeldungsArt) -> Option<&'static str> {
    Some(match kind {
        MeldungsArt::StoerungAkku => "akku",
        MeldungsArt::StoerungNetz => "netz",
        MeldungsArt::StoerungUebertragungsweg => "uebertragung",
        MeldungsArt::Stoerung => "stoerung",
        MeldungsArt::TechnischeMeldung | MeldungsArt::Technikalarm => "technik",
        MeldungsArt::Abschaltung => "abschaltung",
        _ => return None,
    })
}

/// Decodes Telenot detection-point text (record type 0x54) to UTF-8. The panel stores text
/// in the **HD44780-A00 LCD character set** (display controller of the keypads), NOT in
/// Latin-1/UTF-8. Verified on real panel text: 0xE1=ä, 0xF5=ü (standard A00 ROM).
/// Unmappable bytes become '?' (prevents broken UTF-8 / wrong glyphs).
pub fn decode_panel_text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0xE1 => 'ä',
            0xEF => 'ö',
            0xF5 => 'ü',
            0xE2 => 'ß',
            0x20..=0x7E => b as char,
            _ => '?',
        })
        .collect()
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    /// The complex profile must equal the historical, pcap-verified literals forever —
    /// this is the "complex 400H is bit-identical after the profile refactor" guarantee.
    #[test]
    fn profile_matches_legacy_constants() {
        let p = &COMPLEX400;
        assert_eq!(p.addr_unscharf(1), ADDR_UNSCHARF);
        assert_eq!(p.addr_intern_scharf(1), ADDR_INTERN_SCHARF);
        assert_eq!(p.addr_extern_scharf(1), ADDR_EXTERN_SCHARF);
        assert_eq!(p.addr_alarm(1), ADDR_ALARM);
        assert_eq!(p.addr_intern_bereit(1), ADDR_INTERN_BEREIT);
        assert_eq!(p.addr_extern_bereit(1), ADDR_EXTERN_BEREIT);
        assert_eq!(p.mb_gesperrt_base, ADDR_MB_GESPERRT);
        assert_eq!(p.addr_mb_gesperrt(1), ADDR_MB_GESPERRT);
        assert_eq!(p.mb_max, MB_MAX as u16);
        assert_eq!(p.areas.count, 1);
        assert_eq!(p.output_addr_range, telenot_config::OUTPUT_ADDR_RANGE);
        assert_eq!(p.status_addr_range, telenot_config::STATUS_ADDR_RANGE);
        // Switchable semantics identical to the config-level check for the whole space.
        for addr in 0x0400..0x0800u16 {
            assert_eq!(
                p.is_switchable_addr(addr),
                telenot_config::is_switchable_addr(addr),
                "switchable mismatch at 0x{addr:04X}"
            );
        }
    }
}

#[cfg(test)]
mod text_tests {
    use super::decode_panel_text;

    #[test]
    fn decodes_hd44780_umlauts() {
        // 'ESG Geh<0xE1>use' / 'MK T<0xF5>r' from the real capture.
        assert_eq!(decode_panel_text(b"ESG Geh\xE1use"), "ESG Gehäuse");
        assert_eq!(decode_panel_text(b"MK T\xF5r"), "MK Tür");
        assert_eq!(decode_panel_text(b"St\xEFrung"), "Störung");
        assert_eq!(decode_panel_text(b"Stra\xE2e"), "Straße");
        assert_eq!(decode_panel_text(b"IM Essen"), "IM Essen");
    }
}

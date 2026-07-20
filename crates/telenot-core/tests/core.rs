//! Host tests for the sans-IO core — against REAL frame fixtures from the panel and against
//! synthetic scenarios with injected time (no I/O, no sleep).

use telenot_config::{Config, Polarity, Sensor, SensorKind, CURRENT_SCHEMA_VERSION};
use telenot_core::{
    Action, ArmCommand, ArmState, Availability, Core, CoreOptions, Tick, COMMAND_ACK_TIMEOUT_MS,
    COMMAND_MAX_ATTEMPTS, EAGER_IDLE_GUARD_MS, RECOVERY_FRAMES, SERIAL_DEADLINE_MS,
};
use telenot_protocol::{
    encode_command_02, encode_frame, satztyp, Frame, FrameDecoder, C_SEND_NDAT, ERW_AUSGAENGE,
};

const OUTSTATUS_DISARMED: &str = "6846466873023a2400050002fffffffffffffe9e9e9e9e9e9e9efffcffffffffffffffffffffffffffff7fffffffffffffffffffffffffffffffffffffffffffffff0656999999ffffff9d16";
const OUTSTATUS_INTERN: &str = "6846466873023a2400050002ffffffffffffdd9e9e9e9e9e9e9efffdffffffffffffffffffffffffffff7fffffffffffffffffffffffffffffffefefefffffffffff0656999999ffffff4d16";
const INSTATUS: &str = "683e3e687302322400000001fffffffffffffffffffffffffffffffffdfffffffffffbfffffffffffffffffffdffffffffffffffffffffffffff0656999999ffffffba16";
// Real frame: disarmed AND arm-home-ready (0x0535 active), arm-away NOT ready.
const READY_DISARMED: &str = "6846466873023a2400050002ffffffffffffde9e9e9e9e9e9e9efffdffffffffffffffffffffffffffff7fffffffffffffffffffffffffffffffffffffffffffffff0656999999ffffff7e16";

fn decode(s: &str) -> Frame {
    let bytes: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    let mut d = FrameDecoder::new();
    d.feed(&bytes);
    d.next_frame().unwrap().unwrap()
}

fn sensor(address: u16, topic: &str, kind: SensorKind) -> Sensor {
    Sensor {
        address,
        name: topic.into(),
        name_ha: topic.into(),
        kind,
        topic: topic.into(),
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
        // The existing suite exercises the strict poll-window path deterministically; eager-send
        // (default ON in production) has its own tests below via `core_eager`.
        panel: telenot_config::PanelSettings {
            eager_send: false,
            ..Default::default()
        },
    }
}

fn core(disarm: bool) -> Core {
    Core::new(
        std::sync::Arc::new(cfg(vec![sensor(
            0x0075,
            "eg/essen/bewegung",
            SensorKind::Bewegungsmelder,
        )])),
        CoreOptions {
            disarm_enabled: disarm,
        },
    )
}

/// Like [`core`] but with eager-send ENABLED (production default) — for the eager-path tests.
fn core_eager(disarm: bool) -> Core {
    Core::new(
        std::sync::Arc::new(Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: telenot_config::SensorTable::from_sensors(&[sensor(
                0x0075,
                "eg/essen/bewegung",
                SensorKind::Bewegungsmelder,
            )])
            .unwrap(),
            panel: telenot_config::PanelSettings {
                eager_send: true,
                ..Default::default()
            },
        }),
        CoreOptions {
            disarm_enabled: disarm,
        },
    )
}

fn has_send(actions: &[Action], bytes: &[u8]) -> bool {
    actions
        .iter()
        .any(|a| matches!(a, Action::SendFrame(b) if b == bytes))
}
fn published(actions: &[Action], topic: &str) -> Option<String> {
    actions.iter().find_map(|a| match a {
        Action::Publish {
            topic: t, payload, ..
        } if t == topic => Some(payload.clone()),
        _ => None,
    })
}

const CONF_ACK: [u8; 8] = [0x68, 0x02, 0x02, 0x68, 0x00, 0x02, 0x02, 0x16];

#[test]
fn acks_every_status_telegram() {
    let mut c = core(false);
    let a = c.on_frame(0, &decode(OUTSTATUS_DISARMED));
    assert!(
        has_send(&a, &CONF_ACK),
        "jedes SEND_NDAT muss geackt werden"
    );
}

#[test]
fn arm_state_derived_from_real_snapshots() {
    let mut c = core(false);
    let a = c.on_frame(0, &decode(OUTSTATUS_DISARMED));
    assert_eq!(c.arm_state(), ArmState::Disarmed);
    assert_eq!(published(&a, "state").as_deref(), Some("DISARMED"));

    let a = c.on_frame(POLL_NEXT, &decode(OUTSTATUS_INTERN));
    assert_eq!(
        c.arm_state(),
        ArmState::ArmedHome,
        "without night-flag = home"
    );
    assert_eq!(published(&a, "state").as_deref(), Some("ARMED_HOME"));
}
const POLL_NEXT: Tick = 3_000;

#[test]
fn restored_night_flag_yields_armed_night() {
    let mut c = core(false);
    c.restore_night_flag(true);
    c.on_frame(0, &decode(OUTSTATUS_INTERN));
    assert_eq!(
        c.arm_state(),
        ArmState::ArmedNight,
        "arm-home + flag = night"
    );
}

#[test]
fn night_flag_discarded_when_panel_not_intern() {
    // Panel is truth: when it reports disarmed, a set night-flag is discarded.
    let mut c = core(false);
    c.restore_night_flag(true);
    let a = c.on_frame(0, &decode(OUTSTATUS_DISARMED));
    assert_eq!(c.arm_state(), ArmState::Disarmed);
    assert!(!c.night_flag(), "flag discarded");
    assert!(a.contains(&Action::PersistNightFlag(false)));
}

#[test]
fn sensor_state_published_from_input_block() {
    let mut c = core(false);
    let a = c.on_frame(0, &decode(INSTATUS));
    let p = published(&a, "sensor/eg/essen/bewegung/state").expect("sensor publish for 0x0075");
    assert!(p == "ON" || p == "OFF", "logischer Zustand: {p}");
}

// Master SEND_NORM poll (C=0x40, A=0x02) as a long telegram.
const SEND_NORM: &str = "6802026840024216";

#[test]
fn command_sent_in_poll_window() {
    // The real panel reliably ACKs commands only in the SEND_NORM poll window
    // (the command window after the output telegram collided reproducibly:
    // "TIMEOUT ArmHome: Kollision, Versuche erschöpft").
    let mut c = core(false);
    c.on_frame(0, &decode(READY_DISARMED)); // intern bereit
    let a = c.on_command(10, ArmCommand::ArmHome);
    assert!(
        !a.iter().any(|x| matches!(x, Action::SendFrame(_))),
        "queuing does not send yet"
    );
    let a = c.on_frame(20, &decode(SEND_NORM));
    assert!(
        a.iter()
            .any(|x| matches!(x, Action::SendFrame(f) if f.as_slice() != CONF_ACK)),
        "pending command goes out in the poll window"
    );
    assert!(
        !has_send(&a, &CONF_ACK),
        "the command REPLACES the ACK in the poll window (FT1.2 send discipline)"
    );
}

#[test]
fn unconfirmed_sensor_tracked_but_not_published() {
    // Unconfirmed sensors: track state for the live test board (door-open test for polarity
    // confirmation), but NO MQTT publish (no HA noise).
    let mut s = sensor(0x0075, "eg/essen/bewegung", SensorKind::Bewegungsmelder);
    s.confirmed = false;
    let mut c = Core::new(
        std::sync::Arc::new(cfg(vec![s])),
        CoreOptions {
            disarm_enabled: false,
        },
    );
    let a = c.on_frame(0, &decode(INSTATUS));
    assert!(
        published(&a, "sensor/eg/essen/bewegung/state").is_none(),
        "unconfirmed must not be published"
    );
    assert!(
        c.sensor_states().contains_key(&0x0075),
        "state must still be tracked (live board)"
    );
}

#[test]
fn serial_liveness_goes_unavailable_then_recovers() {
    let mut c = core(false);
    c.on_frame(0, &decode(OUTSTATUS_DISARMED));
    // No frame past the deadline → unavailable.
    let a = c.on_tick(SERIAL_DEADLINE_MS + 1);
    assert_eq!(c.availability(), Availability::Unavailable);
    assert_eq!(published(&a, "availability").as_deref(), Some("offline"));
    assert_eq!(published(&a, "state").as_deref(), Some("unavailable"));

    // Recovery only after hysteresis (RECOVERY_FRAMES valid frames).
    let mut t: Tick = SERIAL_DEADLINE_MS + 2;
    for _ in 0..RECOVERY_FRAMES - 1 {
        c.on_frame(t, &decode(OUTSTATUS_DISARMED));
        assert_eq!(
            c.availability(),
            Availability::Unavailable,
            "not yet recovered"
        );
        t += 100;
    }
    let a = c.on_frame(t, &decode(OUTSTATUS_DISARMED));
    assert_eq!(c.availability(), Availability::Online);
    assert_eq!(published(&a, "availability").as_deref(), Some("online"));
}

#[test]
fn boot_without_any_frame_goes_unavailable_after_deadline() {
    // No frame at all since boot (panel off): availability must not stay optimistically "online".
    let mut c = core(false);
    assert_eq!(
        c.availability(),
        Availability::Online,
        "initial state is online"
    );
    let a = c.on_tick(SERIAL_DEADLINE_MS + 1);
    assert_eq!(c.availability(), Availability::Unavailable);
    assert_eq!(published(&a, "availability").as_deref(), Some("offline"));
}

#[test]
fn disarm_is_fail_closed_by_default() {
    let mut c = core(false); // disarm_enabled = false
    let a = c.on_command(0, ArmCommand::Disarm);
    assert!(
        !a.iter().any(|x| matches!(x, Action::SendFrame(_))),
        "no frame sent"
    );
    assert!(a.iter().any(|x| matches!(x, Action::Log(_))));
    // Nothing sent even in the poll window.
    let a = c.on_frame(0, &decode(OUTSTATUS_INTERN));
    let disarm = encoded(0x0530, 0xE1);
    assert!(!has_send(&a, &disarm), "disarm frame must never go out");
}

#[test]
fn disarm_sent_in_window_when_enabled() {
    let mut c = core(true); // disarm_enabled = true
    c.on_command(0, ArmCommand::Disarm);
    let a = c.on_frame(0, &decode(OUTSTATUS_INTERN));
    assert!(has_send(&a, &CONF_ACK), "erst ACK");
    assert!(
        has_send(&a, &encoded(0x0530, 0xE1)),
        "then disarm in the window"
    );
}

#[test]
fn arm_command_only_sent_in_poll_window_and_not_optimistic() {
    let mut c = core(false);
    // Command is queued, NOT sent immediately.
    let a = c.on_command(0, ArmCommand::ArmHome);
    assert!(!a.iter().any(|x| matches!(x, Action::SendFrame(_))));
    assert_eq!(
        c.arm_state(),
        ArmState::Unknown,
        "command does NOT change state optimistically"
    );

    // Sent only in the inter-poll window (after ACK of a ready status).
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(
        has_send(&a, &encoded(0x0531, 0x62)),
        "arm-home in the window (ready)"
    );
    // State still comes only from the snapshot (still disarmed here).
    assert_eq!(c.arm_state(), ArmState::Disarmed);
}

#[test]
fn arm_night_sets_persisted_flag() {
    let mut c = core(false);
    c.on_command(0, ArmCommand::ArmNight);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(
        has_send(&a, &encoded(0x0531, 0x62)),
        "night = arm-home telegram"
    );
    assert!(c.night_flag(), "night-flag set");
    assert!(a.contains(&Action::PersistNightFlag(true)));
}

#[test]
fn arm_rejected_when_panel_not_ready() {
    // OUTSTATUS_DISARMED: intern_bereit = OFF (open contact) → no arm telegram.
    let mut c = core(false);
    c.on_command(0, ArmCommand::ArmHome);
    let a = c.on_frame(0, &decode(OUTSTATUS_DISARMED));
    assert!(
        !has_send(&a, &encoded(0x0531, 0x62)),
        "not ready → no arm frame"
    );
    assert_eq!(
        published(&a, "command_result").map(|p| p.contains("REJECTED") && p.contains("intern")),
        Some(true)
    );
}

#[test]
fn arm_away_rejected_when_extern_not_ready() {
    // Even in the ready status, arm-away readiness is NOT ready → arm_away rejected.
    let mut c = core(false);
    c.on_command(0, ArmCommand::ArmAway);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(!has_send(&a, &encoded(0x0532, 0x61)), "arm-away not ready");
    assert!(published(&a, "command_result")
        .map(|p| p.contains("extern"))
        .unwrap_or(false));
}

#[test]
fn readiness_is_published_from_status() {
    let mut c = core(false);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert_eq!(published(&a, "ready/intern").as_deref(), Some("yes"));
    assert_eq!(published(&a, "ready/extern").as_deref(), Some("no"));
    // Transition to not-ready is tracked.
    let a = c.on_frame(3_000, &decode(OUTSTATUS_DISARMED));
    assert_eq!(published(&a, "ready/intern").as_deref(), Some("no"));
}

#[test]
fn belegt_response_does_not_corrupt_state() {
    // A 0x24 block with address extension 0x71 (occupancy response) must not publish
    // sensor or arm state — only ACK.
    let mut c = core(false);
    // user_data = [C=73, A=02, record: len=0x0C, type=0x24, device=00, addr=00, extra=00, ext=71, 8x status]
    let user = [
        0x73, 0x02, 0x0C, 0x24, 0x00, 0x00, 0x00, 0x71, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff,
    ];
    let mut buf = [0u8; 64];
    let n = telenot_protocol::encode_frame(&user, &mut buf).unwrap();
    let mut d = FrameDecoder::new();
    d.feed(&buf[..n]);
    let frame = d.next_frame().unwrap().unwrap();

    let a = c.on_frame(0, &frame);
    assert!(has_send(&a, &CONF_ACK), "occupancy response is still ACKed");
    assert!(
        !a.iter().any(|x| matches!(x, Action::Publish { .. })),
        "occupancy response must not publish anything"
    );
}

#[test]
fn panel_command_rejection_is_reported() {
    // CONFIRM_ACK with embedded error record 0x11 (code 0x10) → report to HA.
    let mut c = core(false);
    let a = c.on_frame(0, &decode("680606680002021100102516"));
    let p = published(&a, "command_result").expect("command_result");
    assert!(p.contains("FEHLER"), "rejected command reported: {p}");
}

#[test]
fn central_restart_emits_event() {
    // Spontaneous restart message (0x53, address 0xFFFF).
    let mut c = core(false);
    let a = c.on_frame(
        0,
        &decode("681a1a687302050200ffff015307500514030a0b10390656008051ffffffc916"),
    );
    assert_eq!(published(&a, "event").as_deref(), Some("restart"));
}

fn encoded(addr: u16, art: u8) -> Vec<u8> {
    let mut buf = [0u8; 32];
    let n = encode_command_02(addr, ERW_AUSGAENGE, art, &mut buf).unwrap();
    buf[..n].to_vec()
}

/// Builds a SEND_NDAT frame around arbitrary records (same structure as the panel stream).
fn ndat(records: &[u8]) -> Frame {
    let mut ud = vec![C_SEND_NDAT, 0x73];
    ud.extend_from_slice(records);
    let mut out = vec![0u8; ud.len() + 8];
    let n = encode_frame(&ud, &mut out).expect("frame");
    out.truncate(n);
    let mut d = FrameDecoder::new();
    d.feed(&out[..n]);
    d.next_frame().unwrap().unwrap()
}

/// A 0x02 message record: [record_len][record_type][device, addr_hi, addr_lo, addr_ext, msg_type].
fn meldung_rec(addr: u16, meldungsart: u8) -> Vec<u8> {
    let payload = [
        0x00u8,
        (addr >> 8) as u8,
        (addr & 0xff) as u8,
        0x01,
        meldungsart,
    ];
    let mut v = vec![payload.len() as u8, satztyp::MELDUNG];
    v.extend_from_slice(&payload);
    v
}

#[test]
fn state_republished_after_recovery_to_same_arm_state() {
    let mut c = core(false);
    // Establish DISARMED.
    c.on_frame(0, &decode(OUTSTATUS_DISARMED));
    assert_eq!(c.arm_state(), ArmState::Disarmed);

    // Serial goes silent → unavailable (cache invalidated).
    c.on_tick(SERIAL_DEADLINE_MS + 1);
    assert_eq!(c.availability(), Availability::Unavailable);

    // Recovery (hysteresis) with THE SAME DISARMED status. The fresh snapshot MUST republish
    // state — otherwise HA would stay stuck on retained `state=unavailable`.
    let mut saw_state: Option<String> = None;
    let mut t: Tick = SERIAL_DEADLINE_MS + 2;
    for _ in 0..RECOVERY_FRAMES {
        let a = c.on_frame(t, &decode(OUTSTATUS_DISARMED));
        if let Some(p) = published(&a, "state") {
            saw_state = Some(p);
        }
        t += 100;
    }
    assert_eq!(c.availability(), Availability::Online);
    assert_eq!(
        saw_state.as_deref(),
        Some("DISARMED"),
        "state must be republished after serial recovery (republish bug)"
    );
}

// --- Command transaction (command window, collision, ACK tracking) ---------------------

/// The panel's acknowledgement for our command (identical to the CONF_ACK constant).
fn panel_ack() -> Frame {
    decode("6802026800020216")
}

#[test]
fn command_waits_for_second_status_telegram() {
    // Commands only in the pause AFTER both status telegrams — a pure input telegram
    // (address extension 0x01) does NOT open the window.
    let mut c = core(true);
    c.on_frame(0, &decode(READY_DISARMED)); // establish readiness
    c.on_command(100, ArmCommand::ArmHome);
    let a = c.on_frame(3_000, &decode(INSTATUS));
    assert!(
        !a.iter()
            .any(|x| matches!(x, Action::SendFrame(b) if b != &CONF_ACK.to_vec())),
        "after telegram 1 (inputs) no command must go out"
    );
    let a = c.on_frame(3_100, &decode(READY_DISARMED));
    assert!(
        has_send(&a, &encoded(0x0531, 0x62)),
        "only after telegram 2"
    );
}

#[test]
fn command_acked_publishes_ok() {
    let mut c = core(true);
    c.on_command(0, ArmCommand::ArmHome);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(has_send(&a, &encoded(0x0531, 0x62)));
    let a = c.on_frame(100, &panel_ack());
    assert!(
        published(&a, "command_result").is_some_and(|p| p.starts_with("OK ArmHome")),
        "panel ACK → OK (with latency breakdown)"
    );
}

#[test]
fn command_retried_on_collision_then_fails_visibly() {
    // If a panel telegram arrives instead of an ACK, our command is considered
    // discarded → bounded retry, then visible TIMEOUT.
    let mut c = core(true);
    c.on_command(0, ArmCommand::ArmHome);
    let mut t: Tick = 0;
    // Attempts 1..MAX: each further status telegram (without ACK) = collision + re-send.
    for _ in 0..COMMAND_MAX_ATTEMPTS {
        let a = c.on_frame(t, &decode(READY_DISARMED));
        assert!(
            has_send(&a, &encoded(0x0531, 0x62)),
            "send attempt at t={t}"
        );
        t += 3_000;
    }
    // Attempts exhausted → no further send, but TIMEOUT command_result.
    let a = c.on_frame(t, &decode(READY_DISARMED));
    assert!(!has_send(&a, &encoded(0x0531, 0x62)), "attempts exhausted");
    assert!(
        published(&a, "command_result")
            .map(|p| p.starts_with("TIMEOUT ArmHome"))
            .unwrap_or(false),
        "failure must be visible"
    );
}

#[test]
fn command_resent_after_ack_timeout() {
    let mut c = core(true);
    c.on_command(0, ArmCommand::ArmHome);
    c.on_frame(0, &decode(READY_DISARMED)); // attempt 1
                                            // No ACK within 3 s → identical re-send via tick.
    let a = c.on_tick(COMMAND_ACK_TIMEOUT_MS + 1);
    assert!(has_send(&a, &encoded(0x0531, 0x62)), "timeout retry");
    // Quittung des Re-Sends → OK.
    let a = c.on_frame(COMMAND_ACK_TIMEOUT_MS + 100, &panel_ack());
    assert!(published(&a, "command_result").is_some_and(|p| p.starts_with("OK ArmHome")));
}

#[test]
fn serial_loss_drops_queued_command_fail_safe() {
    // A queued command must NOT fire minutes later after serial recovery.
    let mut c = core(true);
    c.on_frame(0, &decode(INSTATUS)); // establish liveness; window stays closed
    c.on_command(100, ArmCommand::ArmHome);
    let a = c.on_tick(SERIAL_DEADLINE_MS + 1);
    assert!(
        published(&a, "command_result")
            .map(|p| p.starts_with("TIMEOUT ArmHome"))
            .unwrap_or(false),
        "discard must be visible"
    );
    // After recovery: no late send.
    let mut t = SERIAL_DEADLINE_MS + 2;
    for _ in 0..=RECOVERY_FRAMES {
        let a = c.on_frame(t, &decode(READY_DISARMED));
        assert!(!has_send(&a, &encoded(0x0531, 0x62)), "no ghost arm");
        t += 100;
    }
}

#[test]
fn panel_fehler_in_ack_resolves_transaction_without_ok() {
    // CONFIRM_ACK with embedded 0x11 = command rejected → FEHLER, no OK.
    let mut c = core(true);
    c.on_command(0, ArmCommand::ArmHome);
    c.on_frame(0, &decode(READY_DISARMED));
    let a = c.on_frame(100, &decode("680606680002021100102516"));
    let p = published(&a, "command_result").expect("command_result");
    assert!(p.contains("FEHLER"), "rejection reported: {p}");
    assert!(!p.starts_with("OK"), "no OK on error record");
    // Transaction is complete — a later tick must not re-send anything.
    let a = c.on_tick(COMMAND_ACK_TIMEOUT_MS + 200);
    assert!(!a.iter().any(|x| matches!(x, Action::SendFrame(_))));
}

#[test]
fn stoerung_meldung_published_not_dropped() {
    let mut c = core(false);
    // StoerungAkku active (message type 0x33, bit7=0 → on).
    let a = c.on_frame(0, &ndat(&meldung_rec(0x0103, 0x33)));
    assert_eq!(
        published(&a, "diag/akku").as_deref(),
        Some("ON"),
        "fault message must not be silently discarded"
    );
    assert!(
        published(&a, "event").is_some(),
        "also as spontaneous event"
    );

    // Withdrawal (bit7=1 → 0xB3) → OFF.
    let a = c.on_frame(100, &ndat(&meldung_rec(0x0103, 0xB3)));
    assert_eq!(published(&a, "diag/akku").as_deref(), Some("OFF"));
}

// --- Set time (VdS 2465 record type 0x50) ----------------------------------------------

#[test]
fn set_time_sent_in_window_and_acked() {
    use telenot_protocol::{encode_set_datetime, DateTime};
    let dt = DateTime {
        jahr: 26,
        jh_or_weekday: 3,
        monat: 6,
        tag: 11,
        stunde: 14,
        minute: 30,
        sekunde: 5,
    };
    let mut c = core(false);
    let a = c.on_set_time(0, dt);
    assert!(
        !a.iter().any(|x| matches!(x, Action::SendFrame(_))),
        "only in the window"
    );

    let mut buf = [0u8; 32];
    let n = encode_set_datetime(&dt, &mut buf).unwrap();
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(
        has_send(&a, &buf[..n]),
        "0x50 telegram in the command window"
    );

    let a = c.on_frame(100, &panel_ack());
    assert!(published(&a, "command_result").is_some_and(|p| p.starts_with("OK SetTime")));
}

#[test]
fn set_time_yields_to_pending_arm_command() {
    use telenot_protocol::DateTime;
    let dt = DateTime {
        jahr: 26,
        jh_or_weekday: 0,
        monat: 1,
        tag: 1,
        stunde: 0,
        minute: 0,
        sekunde: 0,
    };
    let mut c = core(false);
    c.on_command(0, ArmCommand::ArmHome);
    c.on_set_time(10, dt); // must yield to the arm command
    let a = c.on_frame(100, &decode(READY_DISARMED));
    assert!(has_send(&a, &encoded(0x0531, 0x62)), "arm has priority");
    let sends = a
        .iter()
        .filter(|x| matches!(x, Action::SendFrame(b) if b != &CONF_ACK.to_vec()))
        .count();
    assert_eq!(sends, 1, "exactly ONE command per window");
}

// --- Detection area bypass (0x51/0xD1 @ 0x05F0+) -------------------------------------

#[test]
fn bypass_sperren_fail_closed_without_optin() {
    // Bypassing reduces security → rejected without master switch (disarm_enabled).
    let mut c = core(false);
    c.on_bypass(0, 3, true);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(!has_send(&a, &encoded(0x05F2, 0x51)), "fail-closed");
    // Unbypassing increases security → always allowed.
    c.on_bypass(100, 3, false);
    let a = c.on_frame(3_000, &decode(READY_DISARMED));
    assert!(has_send(&a, &encoded(0x05F2, 0xD1)), "unbypass allowed");
}

#[test]
fn bypass_sent_in_window_and_acked() {
    // Bypass detection area 3 = address 0x05F2, message type 0x51.
    let mut c = core(true);
    c.on_bypass(0, 3, true);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(has_send(&a, &encoded(0x05F2, 0x51)));
    let a = c.on_frame(100, &panel_ack());
    assert!(published(&a, "command_result").is_some_and(|p| p.starts_with("OK BypassOn MB3")));
}

#[test]
fn bypass_validates_mb_range() {
    let mut c = core(true);
    c.on_bypass(0, 0, true);
    c.on_bypass(0, 129, true);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(
        !a.iter()
            .any(|x| matches!(x, Action::SendFrame(b) if b != &CONF_ACK.to_vec())),
        "detection area outside 1–128 must never be sent"
    );
}

#[test]
fn mb_bypassed_readback_from_real_snapshot() {
    // The real panel reports detection area 8 bypassed (bit 0x05F7 = '0') — first observation ON.
    let mut c = core(false);
    let a = c.on_frame(0, &decode(OUTSTATUS_DISARMED));
    assert_eq!(published(&a, "mb/8/bypassed").as_deref(), Some("ON"));
    // Non-bypassed areas generate NO boot flood.
    assert!(published(&a, "mb/1/bypassed").is_none());
    assert_eq!(c.mb_bypassed().get(&8), Some(&true));

    // Unbypass in the next snapshot → OFF transition is published.
    let mut unlocked = decode(OUTSTATUS_DISARMED).user_data().to_vec();
    // 0x05F7 relative to 0x0500 = 0xF7 → status byte 30, bit 7. Status starts after
    // [C,A, len,type, device,hi,lo,ext] = offset 8 in user_data.
    unlocked[8 + 30] |= 0x80;
    let mut out = vec![0u8; unlocked.len() + 8];
    let n = encode_frame(&unlocked, &mut out).unwrap();
    let mut d = FrameDecoder::new();
    d.feed(&out[..n]);
    let frame = d.next_frame().unwrap().unwrap();
    let a = c.on_frame(3_000, &frame);
    assert_eq!(published(&a, "mb/8/bypassed").as_deref(), Some("OFF"));
}

// --- Switch outputs (0x00/0x80) --------------------------------------------------------

/// Core with one enabled (`switchable`) transistor output 1 (0x0515).
fn core_with_switchable_output() -> Core {
    let mut out = sensor(0x0515, "keller/relais", SensorKind::Signalgeber);
    out.switchable = true;
    Core::new(
        std::sync::Arc::new(cfg(vec![
            sensor(0x0075, "eg/essen/bewegung", SensorKind::Bewegungsmelder),
            out,
        ])),
        CoreOptions {
            disarm_enabled: false,
        },
    )
}

#[test]
fn output_fail_closed_without_allowlist() {
    // 0x0516 not in config — and 0x0075 is not a switchable output.
    let mut c = core_with_switchable_output();
    c.on_output(0, 0x0516, true);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    assert!(
        !a.iter()
            .any(|x| matches!(x, Action::SendFrame(b) if b != &CONF_ACK.to_vec())),
        "non-allowlisted output must never be switched"
    );
}

#[test]
fn output_command_matches_reference_telegram() {
    // Switch on transistor output 1 (0x0515) — telegram byte-identical to captured reference telegrams.
    let mut c = core_with_switchable_output();
    c.on_output(0, 0x0515, true);
    let a = c.on_frame(0, &decode(READY_DISARMED));
    let reference = [
        0x68, 0x09, 0x09, 0x68, 0x73, 0x01, 0x05, 0x02, 0x00, 0x05, 0x15, 0x02, 0x00, 0x97, 0x16,
    ];
    assert!(has_send(&a, &reference), "reference output-on telegram");
    let a = c.on_frame(100, &panel_ack());
    assert!(published(&a, "command_result").is_some_and(|p| p.starts_with("OK OutputOn 0x0515")));
    // Switch off: message type 0x80.
    c.on_output(200, 0x0515, false);
    let a = c.on_frame(3_000, &decode(READY_DISARMED));
    assert!(has_send(&a, &encoded(0x0515, 0x80)), "output off");
}

#[test]
fn pending_fails_visibly_when_command_window_never_opens() {
    // Misbehaving panel: only input telegrams (ext 0x01), never outputs → command window
    // never opens. The command must not stall silently.
    let mut c = core(false);
    c.on_command(0, ArmCommand::ArmHome);
    for t in [3_000u64, 6_000, 9_000] {
        let a = c.on_frame(t, &decode(INSTATUS));
        assert!(
            !a.iter()
                .any(|x| matches!(x, Action::SendFrame(b) if b != &CONF_ACK.to_vec())),
            "input telegram does not open the window"
        );
    }
    // Serial is alive (last frame at 9 000), but pending is older than the limit.
    let a = c.on_tick(telenot_core::PENDING_MAX_AGE_MS + 1);
    assert!(
        published(&a, "command_result")
            .map(|p| p.starts_with("TIMEOUT ArmHome"))
            .unwrap_or(false),
        "starved command must fail visibly"
    );
}

// ---- Eager-Send: fire immediately on an idle line, fall back to the poll slot on collision ----

#[test]
fn eager_send_fires_immediately_when_line_idle() {
    // eager ON + line idle past the guard → the command goes out at queue time, without
    // waiting for a SEND_NORM poll window.
    let mut c = core_eager(false);
    c.on_frame(0, &decode(READY_DISARMED)); // intern bereit; last_frame_at = 0
    let a = c.on_command(EAGER_IDLE_GUARD_MS + 100, ArmCommand::ArmHome);
    assert!(
        a.iter()
            .any(|x| matches!(x, Action::SendFrame(f) if f.as_slice() != CONF_ACK)),
        "eager: command is sent immediately once the line is idle"
    );
}

#[test]
fn eager_send_guard_blocks_right_after_frame() {
    // eager ON but a frame just arrived → too close to a possible panel burst → NOT sent yet;
    // it still goes out in the next SEND_NORM window (fallback intact).
    let mut c = core_eager(false);
    c.on_frame(0, &decode(READY_DISARMED));
    let a = c.on_command(10, ArmCommand::ArmHome); // 10 ms < guard
    assert!(
        !a.iter().any(|x| matches!(x, Action::SendFrame(_))),
        "eager guard: too soon after a frame → queued, not sent"
    );
    let a = c.on_frame(20, &decode(SEND_NORM));
    assert!(
        a.iter()
            .any(|x| matches!(x, Action::SendFrame(f) if f.as_slice() != CONF_ACK)),
        "still delivered in the poll window"
    );
}

#[test]
fn eager_collision_falls_back_to_slot() {
    // eager fires, then the panel polls (SEND_NORM) before ACKing → treated as collision →
    // requeued and RE-SENT in the reliable slot. Nothing is lost.
    let mut c = core_eager(false);
    c.on_frame(0, &decode(READY_DISARMED));
    let a = c.on_command(EAGER_IDLE_GUARD_MS + 100, ArmCommand::ArmHome);
    assert!(
        a.iter()
            .any(|x| matches!(x, Action::SendFrame(f) if f.as_slice() != CONF_ACK)),
        "eager attempt fired"
    );
    let a = c.on_frame(EAGER_IDLE_GUARD_MS + 200, &decode(SEND_NORM));
    assert!(
        a.iter()
            .any(|x| matches!(x, Action::SendFrame(f) if f.as_slice() != CONF_ACK)),
        "collision fallback: command re-sent in the SEND_NORM slot"
    );
}

#[test]
fn eager_disabled_is_todays_behavior() {
    // eager OFF (via `core`) → strict poll-window behavior even long after the last frame.
    let mut c = core(false);
    c.on_frame(0, &decode(READY_DISARMED));
    let a = c.on_command(1_000, ArmCommand::ArmHome); // way past the guard, but eager is off
    assert!(
        !a.iter().any(|x| matches!(x, Action::SendFrame(_))),
        "eager off → command stays poll-gated"
    );
    let a = c.on_frame(1_010, &decode(SEND_NORM));
    assert!(
        a.iter()
            .any(|x| matches!(x, Action::SendFrame(f) if f.as_slice() != CONF_ACK)),
        "delivered in the poll window"
    );
}

//! `telenot-sim serve` — host daemon: serves the real setup web + REST against the real
//! `telenot-core`, WITHOUT an ESP32. A dedicated EMA thread owns `Runtime` + `Transport`
//! (mock/replay/tcp) and publishes a live snapshot; the HTTP threads (tiny_http) only read
//! the snapshot or submit intents — the core is never called synchronously from HTTP.

use std::collections::VecDeque;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use telenot_app::api::dispatch;
use telenot_app::app::{ConnCheckView, MqttTestView, TestState};
use telenot_app::dto::DeviceInfo;
use telenot_app::mqtt::{MqttSink, MqttTarget, MqttTestResult};
use telenot_app::{
    authorize_command, ApiRequest, App, CommandAuth, InMemoryServices, Intent, Method, Runtime,
};
use telenot_config::Config;
use telenot_core::Action;
use telenot_protocol::{
    encode_frame, satztyp, FrameDecoder, ABFRAGE_BELEGT, ABFRAGE_TEXT, A_QUERY, C_SEND_NDAT,
    C_SEND_NORM, ERW_BELEGT_AUSGAENGE, ERW_BELEGT_EINGAENGE, ERW_TEXT,
};

use rumqttc::{Client, Event, Incoming, MqttOptions, QoS};

use crate::Transport;

// ───────────────────────── Mock frame synthesis ─────────────────────────

/// VdS-2465 record: `[record_len=payload.len()][record_type][payload…]`.
fn record(satztyp: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 2);
    v.push(payload.len() as u8);
    v.push(satztyp);
    v.extend_from_slice(payload);
    v
}

/// SEND_NDAT frame around arbitrary records (same 2-byte header as `encode_command_02`).
fn ndat(records: &[u8]) -> Vec<u8> {
    let mut ud = vec![C_SEND_NDAT, 0x73];
    ud.extend_from_slice(records);
    let mut out = vec![0u8; ud.len() + 8];
    let n = encode_frame(&ud, &mut out).expect("frame");
    out.truncate(n);
    out
}

/// SEND_NORM poll (master: "slave, you may send") — the send window in which the slave may
/// place queries/commands. Without this window the runtime (FT1.2-faithful) never sends.
fn send_norm_poll() -> Vec<u8> {
    let ud = [C_SEND_NORM, A_QUERY];
    let mut out = vec![0u8; ud.len() + 8];
    let n = encode_frame(&ud, &mut out).expect("frame");
    out.truncate(n);
    out
}

/// 0x24 block status record. payload = [device, address, addr_extra, ext, status…].
fn block(erw: u8, adresse: u8, adressenzusatz: u8, status: &[u8]) -> Vec<u8> {
    let mut p = vec![0x00, adresse, adressenzusatz, erw];
    p.extend_from_slice(status);
    record(satztyp::BLOCKSTATUS, &p)
}

/// Simulated arm state of the mock panel (set by command frames).
#[derive(Clone, Copy, PartialEq)]
enum MockArm {
    Disarmed,
    Intern,
    Extern,
}

/// Occupancy response for inputs (0x71): '0'-bit = occupied, for each seed address < 0x0500.
fn belegt_inputs(seed: &[(u16, String)]) -> Vec<u8> {
    let mut s = vec![0xFFu8; 160]; // covers 0x0000..0x0500
    for (a, _) in seed.iter().filter(|(a, _)| *a < 0x0500) {
        let off = *a as usize;
        s[off / 8] &= !(1 << (off % 8));
    }
    ndat(&block(ERW_BELEGT_EINGAENGE, 0x00, 0x00, &s))
}

/// Occupancy response for outputs (0x72): seed addresses >= 0x0500.
fn belegt_outputs(seed: &[(u16, String)]) -> Vec<u8> {
    let mut s = vec![0xFFu8; 64]; // covers 0x0500..0x0700
    for (a, _) in seed.iter().filter(|(a, _)| *a >= 0x0500) {
        let off = (*a - 0x0500) as usize;
        s[off / 8] &= !(1 << (off % 8));
    }
    ndat(&block(ERW_BELEGT_AUSGAENGE, 0x05, 0x00, &s))
}

/// Text response: 0x0C (address) + 0x54 (name) in a SEND_NDAT.
fn text_response(addr: u16, name: &str) -> Vec<u8> {
    let bm = record(
        satztyp::BEREICH_MELDEBEREICH,
        &[
            0x00,
            (addr >> 8) as u8,
            (addr & 0xFF) as u8,
            0x00,
            0x01,
            0x00,
        ],
    );
    let ascii = record(satztyp::ASCII, name.as_bytes());
    let mut recs = bm;
    recs.extend_from_slice(&ascii);
    ndat(&recs)
}

/// Realistic demo seed: ~240 detectors across many rooms/types (for an impressive scan result),
/// plus two switch outputs. Seed[0] is the "intrusion" detector that opens on alarm (see sensor_open).
fn default_seed() -> Vec<(u16, String)> {
    const ROOMS: [&str; 30] = [
        "Terrace Door",
        "Front Door",
        "Vestibule",
        "Living Room",
        "Dining Room",
        "Kitchen",
        "Hallway GF",
        "Guest WC",
        "Office",
        "Library",
        "Bedroom",
        "Dressing Room",
        "Bath UF",
        "Kids 1",
        "Kids 2",
        "Hallway UF",
        "Guest Room",
        "Loft Studio",
        "Attic",
        "Basement Hall",
        "Boiler Room",
        "Hobby Room",
        "Wine Cellar",
        "Laundry",
        "Garage",
        "Carport",
        "Conservatory",
        "Balcony",
        "Garden Door",
        "Workshop",
    ];
    const TYPES: [&str; 5] = ["Contact", "Motion", "Smoke", "Glass", "Leak"];
    let mut seed: Vec<(u16, String)> = Vec::with_capacity(242);
    let mut addr: u16 = 0x0040;
    let mut i = 0usize;
    while seed.len() < 240 {
        let room = ROOMS[i % ROOMS.len()];
        let ty = TYPES[(i / ROOMS.len()) % TYPES.len()];
        let round = i / (ROOMS.len() * TYPES.len()) + 1;
        let name = if round > 1 {
            format!("{ty} {room} {round}")
        } else {
            format!("{ty} {room}")
        };
        seed.push((addr, name));
        addr += 1;
        if i % 7 == 6 {
            addr += 1; // occasional address gaps → more realistic
        }
        i += 1;
    }
    // Outputs (≥ 0x0500): switch output demo (switchable allowlist + HA switch entity).
    seed.push((0x0515, "Relay Basement".to_string()));
    seed.push((0x050C, "Siren".to_string()));
    seed
}

/// Synthetische EMA: liefert periodischen Status + beantwortet Belegt-/Text-Abfragen.
struct MockTransport {
    seed: Vec<(u16, String)>,
    pending: VecDeque<Vec<u8>>,
    dec: FrameDecoder,
    arm: MockArm,
    /// Bypassed detection areas (1–128) — mirrored from received 0x51/0xD1 commands.
    bypassed: std::collections::BTreeSet<u8>,
    /// Switched-on outputs — mirrored from received 0x00/0x80 commands.
    outputs_on: std::collections::BTreeSet<u16>,
    /// Sim-only: "intrusion" triggered (via POST /sim/intrude) → when armed ⇒ alarm (0x0533).
    alarm: Arc<AtomicBool>,
    tick: u64,
}

impl MockTransport {
    fn new(seed: Vec<(u16, String)>, alarm: Arc<AtomicBool>) -> Self {
        MockTransport {
            seed,
            pending: VecDeque::new(),
            dec: FrameDecoder::new(),
            arm: MockArm::Disarmed,
            bypassed: std::collections::BTreeSet::new(),
            outputs_on: std::collections::BTreeSet::new(),
            alarm,
            tick: 0,
        }
    }

    /// Input status: normally all closed; on simulated intrusion the intrusion detector opens.
    fn status_input(&self) -> Vec<u8> {
        let mut s = vec![0xFFu8; 64];
        for (i, (a, _)) in self.seed.iter().enumerate() {
            if *a < 0x0500 && self.sensor_open(i) {
                let off = *a as usize;
                s[off / 8] &= !(1 << (off % 8)); // '0' = active/open
            }
        }
        ndat(&block(0x01, 0x00, 0x00, &s))
    }

    fn sensor_open(&self, i: usize) -> bool {
        // Clean demo: everything normally closed; on simulated intrusion (POST /sim/intrude)
        // the "intrusion" detector (Seed[0], "Contact Terrace Door") opens → alarm hero shows it.
        i == 0 && self.alarm.load(Ordering::Relaxed)
    }

    /// Output status: area bits according to the simulated arm state ('0' = active).
    /// Byte 6 from base 0x0500: bit0=disarmed, 1=arm-home, 2=arm-away, 5=home-ready, 6=away-ready.
    fn status_output(&self) -> Vec<u8> {
        let b = match self.arm {
            MockArm::Disarmed => 0x9E, // disarmed + ready (0xFF ^ 0b0110_0001)
            MockArm::Intern => 0x9D,   // arm-home  + ready (0xFF ^ 0b0110_0010)
            MockArm::Extern => 0x9B,   // arm-away  + ready (0xFF ^ 0b0110_0100)
        };
        // Status bits for the COMPLETE output address space — size derived from the
        // central constant rather than maintained as a magic number.
        const OUT_BASE: u16 = telenot_config::OUTPUT_ADDR_RANGE.start;
        const OUT_BYTES: usize = (telenot_config::OUTPUT_ADDR_RANGE.end - OUT_BASE) as usize / 8;
        let mut s = [0xFFu8; OUT_BYTES];
        s[6] = b;
        // Alarm (0x0533): when simulated intrusion + armed, set the bit active ('0') →
        // the core derives ArmState::Triggered from it.
        if self.alarm.load(Ordering::Relaxed) && self.arm != MockArm::Disarmed {
            let off = (telenot_core::ADDR_ALARM - OUT_BASE) as usize;
            s[off / 8] &= !(1 << (off % 8));
        }
        for &mb in &self.bypassed {
            let off = (telenot_core::ADDR_MB_GESPERRT - OUT_BASE) as usize + (mb as usize - 1);
            s[off / 8] &= !(1 << (off % 8)); // '0' = bypassed
        }
        for &addr in &self.outputs_on {
            let off = (addr - OUT_BASE) as usize;
            s[off / 8] &= !(1 << (off % 8)); // '0' = on
        }
        ndat(&block(0x02, 0x05, 0x00, &s))
    }
}

impl Transport for MockTransport {
    fn read_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        self.tick = self.tick.wrapping_add(1);
        // Poll FIRST: opens the slave send window (the runtime sends queries/commands only on
        // SEND_NORM). If a queued ACK/query-response follows (after a just-received command/query),
        // send ONLY that — an additional unsolicited status SEND would otherwise collide with
        // the command ACK ("TIMEOUT: Kollision, Versuche erschöpft").
        // Otherwise the periodic status (keeps the core alive + carries arm/detector state).
        let mut out = send_norm_poll();
        if self.pending.is_empty() {
            out.extend(self.status_input());
            out.extend(self.status_output());
        } else {
            while let Some(r) = self.pending.pop_front() {
                out.extend(r);
            }
        }
        Ok(Some(out))
    }

    fn write_frame(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.dec.feed(bytes);
        while let Some(Ok(frame)) = self.dec.next_frame() {
            // The panel ACKs every received data telegram. Without this ACK the core's
            // command transaction would run into retry/TIMEOUT.
            if frame.function() == Some(telenot_protocol::Function::SendNdat) {
                self.pending
                    .push_back(telenot_protocol::CONF_ACK_FRAME.to_vec());
            }
            for rec in frame.records().filter_map(|r| r.ok()) {
                if rec.satztyp == 0x10 && rec.payload.len() >= 5 {
                    // Query (occupancy/text) → queue the matching response.
                    let p = rec.payload;
                    let addr = ((p[1] as u16) << 8) | p[2] as u16;
                    let (erw, abfrage) = (p[3], p[4]);
                    if abfrage == ABFRAGE_BELEGT && erw == ERW_BELEGT_EINGAENGE {
                        self.pending.push_back(belegt_inputs(&self.seed));
                        self.pending.push_back(belegt_outputs(&self.seed));
                    } else if abfrage == ABFRAGE_TEXT && erw == ERW_TEXT {
                        if let Some((_, name)) = self.seed.iter().find(|(a, _)| *a == addr) {
                            self.pending.push_back(text_response(addr, name));
                        }
                    }
                } else if let Some(m) = rec.as_meldung() {
                    // Command (0x02) → mirror into simulated state, by message type.
                    use telenot_core::{ADDR_MB_GESPERRT, MB_MAX};
                    use telenot_protocol::{
                        ART_AUSGANG_AUS, ART_AUSGANG_EIN, ART_EXTERN_SCHARF, ART_INTERN_SCHARF,
                        ART_MB_ENTSPERREN, ART_MB_SPERREN, ART_UNSCHARF,
                    };
                    match m.meldungsart {
                        ART_INTERN_SCHARF => self.arm = MockArm::Intern,
                        ART_EXTERN_SCHARF => self.arm = MockArm::Extern,
                        ART_UNSCHARF => {
                            self.arm = MockArm::Disarmed;
                            self.alarm.store(false, Ordering::Relaxed); // disarm ends the alarm
                        }
                        ART_MB_SPERREN | ART_MB_ENTSPERREN => {
                            // Bypass/unbypass detection area @ 0x05F0+(N-1).
                            let addr = m.address();
                            let range = ADDR_MB_GESPERRT..ADDR_MB_GESPERRT + MB_MAX as u16;
                            if range.contains(&addr) {
                                let mb = (addr - ADDR_MB_GESPERRT) as u8 + 1;
                                if m.meldungsart == ART_MB_SPERREN {
                                    self.bypassed.insert(mb);
                                } else {
                                    self.bypassed.remove(&mb);
                                }
                            }
                        }
                        ART_AUSGANG_EIN | ART_AUSGANG_AUS => {
                            // Output on/off → mirror into status.
                            let addr = m.address();
                            if telenot_config::OUTPUT_ADDR_RANGE.contains(&addr) {
                                if m.meldungsart == ART_AUSGANG_EIN {
                                    self.outputs_on.insert(addr);
                                } else {
                                    self.outputs_on.remove(&addr);
                                }
                            }
                        }
                        _ => {} // 0x52 reset etc.: no arm effect in the mock
                    }
                }
            }
        }
        Ok(())
    }

    fn is_live(&self) -> bool {
        true
    }
}

/// Replay of a capture that loops back to the start at end-of-file (keeps the daemon "alive").
struct LoopReplay {
    data: Vec<u8>,
    pos: usize,
}

impl Transport for LoopReplay {
    fn read_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.data.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if self.pos >= self.data.len() {
            self.pos = 0; // loop
        }
        let end = (self.pos + 256).min(self.data.len());
        let out = self.data[self.pos..end].to_vec();
        self.pos = end;
        Ok(Some(out))
    }
    fn write_frame(&mut self, _bytes: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
    fn is_live(&self) -> bool {
        true
    }
}

// ───────────────────────── MQTT sink (stub) ──────────────────────────

/// Host MQTT sink (plain-text). With broker → real publishes via rumqttc (auto-reconnect in
/// background thread); without → stdout only. `test_connection` always does a real probe.
struct HostSink {
    root: String,
    client: Option<Client>,
}

impl MqttSink for HostSink {
    fn publish(&mut self, topic: &str, payload: &str, retain: bool) {
        let full = format!("{}/{topic}", self.root);
        if let Some(c) = &self.client {
            let _ = c.try_publish(
                full.clone(),
                QoS::AtLeastOnce,
                retain,
                payload.as_bytes().to_vec(),
            );
        }
        let r = if retain { " (retain)" } else { "" };
        println!("  PUB {full} = {payload}{r}");
    }
    fn publish_raw(&mut self, topic: &str, payload: &str, retain: bool) {
        if let Some(c) = &self.client {
            let _ = c.try_publish(
                topic.to_string(),
                QoS::AtLeastOnce,
                retain,
                payload.as_bytes().to_vec(),
            );
        }
    }
    fn test_connection(&mut self, target: &MqttTarget) -> MqttTestResult {
        probe_mqtt(target)
    }
}

/// Publishes all HA discovery configs (retained) → HA creates the entities automatically.
fn publish_discovery(sink: &mut HostSink, config: &Config, controllable: bool) {
    let mut opts = telenot_app::DiscoveryOpts::new(sink.root.clone());
    opts.controllable = controllable; // controllable alarm_control_panel only with --mqtt-commands
    opts.areas = telenot_app::areas_from_config(config); // hiplex: per-area entities
    let msgs = telenot_app::discovery_messages(config, &opts);
    let n = msgs.len();
    for m in msgs {
        sink.publish_raw(&m.topic, &m.payload, m.retain);
    }
    eprintln!("  HA-Discovery: {n} Entities publiziert (homeassistant/…)");
}

/// Publishes the retained entity manifest (`inventory`) for non-HA consumers. Above the
/// chunking threshold: envelope on `inventory` + `inventory/chunk/<i>` (retained), one chunk
/// string at a time. `last_chunks` remembers the previous chunk count so a shrinking
/// inventory clears its orphaned retained chunk topics (empty payload); chunks from before
/// a daemon restart are unknown and stay — accepted per contract.
fn publish_inventory(sink: &mut HostSink, config: &Config, last_chunks: &mut usize) {
    let mut chunks = 0usize;
    telenot_app::inventory_for_each(config, &mut |topic, payload| {
        sink.publish(topic, payload, true);
        if topic != "inventory" {
            chunks += 1;
        }
    });
    for i in chunks..*last_chunks {
        sink.publish(&format!("inventory/chunk/{i}"), "", true);
    }
    *last_chunks = chunks;
}

/// Builds the retained diagnostics JSON for the `diagnostics` topic from the live snapshot.
fn diagnostics_payload(app: &Arc<Mutex<App>>, now: u64) -> String {
    let a = app.lock().unwrap();
    let online = a.live.availability == "online";
    serde_json::json!({
        "serial": { "status": if online { "ok" } else { "stale" }, "last_frame_ms": a.live.last_frame_ms_ago },
        "uptime_s": now / 1000,
        "firmware_version": env!("CARGO_PKG_VERSION"),
        "schema_version": telenot_config::CURRENT_SCHEMA_VERSION,
        "reconnects": 0,
    })
    .to_string()
}

fn mqtt_options(
    id: &str,
    host: &str,
    port: u16,
    user: Option<&str>,
    pass: Option<&str>,
) -> MqttOptions {
    let mut opts = MqttOptions::new(id, host.to_string(), port);
    opts.set_keep_alive(Duration::from_secs(15));
    if let Some(u) = user {
        if !u.is_empty() {
            opts.set_credentials(u.to_string(), pass.unwrap_or("").to_string());
        }
    }
    opts
}

/// One-shot plain-text connection probe (runs in a worker thread, NEVER in the serial owner).
fn probe_mqtt(t: &MqttTarget) -> MqttTestResult {
    if t.host.trim().is_empty() {
        return MqttTestResult::Connect;
    }
    if t.tls {
        eprintln!("  (MQTT-Test: TLS auf dem Host nicht verdrahtet — Probe als Klartext)");
    }
    let opts = mqtt_options(
        "telenot-probe",
        &t.host,
        t.port,
        Some(&t.username),
        t.password.as_deref(),
    );
    let (client, mut conn) = Client::new(opts, 10);
    let mut result = MqttTestResult::Connect;
    for ev in conn.iter().take(50) {
        match ev {
            Ok(Event::Incoming(Incoming::ConnAck(ack))) => {
                result = if ack.code == rumqttc::ConnectReturnCode::Success {
                    MqttTestResult::Ok
                } else {
                    MqttTestResult::Auth
                };
                break;
            }
            Err(e) => {
                let s = e.to_string().to_lowercase();
                result =
                    if s.contains("auth") || s.contains("not authorized") || s.contains("password")
                    {
                        MqttTestResult::Auth
                    } else {
                        MqttTestResult::Connect
                    };
                break;
            }
            _ => {}
        }
    }
    let _ = client.disconnect();
    result
}

// ───────────────────────── Daemon ────────────────────────────────────

/// Creates an app in its initial (freshly seeded) state. Shared by `run` (start) and
/// the sim-only `/sim/reset` (e2e isolation) so both look identical.
fn seeded_app(
    device: DeviceInfo,
    config: &Config,
    pin: &Option<String>,
    remote_disarm: bool,
) -> App {
    let mut app = App::new(device, Box::new(InMemoryServices::new("telenot-setup")));
    // Mirror the boot config as the persisted state (same Arc semantics as the firmware).
    app.persisted = Arc::new(config.clone());
    if !config.sensors.is_empty() {
        app.setup.seed(config.clone(), Default::default());
    }
    // Fake slots for the update panel (firmware mirrors real EspOta data here).
    let slot = telenot_app::SlotInfo {
        label: "ota_0".into(),
        state: "valid".into(),
        version: Some(env!("CARGO_PKG_VERSION").into()),
    };
    app.ota_slots = Some((slot.clone(), slot));
    // CLI convenience for live tests WITHOUT the wizard: --pin seeds the disarm PIN and enables
    // remote disarm; --enable-disarm only activates the master switch (disarm still requires a
    // PIN → stays fail-closed). The actual gate always checks app.setup + services.
    if let Some(p) = pin {
        app.services.set_pin(p);
    }
    app.setup.remote_disarm = remote_disarm;
    app
}

/// Factory for a fresh initial app (for `/sim/reset`). `Send + Sync` because it is shared
/// with the HTTP handler.
type ResetFactory = Arc<dyn Fn() -> App + Send + Sync>;

pub fn run(args: &[String]) -> std::io::Result<()> {
    let ema = crate::flag_value(args, "--ema").unwrap_or_else(|| "mock".into());
    let bind = crate::flag_value(args, "--bind").unwrap_or_else(|| "127.0.0.1:8443".into());
    let web_path = crate::flag_value(args, "--web").unwrap_or_else(|| "web/dist/index.html".into());
    let config_path = crate::flag_value(args, "--config");
    let enable_disarm = args.iter().any(|a| a == "--enable-disarm");
    let pin = crate::flag_value(args, "--pin");
    let disarm_enabled = enable_disarm || pin.is_some();
    let mqtt_broker = crate::flag_value(args, "--mqtt");
    let mqtt_user = crate::flag_value(args, "--mqtt-user");
    let mqtt_pass = crate::flag_value(args, "--mqtt-pass");
    let mqtt_tls = args.iter().any(|a| a == "--mqtt-tls");
    let ha_discovery = !args.iter().any(|a| a == "--no-ha-discovery");
    let mqtt_commands = args.iter().any(|a| a == "--mqtt-commands");

    let config = match &config_path {
        Some(p) => Config::from_json(&std::fs::read(p)?).expect("config valid"),
        None => Config::default(),
    };

    // Sim-only control flag: "intrusion" (alarm) via POST /sim/intrude — shared between
    // HTTP handler and MockTransport. Host sim only, never in the firmware.
    let sim_alarm = Arc::new(AtomicBool::new(false));
    let transport: Box<dyn Transport + Send> = build_ema(&ema, &sim_alarm)?;

    let device = DeviceInfo {
        model: "EMA-Bridge (telenot-sim serve)".into(),
        fw: env!("CARGO_PKG_VERSION").into(),
        fw_build: String::new(),
        serial: "HOST-DEV".into(),
        mac: "02:00:00:00:00:01".into(),
        ip: format!("http://{bind}"),
        schema: telenot_config::CURRENT_SCHEMA_VERSION,
        fingerprint: "—".into(),
        configured: !config.sensors.is_empty(),
    };

    // App initial state as factory: both start AND the sim-only reset (/sim/reset, for e2e
    // isolation) produce the same fresh state through it.
    let seed_device = device.clone();
    let seed_config = config.clone();
    let seed_pin = pin.clone();
    let seed_remote = enable_disarm || pin.is_some();
    let make_app: ResetFactory =
        Arc::new(move || seeded_app(seed_device.clone(), &seed_config, &seed_pin, seed_remote));
    let app = Arc::new(Mutex::new(make_app()));

    let html = std::fs::read(&web_path).unwrap_or_else(|_| fallback_html(&web_path).into_bytes());
    let html = Arc::new(html);

    // EMA thread (serial owner): owns Runtime + Transport.
    let app_ema = Arc::clone(&app);
    let cfg_path = config_path.clone();
    let broker = mqtt_broker.clone();
    std::thread::spawn(move || {
        ema_loop(
            transport,
            config,
            pin,
            enable_disarm,
            cfg_path,
            broker,
            mqtt_user,
            mqtt_pass,
            mqtt_tls,
            ha_discovery,
            mqtt_commands,
            app_ema,
        )
    });

    eprintln!("== telenot-sim serve ==");
    eprintln!("  EMA:    {ema}");
    eprintln!(
        "  MQTT:   {}",
        mqtt_broker.as_deref().unwrap_or("stub (stdout)")
    );
    eprintln!(
        "  MQTT-Commands: {}",
        if mqtt_commands {
            "an (HA kann steuern)"
        } else {
            "aus"
        }
    );
    eprintln!("  Web:    {web_path}");
    eprintln!("  Login:  Initial-Passwort 'telenot-setup' (Dev)");
    eprintln!(
        "  Disarm: {}",
        if disarm_enabled {
            "aktiv"
        } else {
            "fail-closed"
        }
    );
    eprintln!("  URL:    http://{bind}  (Loopback-Dev, kein TLS)");
    eprintln!("  ⚠  Host-Modus ist NICHT die Sicherheitsgrenze (siehe THREAT-MODEL.md).");

    let server = tiny_http::Server::http(&bind)
        .map_err(|e| std::io::Error::other(format!("HTTP-Bind {bind}: {e}")))?;
    for request in server.incoming_requests() {
        handle_http(request, &app, &html, &sim_alarm, &make_app);
    }
    Ok(())
}

fn build_ema(ema: &str, alarm: &Arc<AtomicBool>) -> std::io::Result<Box<dyn Transport + Send>> {
    let (kind, rest) = ema.split_once(':').unwrap_or((ema, ""));
    match kind {
        "mock" => Ok(Box::new(MockTransport::new(
            default_seed(),
            Arc::clone(alarm),
        ))),
        "replay" => {
            let data = std::fs::read(rest)?;
            Ok(Box::new(LoopReplay { data, pos: 0 }))
        }
        "tcp" => {
            let stream = TcpStream::connect(rest)?;
            stream.set_read_timeout(Some(Duration::from_millis(500)))?;
            Ok(Box::new(crate::TcpTransport { stream }))
        }
        _ => Err(std::io::Error::other(format!("unbekannte --ema {ema}"))),
    }
}

#[allow(clippy::too_many_arguments)]
fn ema_loop(
    mut transport: Box<dyn Transport + Send>,
    config: Config,
    pin: Option<String>,
    enable_disarm: bool,
    config_path: Option<String>,
    mqtt_broker: Option<String>,
    mqtt_user: Option<String>,
    mqtt_pass: Option<String>,
    mqtt_tls: bool,
    ha_discovery: bool,
    mqtt_commands: bool,
    app: Arc<Mutex<App>>,
) {
    let cfg_for_disc = config.clone();
    let disarm_enabled = enable_disarm || pin.is_some();
    let mut runtime = Runtime::new(Arc::new(config), disarm_enabled);
    let root = app.lock().unwrap().setup.mqtt.topic_root.clone();

    // Real publish client (plain-text) only with --mqtt; otherwise stdout only.
    let client = mqtt_broker.as_ref().map(|hp| {
        let (h, p) = hp.split_once(':').unwrap_or((hp.as_str(), "1883"));
        let port: u16 = p.parse().unwrap_or(1883);
        if mqtt_tls {
            eprintln!("  ⚠ --mqtt-tls auf dem Host nicht verdrahtet → Klartext {h}:{port}");
        }
        let mut opts = mqtt_options(
            "telenot-bridge",
            h,
            port,
            mqtt_user.as_deref(),
            mqtt_pass.as_deref(),
        );
        // LWT: if the bridge disconnects, the broker sets availability=offline (retained).
        opts.set_last_will(rumqttc::LastWill::new(
            format!("{root}/availability"),
            "offline",
            QoS::AtLeastOnce,
            true,
        ));
        let (cl, mut conn) = Client::new(opts, 64);
        // Opt-in: generischen v1-`command`-Topic abonnieren (HA & jeder andere MQTT-Sender).
        let cmd_topic = if mqtt_commands {
            let t = format!("{root}/command");
            let _ = cl.subscribe(&t, QoS::AtLeastOnce);
            eprintln!("  MQTT-Subscribe: {t}");
            Some(t)
        } else {
            None
        };
        let app_ev = Arc::clone(&app);
        std::thread::spawn(move || {
            for ev in conn.iter() {
                match ev {
                    Ok(Event::Incoming(Incoming::Publish(p))) => {
                        if cmd_topic.as_deref() == Some(p.topic.as_str()) {
                            // Eventloop-Thread reicht nur einen Intent ein — NIE direkt in den Core.
                            match telenot_app::parse_command_message(p.payload.as_ref(), p.retain) {
                                Ok((cmd, pin)) => {
                                    app_ev
                                        .lock()
                                        .unwrap()
                                        .intents
                                        .push(Intent::Command { cmd, pin });
                                }
                                Err(reason) => eprintln!("  MQTT-Command verworfen: {reason}"),
                            }
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_secs(2)), // Reconnect drosseln
                    _ => {}
                }
            }
        });
        eprintln!("  MQTT-Publish → {h}:{port}");
        cl
    });
    let mut sink = HostSink { root, client };
    // Retained-chunk bookkeeping for the inventory (see publish_inventory).
    let mut last_inventory_chunks: usize = 0;
    if sink.client.is_some() {
        if ha_discovery {
            publish_discovery(&mut sink, &cfg_for_disc, mqtt_commands);
        }
        publish_inventory(&mut sink, &cfg_for_disc, &mut last_inventory_chunks);
    }
    let start = Instant::now();
    // availability=online is only published after confirmed serial liveness (no blind
    // retained republish, cf. MQTT contract); diagnostics published periodically.
    let mut online_announced = false;
    let mut last_diag_ms: u64 = 0;

    loop {
        let now = start.elapsed().as_millis() as u64;

        // 1. Intents drainen + routen (Lock nur kurz).
        let intents: Vec<Intent> = {
            let mut a = app.lock().unwrap();
            std::mem::take(&mut a.intents)
        };
        for intent in intents {
            match intent {
                Intent::TestMqtt(t) => {
                    // Probe in worker thread — must NEVER block the serial owner.
                    let app2 = Arc::clone(&app);
                    std::thread::spawn(move || {
                        let res = probe_mqtt(&t);
                        let mut a = app2.lock().unwrap();
                        a.mqtt_test = MqttTestView {
                            state: TestState::Done,
                            result: Some(res),
                            cert: None, // Host: kein TOFU-Cert-Fetch (TLS nicht verdrahtet)
                        };
                        if res.ok() {
                            a.setup.mqtt_tested = true;
                        }
                    });
                }
                Intent::ReloadConfig(cfg) => {
                    if let Some(p) = &config_path {
                        let _ = std::fs::write(p, cfg.to_json().unwrap_or_default());
                    }
                    // After commit the device is considered configured → a reload lands on the
                    // dashboard (mirrors real firmware, which goes live immediately after reboot).
                    {
                        let mut a = app.lock().unwrap();
                        a.device.configured = true;
                        // App and core share the Arc (one sensor table in steady state).
                        a.persisted = Arc::clone(&cfg);
                    }
                    push_log(&app, now, "info", "Konfiguration gespeichert & neu geladen");
                    // Symmetric to connect: always refresh inventory (path-B consumers,
                    // even without HA discovery); discovery only for path A.
                    if sink.client.is_some() {
                        if ha_discovery {
                            publish_discovery(&mut sink, &cfg, mqtt_commands);
                        }
                        publish_inventory(&mut sink, &cfg, &mut last_inventory_chunks);
                    }
                    // Align the core master switch to the remote-disarm setting (may have been
                    // changed in the wizard) so a subsequently enabled disarm takes effect.
                    let remote = app.lock().unwrap().setup.remote_disarm;
                    runtime.reload(cfg, remote);
                }
                Intent::CheckConnection { ip, port } => {
                    // Real TCP probe in a worker (never blocks the serial owner): connect +
                    // listen window to confirm the GMS actually sends bytes — an open port
                    // alone is not enough (USR-TCP232 accepts connections even without a panel).
                    // Result semantics shared with firmware: ConnCheckView::from_probe.
                    let app2 = Arc::clone(&app);
                    std::thread::spawn(move || {
                        let mut data_seen = false;
                        let connected = format!("{ip}:{port}")
                            .to_socket_addrs()
                            .ok()
                            .and_then(|mut it| it.next())
                            .and_then(|sa| {
                                TcpStream::connect_timeout(&sa, Duration::from_secs(2)).ok()
                            })
                            .map(|mut s| {
                                s.set_read_timeout(Some(Duration::from_millis(500))).ok();
                                let deadline = Instant::now() + Duration::from_secs(4);
                                let mut buf = [0u8; 64];
                                while Instant::now() < deadline {
                                    match s.read(&mut buf) {
                                        Ok(n) if n > 0 => {
                                            data_seen = true;
                                            break;
                                        }
                                        Ok(_) => break, // EOF
                                        Err(e)
                                            if matches!(
                                                e.kind(),
                                                std::io::ErrorKind::WouldBlock
                                                    | std::io::ErrorKind::TimedOut
                                            ) => {}
                                        Err(_) => break,
                                    }
                                }
                                true
                            })
                            .unwrap_or(false);
                        app2.lock().unwrap().conn_check =
                            ConnCheckView::from_probe(connected, data_seen);
                    });
                }
                Intent::CaptureStart { mode } => {
                    app.lock().unwrap().capture.start(mode, now);
                    push_log(
                        &app,
                        now,
                        "info",
                        &format!("Debug-Capture: {}", mode.sends_desc()),
                    );
                    // Discover also runs the occupancy/text scan (read-only queries).
                    if mode == telenot_app::app::CaptureMode::Discover {
                        runtime.apply_intent(now, Intent::StartScan);
                    }
                }
                Intent::CaptureStop => {
                    let was_discover = {
                        let mut a = app.lock().unwrap();
                        let d = a.capture.mode == telenot_app::app::CaptureMode::Discover;
                        a.capture.stop(now);
                        d
                    };
                    if was_discover {
                        runtime.apply_intent(
                            now,
                            Intent::CancelScan {
                                keep_partial: false,
                            },
                        );
                    }
                    push_log(&app, now, "info", "Debug-Capture gestoppt");
                }
                Intent::Command { cmd, pin: given } => {
                    // SECURITY: During a debug capture ALL commands are hard-blocked
                    // (belt-and-suspenders — the foreign panel must never be switched).
                    if app.lock().unwrap().capture.active {
                        push_log(&app, now, "warn", "Befehl während Debug-Capture abgelehnt");
                        continue;
                    }
                    // Authorization in ONE tested function (telenot-app::authorize_command):
                    // disarm fail-closed (remote-disarm + constant-time/lockout PIN check in
                    // services), arm/reset allowed (core is pre-arm-gated). Hold lock briefly.
                    let decision = {
                        let mut a = app.lock().unwrap();
                        let remote = a.setup.remote_disarm;
                        authorize_command(cmd, remote, given.as_deref(), a.services.as_mut())
                    };
                    match decision {
                        CommandAuth::Allow => {
                            let acts = runtime.on_command(now, cmd);
                            apply_actions(acts, transport.as_mut(), &mut sink, &app, now);
                        }
                        CommandAuth::Deny(reason) => {
                            push_log(&app, now, "warn", &format!("Disarm abgelehnt: {reason}"));
                        }
                    }
                }
                other => runtime.apply_intent(now, other),
            }
        }

        // 2. Read EMA → feed runtime → execute actions.
        match transport.read_chunk() {
            Ok(Some(chunk)) if !chunk.is_empty() => {
                let mut actions = runtime.feed(now, &chunk);
                // Debug capture: record raw RX bytes; in "listen-only" mode suppress any
                // transmissions to the (foreign) panel.
                {
                    let mut a = app.lock().unwrap();
                    a.capture.record(&chunk);
                    if a.capture.active && a.capture.mode.suppresses_tx() {
                        actions.retain(|act| !matches!(act, Action::SendFrame(_)));
                    }
                }
                apply_actions(actions, transport.as_mut(), &mut sink, &app, now);
            }
            Ok(_) => {
                let actions = runtime.tick(now);
                apply_actions(actions, transport.as_mut(), &mut sink, &app, now);
            }
            Err(e) => {
                push_log(&app, now, "error", &format!("EMA-Lesefehler: {e}"));
                break;
            }
        }

        // 3. Live-Snapshot publizieren + fertiges Scan-Ergebnis seeden.
        let announce_online = {
            let mut a = app.lock().unwrap();
            a.live = runtime.snapshot();
            a.mqtt_connected = sink.client.is_some();
            if let Some((cfg, raw)) = runtime.take_scan_result() {
                a.setup.seed(cfg, raw);
            }
            // Host hat kein HAP: sobald HomeKit-Modus aktiv ist, ein Demo-Pairing (QR + Code)
            // einspeisen, damit der HomeKit-Screen im Video/e2e einen QR zeigt (kein echtes Pairing).
            if a.setup.mqtt.homekit_mode && a.homekit_pair.is_none() {
                a.homekit_pair = Some(telenot_app::dto::HomekitPair {
                    code: "123-45-678".into(),
                    payload: "X-HM://0024K0Q1TELENOTBRIDGE".into(),
                });
            }
            !online_announced
                && sink.client.is_some()
                && a.live.availability == "online"
                && a.live.last_frame_ms_ago.is_some()
        };
        if announce_online {
            sink.publish("availability", "online", true);
            sink.publish("diagnostics", &diagnostics_payload(&app, now), true);
            last_diag_ms = now;
            online_announced = true;
        }

        // 4. Diagnostics periodically (retained) for field diagnosis without the web UI.
        if sink.client.is_some() && now.saturating_sub(last_diag_ms) >= 30_000 {
            last_diag_ms = now;
            sink.publish("diagnostics", &diagnostics_payload(&app, now), true);
        }

        // Host sim may tick faster than firmware (80 ms) → 240-step scan stays short enough for video.
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn apply_actions(
    actions: Vec<Action>,
    transport: &mut dyn Transport,
    sink: &mut HostSink,
    app: &Arc<Mutex<App>>,
    now: u64,
) {
    let mut logs: Vec<(&'static str, String)> = Vec::new();
    for a in actions {
        match a {
            Action::SendFrame(bytes) => {
                let _ = transport.write_frame(&bytes);
            }
            Action::Publish {
                topic,
                payload,
                retain,
            } => {
                sink.publish(&topic, &payload, retain);
                logs.push(("info", format!("PUB {topic} = {payload}")));
            }
            Action::PersistNightFlag(f) => logs.push(("info", format!("night_flag = {f}"))),
            Action::Log(msg) => logs.push(("info", msg)),
        }
    }
    if !logs.is_empty() {
        let mut a = app.lock().unwrap();
        for (level, msg) in logs {
            a.ring.push(now, level, msg);
        }
    }
}

fn push_log(app: &Arc<Mutex<App>>, now: u64, level: &'static str, msg: &str) {
    app.lock().unwrap().ring.push(now, level, msg.to_string());
}

// ───────────────────────── HTTP ─────────────────────────

fn handle_http(
    mut request: tiny_http::Request,
    app: &Arc<Mutex<App>>,
    html: &[u8],
    sim_alarm: &Arc<AtomicBool>,
    make_app: &ResetFactory,
) {
    let method = match request.method() {
        tiny_http::Method::Get => Method::Get,
        tiny_http::Method::Post => Method::Post,
        tiny_http::Method::Put => Method::Put,
        tiny_http::Method::Patch => Method::Patch,
        tiny_http::Method::Delete => Method::Delete,
        _ => {
            let _ = request.respond(tiny_http::Response::empty(405));
            return;
        }
    };

    let url = request.url().to_string();
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (url.clone(), String::new()),
    };

    // Sim-only control routes (host sim only, NEVER in firmware) — demo/e2e:
    //   POST /sim/intrude → "intrusion" (when armed ⇒ alarm), POST /sim/calm → cancel.
    if let Some(rest) = path.strip_prefix("/sim/") {
        let status = if method == Method::Post && rest == "intrude" {
            sim_alarm.store(true, Ordering::Relaxed);
            204
        } else if method == Method::Post && rest == "calm" {
            sim_alarm.store(false, Ordering::Relaxed);
            204
        } else if method == Method::Post && rest == "reset" {
            // e2e isolation: reset app to a fresh initial state (password, session,
            // config, OTA phase). The EMA thread sees the fresh state on the next tick.
            *app.lock().unwrap() = make_app();
            sim_alarm.store(false, Ordering::Relaxed);
            204
        } else {
            404
        };
        let _ = request.respond(tiny_http::Response::empty(status));
        return;
    }

    // Static asset (everything outside /api).
    if !path.starts_with("/api") {
        if method == Method::Get {
            let mut resp = tiny_http::Response::from_data(html.to_vec());
            resp.add_header(header("Content-Type", "text/html; charset=utf-8"));
            let _ = request.respond(resp);
        } else {
            let _ = request.respond(tiny_http::Response::empty(405));
        }
        return;
    }

    // Read headers.
    let mut session = None;
    let mut csrf = None;
    let mut origin = None;
    let mut host = None;
    for h in request.headers() {
        let field = h.field.as_str().as_str().to_ascii_lowercase();
        let value = h.value.as_str().to_string();
        match field.as_str() {
            "cookie" => session = cookie_value(&value, "session"),
            "x-csrf-token" => csrf = Some(value),
            "origin" => origin = Some(value),
            "host" => host = Some(value),
            _ => {}
        }
    }
    let origin_ok = match (&origin, &host) {
        (None, _) => true,
        (Some(o), Some(h)) => o.contains(h.as_str()),
        (Some(_), None) => false,
    };

    // OTA upload: same auth/validation path as the firmware streaming route, but instead
    // of flashing, only state simulation — for UI testing including error paths (?fail=<code>).
    if method == Method::Post && path == "/api/v1/ota/upload" {
        handle_ota_upload_mock(request, app, session, csrf, origin_ok, &query);
        return;
    }

    let mut body = Vec::new();
    let _ = request.as_reader().read_to_end(&mut body);

    let api_req = ApiRequest {
        method,
        path,
        query,
        body,
        session_token: session,
        csrf_token: csrf,
        origin_ok,
    };

    let api_resp = {
        let mut a = app.lock().unwrap();
        dispatch(&mut a, &api_req)
    };

    let mut resp = tiny_http::Response::from_data(api_resp.body).with_status_code(api_resp.status);
    resp.add_header(header("Content-Type", api_resp.content_type));
    if let Some(c) = api_resp.set_cookie {
        resp.add_header(header("Set-Cookie", &c));
    }
    let _ = request.respond(resp);
}

fn cookie_value(cookie_header: &str, key: &str) -> Option<String> {
    cookie_header.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

fn header(key: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(key.as_bytes(), value.as_bytes()).expect("valid header")
}

fn fallback_html(path: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>bridge setup</title>\
         <body style=\"font-family:system-ui;max-width:40rem;margin:4rem auto;padding:1rem\">\
         <h1>Setup-Web nicht gefunden</h1>\
         <p>Asset <code>{path}</code> fehlt. Erst bauen:</p>\
         <pre>npm --prefix web run build</pre>\
         <p>Die REST-API unter <code>/api/v1</code> läuft trotzdem.</p>"
    )
}

/// Sim mock of the OTA upload: auth + length/SHA/magic checks identical to firmware
/// (telenot-esp-bridge/src/httpd.rs::serve_ota_upload), but without flashing — end state is
/// `ready_to_reboot`. Error injection for UI tests: `?fail=<code>` forces that code.
fn handle_ota_upload_mock(
    mut request: tiny_http::Request,
    app: &Arc<Mutex<App>>,
    session: Option<String>,
    csrf: Option<String>,
    origin_ok: bool,
    query: &str,
) {
    use telenot_app::ota::{
        app_desc_version, len_ok, sha256_matches, Digest, OtaState, Sha256, ESP_IMAGE_MAGIC,
    };

    let respond_err = |request: tiny_http::Request, status: u16, code: &str| {
        let body = format!("{{\"error\":{{\"code\":\"{code}\",\"message\":\"OTA\"}}}}");
        let mut resp = tiny_http::Response::from_string(body).with_status_code(status);
        resp.add_header(header("Content-Type", "application/json; charset=utf-8"));
        let _ = request.respond(resp);
    };

    let total = request.body_length();
    let expected_sha = request
        .headers()
        .iter()
        .find(|h| {
            h.field
                .as_str()
                .as_str()
                .eq_ignore_ascii_case("x-expected-sha256")
        })
        .map(|h| h.value.as_str().trim().to_string())
        .filter(|s| !s.is_empty());
    let forced_fail = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("fail="))
        .map(str::to_string);

    // Auth + Zustands-Gate VOR dem Body (wie die Firmware).
    {
        let mut a = app.lock().unwrap();
        if let Err(resp) =
            telenot_app::api::check_auth(&a, session.as_deref(), csrf.as_deref(), origin_ok, true)
        {
            let mut r = tiny_http::Response::from_data(resp.body).with_status_code(resp.status);
            r.add_header(header("Content-Type", resp.content_type));
            let _ = request.respond(r);
            return;
        }
        if a.ota.phase == telenot_app::OtaPhase::Receiving {
            return respond_err(request, 409, "busy");
        }
        let Some(total) = total else {
            return respond_err(request, 411, "length_required");
        };
        if expected_sha.is_none() {
            return respond_err(request, 400, "sha_required");
        }
        if let Err(code) = len_ok(total) {
            a.ota = OtaState::failed(code);
            return respond_err(request, 400, code);
        }
        a.ota = OtaState::receiving(total);
    }
    let total = total.unwrap_or(0);
    let expected_sha = expected_sha.unwrap_or_default();

    let mut body = Vec::new();
    let _ = request.as_reader().read_to_end(&mut body);
    {
        let mut a = app.lock().unwrap();
        a.ota.received = body.len();
    }

    let fail = forced_fail.map(|s| match s.as_str() {
        "sha_mismatch" => "sha_mismatch",
        "bad_image" => "bad_image",
        "write_failed" => "write_failed",
        _ => "write_failed",
    });
    let result: Result<String, &'static str> = if let Some(code) = fail {
        Err(code)
    } else if body.len() != total {
        Err("too_small")
    } else if body.first() != Some(&ESP_IMAGE_MAGIC) {
        Err("bad_image")
    } else {
        let mut h = Sha256::new();
        h.update(&body);
        if sha256_matches(&expected_sha, &h.finalize()) {
            Ok(app_desc_version(&body).unwrap_or_else(|| "sim".into()))
        } else {
            Err("sha_mismatch")
        }
    };

    match result {
        Ok(version) => {
            let mut a = app.lock().unwrap();
            a.ota.phase = telenot_app::OtaPhase::ReadyToReboot;
            a.ota.new_version = Some(version.clone());
            drop(a);
            let mut resp = tiny_http::Response::from_string(format!(
                "{{\"state\":\"ready_to_reboot\",\"version\":\"{version}\"}}"
            ));
            resp.add_header(header("Content-Type", "application/json; charset=utf-8"));
            let _ = request.respond(resp);
        }
        Err(code) => {
            app.lock().unwrap().ota = OtaState::failed(code);
            let status = match code {
                "write_failed" => 500,
                _ => 400,
            };
            respond_err(request, status, code);
        }
    }
}

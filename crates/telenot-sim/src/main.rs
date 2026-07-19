//! Host runner for the Telenot bridge — runs the REAL `telenot-core` on the desktop,
//! without an ESP32. The transport is interchangeable (same core behind each):
//!
//! ```text
//! telenot-sim replay <incoming.bin> [--config c.json]
//! telenot-sim tcp <host:port> [--config c.json] [--enable-disarm]
//! ```
//!
//! - **replay**: plays back a raw capture (read-only; TX is only logged). Test tool.
//! - **tcp**: connects live to the USR-TCP232 module of the panel and writes our
//!   CONFIRM_ACKs/commands BACK onto the channel (bidirectional). Commands interactively via
//!   stdin: `arm_away` · `arm_home` · `arm_night` · `disarm` · `reset`.

use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use telenot_config::{Config, Polarity, Sensor, SensorKind, CURRENT_SCHEMA_VERSION};
use telenot_core::{
    decode_panel_text, Action, ArmCommand, Availability, Core, CoreOptions, Discovery, Tick,
};
use telenot_protocol::{encode_belegt_query, encode_text_query, Frame, FrameDecoder};

mod serve;

/// Byte source/sink. `read_chunk` returns `None` on EOF/closed connection.
trait Transport {
    fn read_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>>;
    fn write_frame(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    /// Does this transport run indefinitely (TCP/serial) or terminate (replay)?
    fn is_live(&self) -> bool;
}

/// Replay from a raw-byte file, in small chunks — exercises frame reassembly.
struct FileReplay {
    data: Vec<u8>,
    pos: usize,
}
impl Transport for FileReplay {
    fn read_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        let end = (self.pos + 8).min(self.data.len());
        let out = self.data[self.pos..end].to_vec();
        self.pos = end;
        Ok(Some(out))
    }
    fn write_frame(&mut self, _bytes: &[u8]) -> std::io::Result<()> {
        Ok(()) // read-only replay: TX is only logged by the runner
    }
    fn is_live(&self) -> bool {
        false
    }
}

/// Live connection to the USR-TCP232 module (RS232-over-TCP).
struct TcpTransport {
    stream: TcpStream,
}
impl Transport for TcpTransport {
    fn read_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let mut buf = [0u8; 256];
        match self.stream.read(&mut buf) {
            Ok(0) => Ok(None), // remote side closed the connection
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(Some(Vec::new())),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(Some(Vec::new())),
            Err(e) => Err(e),
        }
    }
    fn write_frame(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes)
    }
    fn is_live(&self) -> bool {
        true
    }
}

fn hexlower(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

#[derive(Default)]
struct Counts {
    frames: usize,
    tx: usize,
    pub_: usize,
}

/// Executes an action: write TX bytes to the transport, display publishes/logs.
fn execute(
    action: &Action,
    transport: &mut dyn Transport,
    counts: &mut Counts,
) -> std::io::Result<()> {
    match action {
        Action::SendFrame(bytes) => {
            counts.tx += 1;
            transport.write_frame(bytes)?;
            if bytes.len() != 8 {
                // ACK (8 B) is routine; only highlight command frames.
                println!("  TX[CMD] {}", hexlower(bytes));
            }
        }
        Action::Publish {
            topic,
            payload,
            retain,
        } => {
            counts.pub_ += 1;
            let r = if *retain { " (retain)" } else { "" };
            println!("  PUB telenot/v1/<dev>/{topic} = {payload}{r}");
        }
        Action::PersistNightFlag(f) => println!("  NVS night_flag = {f}"),
        Action::Log(msg) => println!("  LOG {msg}"),
    }
    Ok(())
}

fn parse_cmd(line: &str) -> Option<ArmCommand> {
    match line.trim().to_lowercase().as_str() {
        "arm_away" | "away" => Some(ArmCommand::ArmAway),
        "arm_home" | "home" => Some(ArmCommand::ArmHome),
        "arm_night" | "night" => Some(ArmCommand::ArmNight),
        "disarm" => Some(ArmCommand::Disarm),
        "reset" => Some(ArmCommand::Reset),
        _ => None,
    }
}

/// stdin reader thread → feeds commands into the main loop (live mode only). Mirrors the
/// firmware-side command gate (B4): disarm requires `--enable-disarm` or, preferably,
/// `--pin` with input `disarm <code>` (otherwise rejected).
fn spawn_stdin_reader(pin: Option<String>, enable_disarm: bool) -> Receiver<ArmCommand> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines().map_while(Result::ok) {
            let line = line.trim();
            // Ignore control characters (e.g. arrow keys/ANSI escapes) instead of complaining.
            if line.is_empty() || line.bytes().any(|b| b.is_ascii_control()) {
                continue;
            }
            let mut tok = line.split_whitespace();
            let head = tok.next().unwrap_or("").to_lowercase();

            if head == "disarm" {
                let given = tok.next();
                let ok = match (&pin, given) {
                    (Some(expected), Some(code)) => code == expected,
                    (Some(_), None) => {
                        eprintln!("?? Disarm braucht PIN: 'disarm <code>'");
                        false
                    }
                    (None, _) => {
                        if enable_disarm {
                            true
                        } else {
                            eprintln!("?? Disarm ist fail-closed — starte mit --pin <code> (empfohlen) oder --enable-disarm");
                            false
                        }
                    }
                };
                if ok {
                    let _ = tx.send(ArmCommand::Disarm);
                } else if pin.is_some() && given.is_some() {
                    eprintln!("?? PIN falsch");
                }
                continue;
            }

            match parse_cmd(&head) {
                Some(cmd) => {
                    let _ = tx.send(cmd);
                }
                None => eprintln!(
                    "?? unbekannt: '{line}' (arm_away|arm_home|arm_night|disarm [code]|reset)"
                ),
            }
        }
    });
    rx
}

fn default_config() -> Config {
    let s = |address, topic: &str, kind| Sensor {
        address,
        name: topic.into(),
        name_ha: topic.into(),
        kind,
        topic: topic.into(),
        location: "x".into(),
        polarity: Polarity::ActiveLow,
        confirmed: true,
        switchable: false,
        show_in_homekit: false,
    };
    Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        // Fixed four-sensor default — cannot exceed the table limits.
        sensors: telenot_config::SensorTable::from_sensors(&[
            s(0x0075, "eg/essen/bewegung", SensorKind::Bewegungsmelder),
            s(0x0530, "system/unscharf", SensorKind::Systemstatus),
            s(0x0531, "system/intern_scharf", SensorKind::Systemstatus),
            s(0x0532, "system/extern_scharf", SensorKind::Systemstatus),
        ])
        .expect("static default config fits"),
        panel: Default::default(),
    }
}

fn usage() -> ! {
    eprintln!("Aufruf:");
    eprintln!("  telenot-sim replay <incoming.bin> [--config c.json]");
    eprintln!("  telenot-sim tcp <host:port> [--config c.json] [--enable-disarm] [--pin code]");
    eprintln!(
        "  telenot-sim discover <host:port>   (sendet Belegt-/Text-Abfragen, dumpt Antworten)"
    );
    eprintln!("  telenot-sim serve --ema mock|replay:<bin>|tcp:<host:port> \\");
    eprintln!("                    [--web <index.html>] [--bind 127.0.0.1:8443] \\");
    eprintln!("                    [--config c.json] [--pin <code>|--enable-disarm] \\");
    eprintln!("                    [--mqtt <host:port>] [--mqtt-user U] [--mqtt-pass P] [--mqtt-commands]");
    eprintln!("                    → Setup-Web + REST gegen den echten Core, ohne Hardware;");
    eprintln!("                      --mqtt publiziert live an einen Klartext-Broker (→ HA);");
    eprintln!(
        "                      --mqtt-commands abonniert den command-Topic (Arm/Disarm aus HA)"
    );
    std::process::exit(2);
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        usage();
    }
    let mode = args[0].as_str();
    if mode == "serve" {
        return serve::run(&args);
    }
    let target = args[1].clone();
    let config_path = flag_value(&args, "--config");
    let enable_disarm = args.iter().any(|a| a == "--enable-disarm");
    let pin = flag_value(&args, "--pin");
    let disarm_enabled = enable_disarm || pin.is_some();

    let config = match config_path {
        Some(p) => Config::from_json(&std::fs::read(&p)?).expect("config valid"),
        None => default_config(),
    };
    println!(
        "Config: {} Sensoren (schema v{}) | Disarm: {}",
        config.sensors.len(),
        config.schema_version,
        if pin.is_some() {
            "PIN-gesichert"
        } else if enable_disarm {
            "AKTIV (ohne PIN)"
        } else {
            "fail-closed"
        }
    );

    let mut transport: Box<dyn Transport> = match mode {
        "replay" => {
            let mut data = Vec::new();
            std::fs::File::open(&target)?.read_to_end(&mut data)?;
            Box::new(FileReplay { data, pos: 0 })
        }
        "tcp" | "discover" => {
            println!("Verbinde TCP {target} …");
            let stream = TcpStream::connect(&target)?;
            stream.set_read_timeout(Some(Duration::from_millis(500)))?;
            println!("verbunden. Strg-C zum Beenden.");
            Box::new(TcpTransport { stream })
        }
        _ => usage(),
    };

    let is_discover = mode == "discover";
    let out_path = flag_value(&args, "--out").unwrap_or_else(|| "discovered-config.json".into());
    let mut discovery = Discovery::new();
    let mut text_queued = false;
    let mut queries: VecDeque<(&str, Vec<u8>)> = VecDeque::new();
    let belegt_bytes: Vec<u8> = if is_discover {
        let mut b = [0u8; 32];
        let n = encode_belegt_query(&mut b).unwrap();
        println!("Discovery (read-only): Belegt-Scan → Namen je belegter Adresse → {out_path}");
        b[..n].to_vec()
    } else {
        Vec::new()
    };

    let live = transport.is_live();
    let cmd_rx = if live {
        Some(spawn_stdin_reader(pin.clone(), enable_disarm))
    } else {
        None
    };

    let mut core = Core::new(std::sync::Arc::new(config), CoreOptions { disarm_enabled });
    let mut decoder = FrameDecoder::new();
    let mut counts = Counts::default();
    let start = Instant::now();
    let mut synth: Tick = 0;

    println!("--- {} ---", if live { "Live" } else { "Replay" });
    let mut awaiting_since: Option<Instant> = None;
    loop {
        // Befehle von stdin einspeisen (Live).
        if let Some(rx) = &cmd_rx {
            while let Ok(cmd) = rx.try_recv() {
                let now = start.elapsed().as_millis() as Tick;
                for a in core.on_command(now, cmd) {
                    execute(&a, transport.as_mut(), &mut counts)?;
                }
            }
        }

        match transport.read_chunk()? {
            None => break, // EOF / connection closed
            Some(chunk) => {
                if chunk.is_empty() {
                    // Live idle: check liveness deadline.
                    let now = start.elapsed().as_millis() as Tick;
                    for a in core.on_tick(now) {
                        execute(&a, transport.as_mut(), &mut counts)?;
                    }
                    continue;
                }
                decoder.feed(&chunk);
                let mut saw_output = false;
                while let Some(ev) = decoder.next_frame() {
                    match ev {
                        Ok(frame) => {
                            counts.frames += 1;
                            if is_discover {
                                if handle_discover(
                                    &frame,
                                    &mut discovery,
                                    &mut queries,
                                    &mut text_queued,
                                    &out_path,
                                ) {
                                    awaiting_since = None; // response received → next query free
                                }
                                if is_output_status(&frame) {
                                    saw_output = true;
                                }
                            }
                            let now = if live {
                                start.elapsed().as_millis() as Tick
                            } else {
                                synth += 50;
                                synth
                            };
                            for a in core.on_frame(now, &frame) {
                                execute(&a, transport.as_mut(), &mut counts)?;
                            }
                        }
                        Err(e) => println!("  FRAME-ERR {e:?}"),
                    }
                }
                // Discovery: EXACTLY one query per poll pause (after output status),
                // and only once the previous one is answered (or after timeout).
                if is_discover && saw_output {
                    let timed_out = awaiting_since
                        .map(|t| t.elapsed() > Duration::from_secs(4))
                        .unwrap_or(false);
                    if awaiting_since.is_none() || timed_out {
                        if !discovery.belegt_complete() {
                            // Occupancy scan: keep sending until BOTH telegrams (0x71+0x72) arrive
                            // — a direct query does not always land cleanly in the poll window.
                            if timed_out {
                                println!("  (Belegt-Antwort ausgeblieben — erneut)");
                            }
                            transport.write_frame(&belegt_bytes)?;
                            awaiting_since = Some(Instant::now());
                        } else if let Some((_label, q)) = queries.pop_front() {
                            transport.write_frame(&q)?;
                            awaiting_since = Some(Instant::now());
                            if queries.len().is_multiple_of(10) {
                                println!("  … noch {} Adressen", queries.len());
                            }
                        } else if text_queued {
                            // All occupied addresses queried → write config & exit.
                            let cfg = discovery.into_config();
                            std::fs::write(&out_path, cfg.to_json().expect("config json"))?;
                            println!(
                                "=== Discovery fertig: {} benannte Sensoren → {out_path} ===",
                                cfg.sensors.len()
                            );
                            break;
                        }
                    }
                }
            }
        }
    }

    println!("--- Zusammenfassung ---");
    println!(
        "Frames: {} | TX: {} | Publishes: {}",
        counts.frames, counts.tx, counts.pub_
    );
    println!("Arm-Zustand: {}", core.arm_state().as_str());
    println!(
        "Verfügbarkeit: {}",
        match core.availability() {
            Availability::Online => "online",
            Availability::Unavailable => "unavailable",
        }
    );
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

/// True if the frame is an output status (0x24, ext 0x02) — marks the end of a poll cycle,
/// after which comes the pause in which we may send.
fn is_output_status(frame: &Frame) -> bool {
    frame
        .records()
        .filter_map(|r| r.ok())
        .filter_map(|r| r.as_block_status())
        .any(|bs| bs.adresserweiterung == 0x02)
}

/// Processes a Discovery response: ingests occupancy status (and queues text queries for all
/// occupied addresses once complete) or ingests a text response (address+name) and writes
/// the config incrementally. Returns true if a response was recognised.
fn handle_discover(
    frame: &Frame,
    discovery: &mut Discovery,
    queries: &mut VecDeque<(&'static str, Vec<u8>)>,
    text_queued: &mut bool,
    out_path: &str,
) -> bool {
    let mut resp = false;
    let mut got_belegt = false;

    for rec in frame.records().filter_map(|r| r.ok()) {
        if let Some(bs) = rec.as_block_status() {
            if matches!(bs.adresserweiterung, 0x71 | 0x72) {
                got_belegt = true;
                discovery.ingest_belegt(&bs);
                println!(
                    "  BELEGT erw=0x{:02x} → {} belegte Adressen gesamt",
                    bs.adresserweiterung,
                    discovery.occupied().len()
                );
            }
        }
    }

    // Text response: 0x0C carries the address, 0x54 the plain-text name (in the same telegram).
    let addr = frame
        .records()
        .filter_map(|r| r.ok())
        .find_map(|r| r.as_bereich_meldebereich().map(|bm| bm.address()));
    let name = frame
        .records()
        .filter_map(|r| r.ok())
        .find_map(|r| r.as_ascii().map(decode_panel_text));
    if let (Some(a), Some(n)) = (addr, name) {
        resp = true;
        discovery.ingest_text(a, &n);
        println!(
            "  [{}] 0x{:04X} = '{}'",
            discovery.named_count(),
            a,
            n.trim()
        );
        // incremental save → even an aborted run leaves a usable config
        if let Ok(json) = discovery.into_config().to_json() {
            let _ = std::fs::write(out_path, json);
        }
    }

    // Once both occupancy telegrams are in: queue text queries for all occupied addresses.
    if discovery.belegt_complete() && !*text_queued {
        *text_queued = true;
        for a in discovery.occupied() {
            let mut t = [0u8; 32];
            if let Ok(k) = encode_text_query(a, &mut t) {
                queries.push_back(("text", t[..k].to_vec()));
            }
        }
        println!(
            "  → {} belegte Adressen werden benannt (~3s/Adresse)…",
            queries.len()
        );
    }

    // Only release awaiting once the occupancy scan is COMPLETE (both telegrams) —
    // otherwise the occupancy query would fire again between 0x71 and 0x72.
    if got_belegt && discovery.belegt_complete() {
        resp = true;
    }

    resp
}

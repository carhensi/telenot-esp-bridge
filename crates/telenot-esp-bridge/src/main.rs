//! Telenot-Bridge Firmware (Phase B) — esp-idf I/O shell.
//!
//! Slice 4: the real `telenot-app::Runtime` runs in the EMA poll loop on the board.
//! Bench mode: Panel transport is a TCP client to the host mock (instead of RS232). HTTP
//! (slice 5) and MQTT (slice 6) attach to the same `Arc<Mutex<App>>` later.

mod app_loop;
mod homekit;
mod httpd;
mod mqtt;
mod net;
mod ota;
mod storage;
mod transport;

use std::sync::{Arc, Mutex};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::{EspCustomNvsPartition, EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::sys::EspError;
use telenot_app::dto::ConnKind;
use telenot_app::security::disarm_enabled;
use telenot_app::{App, ConnSettings, DeviceInfo, Runtime};
use telenot_config::CURRENT_SCHEMA_VERSION;
use transport::{PanelTarget, PanelTransport};

/// Device initial password (dev/sticker). HARDENING LATER: derive from sticker/provisioning value.
const INITIAL_PW: &str = "telenot-init";

/// BENCH: address of the host mock panel (TCP-EMA). Emergency fallback only if UART
/// initialisation fails — bench mode sets the mock target via setup (TCP) which persists.
/// Adjust host IP locally as needed.
const PANEL_BENCH_TCP: &str = "192.0.2.212:9000"; // RFC-5737 documentation range; set real bench IP locally

fn main() -> Result<(), EspError> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::info!("telenot-esp-bridge: boot ok");

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;

    // Status LED (GPIO32) + setup button BUT1 (GPIO34, input-only) — control the setup window
    // in the loop (HTTP setup is only active for a limited time → hardening).
    let led = PinDriver::output(peripherals.pins.gpio32)?;
    // GPIO34 is input-only with no internal pull → Floating (relies on the board pull-up for
    // BUT1; add an external pull-up if the pin floats).
    let button = PinDriver::input(peripherals.pins.gpio34, Pull::Floating)?;

    // Bring up Ethernet; keep the handle alive.
    let eth = net::bring_up(
        net::EthPeripherals {
            mac: peripherals.mac,
            rmii_rxd0: peripherals.pins.gpio25,
            rmii_rxd1: peripherals.pins.gpio26,
            rmii_crs_dv: peripherals.pins.gpio27,
            rmii_txd1: peripherals.pins.gpio22,
            rmii_tx_en: peripherals.pins.gpio21,
            rmii_txd0: peripherals.pins.gpio19,
            mdc: peripherals.pins.gpio23,
            mdio: peripherals.pins.gpio18,
            ref_clk: peripherals.pins.gpio17,
            phy_power: peripherals.pins.gpio12,
        },
        sysloop,
    )?;
    let ip = eth.eth().netif().get_ip_info()?.ip;

    // Persistence: services (secrets/lockout) + config + night flag, each with its own NVS handle.
    let part = EspDefaultNvsPartition::take()?;
    let services =
        storage::NvsServices::new(EspNvs::new(part.clone(), "telenot", true)?, INITIAL_PW)
            .map_err(|e| {
                log::error!("NvsServices init: {e}");
                e
            })?;
    let nvs_cfg = EspNvs::new(part.clone(), "telenot", true)?;
    // Sensor config lives in the dedicated 128 KB `cfg` NVS partition — the 24 KB default NVS
    // is too small for a full inventory (~30 KB JSON; set_blob failed there silently).
    // If the partition is missing (partition table not flashed), do NOT abort boot;
    // continue with the default NVS and warn clearly.
    let cfg_store = match EspCustomNvsPartition::take("cfg") {
        Ok(p) => storage::CfgStore::Big(EspNvs::new(p, "telenot", true)?),
        Err(e) => {
            log::error!(
                "cfg-Partition fehlt ({e}) — Fallback auf Default-NVS (Config max ~16 KB!). \
                 Beim Flashen die Partition-Table mitgeben: \
                 espflash flash --partition-table partitions.csv …"
            );
            storage::CfgStore::Fallback(EspNvs::new(part.clone(), "telenot", true)?)
        }
    };
    // Existing devices: migrate config from the old default NVS once.
    cfg_store.migrate_from_default(&nvs_cfg);
    let config = Arc::new(cfg_store.load());
    log::info!(
        "Config geladen: schema {}, {} Sensoren",
        config.schema_version,
        config.sensors.len()
    );

    // Device identity.
    let mac = device_mac();
    let device = DeviceInfo {
        model: "telenot-esp-bridge (ESP32-POE-ISO)".into(),
        fw: env!("CARGO_PKG_VERSION").into(),
        fw_build: env!("BUILD_INFO").into(),
        serial: mac.clone(),
        mac: mac.clone(),
        ip: format!("http://{ip}"),
        schema: CURRENT_SCHEMA_VERSION,
        fingerprint: "—".into(),
        configured: !config.sensors.is_empty(),
    };
    log::info!("Gerät {mac} @ {ip}");

    // Disarm stays fail-closed: only with remote disarm enabled (persisted) AND a PIN set.
    // The master switch survives reboots in NVS — otherwise it would reset to off after
    // every restart even if the user explicitly enabled it in setup.
    let remote_disarm = storage::load_remote_disarm(&nvs_cfg);
    let disarm = disarm_enabled(
        remote_disarm,
        telenot_app::security::Services::pin_set(&services),
    );
    let app = Arc::new(Mutex::new(App::new(device, Box::new(services))));

    // Restore MQTT broker settings from NVS (not part of the config → would be lost after reboot).
    // Topics are device-agnostic (single device) — no device_id derivation needed.
    {
        let mut a = app.lock().unwrap();
        if let Some(m) = storage::load_mqtt(&nvs_cfg) {
            a.setup.mqtt = m;
        }
        a.setup.remote_disarm = remote_disarm;
        a.update_check = storage::load_ota_check(&nvs_cfg);
        // Stability observability: why did we boot, and how many times?
        a.boot_reason = reset_reason_str();
        a.boot_count = storage::bump_boot_count(&nvs_cfg);
        log::info!("Boot #{} — Grund: {}", a.boot_count, a.boot_reason);
        // Share the persisted config with the app side (GET /sensors and the live
        // test board read it directly) — no seeded copy of the sensor table anymore;
        // an edit session materializes copy-on-write only when the user edits.
        a.persisted = Arc::clone(&config);
        if cfg_store.is_fallback() {
            a.ring.push(
                0,
                "warn",
                "cfg-Partition fehlt — Config-Speicher auf ~16 KB begrenzt (großes \
                 Sensor-Inventar kann NICHT gespeichert werden). Partition-Table mitflashen!"
                    .into(),
            );
        }
        // Mirror OTA slot overview. pending_verify = this firmware is booting for the first time
        // after an OTA → the loop runs the self-test and marks the slot valid only afterwards.
        if let Some((running, boot, pending, rolled_back)) = ota::slot_overview() {
            if let Some(from) = &rolled_back {
                log::warn!("Firmware-Rollback erkannt (Slot {from} ungültig)");
                a.ring.push(
                    0,
                    "warn",
                    format!(
                        "Rollback: letztes Update ({from}) hat den Self-Test nicht \
                         bestanden — vorherige Firmware läuft weiter"
                    ),
                );
            }
            if pending {
                log::info!(
                    "Neue Firmware im Self-Test (Slot {}, PENDING_VERIFY)",
                    running.label
                );
            }
            a.ota_pending_verify = pending;
            a.ota_slots = Some((running, boot));
        }
        // One "boot ok" info line so an otherwise empty ring log is readable as
        // "nothing wrong" instead of "nothing logged".
        let slot = a
            .ota_slots
            .as_ref()
            .map(|(r, _)| format!(" · Slot {} ({})", r.label, r.state))
            .unwrap_or_default();
        let boot_line = format!(
            "Boot #{} ok: v{}{slot} · Reset: {}",
            a.boot_count,
            env!("CARGO_PKG_VERSION"),
            a.boot_reason
        );
        a.ring.push(0, "info", boot_line);
        log::info!(
            "MQTT-Ziel: {}:{} ({})",
            a.setup.mqtt.host,
            a.setup.mqtt.port,
            if a.setup.mqtt.host.is_empty() {
                "unkonfiguriert"
            } else {
                "konfiguriert"
            },
        );
    }

    // Operating mode from persisted settings: direct HomeKit (MQTT off) vs. MQTT/HA.
    let homekit_mode = app.lock().unwrap().setup.mqtt.homekit_mode;

    // The setup web (port 80) is started/stopped by the loop only within the **setup window**
    // (hardening: no HTTP in normal operation → no PIN plaintext attack surface). HAP uses its
    // own port (8080, via _hap._tcp mDNS) and runs continuously, independently.

    // Panel transport: internal RS232 (MOD-RS232 on UEXT → GPIO4 TX / GPIO36 RX, see
    // docs/HARDWARE.md variant C) and TCP (USR-TCP232) in one switchable backend;
    // the persisted setup target selects the start mode. Connects lazily in the loop —
    // the loop must run even without a panel, otherwise intents (e.g. "check connection") stall.
    let initial_baud = storage::load_baud(&nvs_cfg);
    let initial_target = match storage::load_conn(&nvs_cfg) {
        Some(PanelTarget::Tcp(ip, port)) => {
            app.lock().unwrap().setup.conn = ConnSettings {
                kind: ConnKind::Tcp,
                ip: ip.clone(),
                port,
                baud: initial_baud,
            };
            PanelTarget::Tcp(ip, port)
        }
        // "internal" persisted OR freshly flashed: internal RS232 is the UI default.
        // setup.conn already defaults to Internal.
        _ => {
            app.lock().unwrap().setup.conn.baud = initial_baud;
            PanelTarget::Internal
        }
    };
    log::info!("Panel-Ziel: {initial_target:?} @ {initial_baud} Baud");
    let transport: Box<dyn PanelTransport> = match transport::UartTransport::new(
        peripherals.uart1,
        peripherals.pins.gpio4.degrade_output(),
        peripherals.pins.gpio36.degrade_input(),
        initial_baud,
    ) {
        Ok(uart) => Box::new(transport::SwitchableTransport::new(uart, initial_target)),
        Err(e) => {
            // Without UART only TCP is available: persisted target or bench mock.
            log::error!("UART-Init (GPIO4/36) fehlgeschlagen ({e}) — nur TCP verfügbar");
            let addr = match &initial_target {
                PanelTarget::Tcp(ip, port) => format!("{ip}:{port}"),
                PanelTarget::Internal => PANEL_BENCH_TCP.to_string(),
            };
            Box::new(transport::ReconnectingTcp::new(addr))
        }
    };

    // Already configured? Config with sensors = no initial setup needed (controls the setup
    // window AND HAP startup).
    let had_config = !config.sensors.is_empty();

    // Direct HomeKit: start HAP at boot only if the device is **already configured**. During
    // initial setup (scan with many detectors + large JSON responses = RAM spike) HAP stays OFF →
    // full heap for setup. HomeKit is added after the first commit/reboot or via
    // Intent::StartHomekit (pairing only makes sense after setup is complete).
    if homekit_mode && had_config {
        homekit::start(app.clone(), (*config).clone());
    }

    // The Arc is SHARED with the core (app.persisted + core config = one sensor table).
    // HA discovery/inventory borrow it in the loop via `runtime.config()`.
    let runtime = Runtime::new(config, disarm);

    // EMA poll loop runs forever (takes over this thread).
    app_loop::run(
        app,
        runtime,
        transport,
        nvs_cfg,
        cfg_store,
        homekit_mode,
        led,
        button,
        had_config,
    );
}

/// Reset reason of the current boot as a stable string (diagnostics: "panic"/"task_wdt"
/// with short uptime = silent crash; "poweron" = power cycle; "sw" = intentional reboot).
fn reset_reason_str() -> &'static str {
    use esp_idf_svc::sys::*;
    match unsafe { esp_reset_reason() } {
        x if x == esp_reset_reason_t_ESP_RST_POWERON => "poweron",
        x if x == esp_reset_reason_t_ESP_RST_EXT => "ext",
        x if x == esp_reset_reason_t_ESP_RST_SW => "sw",
        x if x == esp_reset_reason_t_ESP_RST_PANIC => "panic",
        x if x == esp_reset_reason_t_ESP_RST_INT_WDT => "int_wdt",
        x if x == esp_reset_reason_t_ESP_RST_TASK_WDT => "task_wdt",
        x if x == esp_reset_reason_t_ESP_RST_WDT => "wdt",
        x if x == esp_reset_reason_t_ESP_RST_DEEPSLEEP => "deepsleep",
        x if x == esp_reset_reason_t_ESP_RST_BROWNOUT => "brownout",
        x if x == esp_reset_reason_t_ESP_RST_SDIO => "sdio",
        _ => "unknown",
    }
}

/// Base MAC from eFuse as an `aa:bb:…` string (serial number / diagnostics).
fn device_mac() -> String {
    let mut mac = [0u8; 6];
    unsafe { esp_idf_svc::sys::esp_efuse_mac_get_default(mac.as_mut_ptr()) };
    mac.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

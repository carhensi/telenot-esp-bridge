//! Slices 4+6: the EMA poll loop — the heart of the firmware. Mirrors `telenot-sim` `ema_loop`
//! (crates/telenot-sim/src/serve.rs), but with firmware I/O:
//!
//! 1. Drain + route intents (authorise commands, reload config, test MQTT, …).
//! 2. Read panel transport → `Runtime::feed`/`tick` → execute actions (incl. MQTT publish).
//! 3. Mirror live snapshot into `App`, seed scan result, maintain MQTT lifecycle.
//!
//! The MQTT sink is lazily (re)connected in the loop whenever `setup.mqtt` changes (boot from
//! NVS OR live configuration in setup) — one code path, no reboot needed. Publishes go via
//! the outbox (`enqueue`) and never block the serial owner.

use std::sync::{Arc, Mutex};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{Input, Output, PinDriver};
use esp_idf_svc::http::server::EspHttpServer;
use esp_idf_svc::nvs::EspDefaultNvs;
use telenot_app::app::{ConnCheckView, MqttTestView, TestState};
use telenot_app::dto::ConnKind;
use telenot_app::mqtt::MqttSink;
use telenot_app::{
    authorize_command, homekit_authorized, App, CommandAuth, Intent, MqttSettings, Runtime,
};
use telenot_core::Action;

use crate::mqtt::EspMqttSink;
use crate::transport::{PanelTarget, PanelTransport};

/// Monotonic boot time in ms (tick source for the Runtime).
fn now_ms() -> u64 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u64
}

/// Runs the poll loop forever. Owns `runtime` + `transport` exclusively (like the serial owner
/// in the host daemon); `app` is shared with HTTP/MQTT. `config` is the currently active config
/// (basis for HA discovery/inventory; refreshed on `ReloadConfig`).
#[allow(clippy::too_many_arguments)]
pub fn run(
    app: Arc<Mutex<App>>,
    mut runtime: Runtime,
    mut transport: Box<dyn PanelTransport>,
    nvs: EspDefaultNvs,
    cfg_store: crate::storage::CfgStore,
    homekit_mode: bool,
    mut led: PinDriver<'static, Output>,
    button: PinDriver<'static, Input>,
    had_config: bool,
) -> ! {
    log::info!("EMA-Poll-Loop gestartet");
    let mut last_report_ms: u64 = 0;
    let mut applied_conn: Option<PanelTarget> = None;
    let mut applied_baud: Option<u32> = None;
    let mut applied_remote: Option<bool> = None;

    // Setup window (hardening): HTTP setup (port 80) runs ONLY within an explicit time window —
    // off in normal operation (no PIN plaintext attack surface). Initial setup (no sensors yet)
    // gets 24 h; afterwards 30 min per boot, extendable at any time via BUT1 (GPIO34). HAP
    // (HomeKit, port 8080) is unaffected and runs continuously.
    const WINDOW_MS: u64 = 30 * 60 * 1000;
    const FIRST_SETUP_MS: u64 = 24 * 60 * 60 * 1000;
    // Robust start: port 80 may be briefly busy right after stopping → 3 attempts.
    let start_http = |app: &Arc<Mutex<App>>| {
        for attempt in 0..3 {
            match crate::httpd::start(app.clone(), 80) {
                Ok(h) => {
                    if attempt > 0 {
                        log::info!("HTTP-Setup-Server: Start ok (Versuch {})", attempt + 1);
                    }
                    return Some(h);
                }
                Err(e) => {
                    log::warn!("HTTP-Setup-Server Start (Versuch {}): {e}", attempt + 1);
                    FreeRtos::delay_ms(250);
                }
            }
        }
        log::error!("HTTP-Setup-Server: Start nach 3 Versuchen fehlgeschlagen");
        None
    };
    let mut http: Option<EspHttpServer<'static>> = start_http(&app);
    let mut setup_until: u64 = now_ms()
        + if had_config {
            WINDOW_MS
        } else {
            FIRST_SETUP_MS
        };
    // Absolute cap for web-triggered extensions: never more than 24 h since boot. The physical
    // BUT1 button (physical access = trust) remains uncapped; only the web path is limited so
    // that a compromised client cannot keep the security window open indefinitely.
    let setup_cap: u64 = now_ms() + FIRST_SETUP_MS;
    let mut btn_low_since: u64 = 0;
    let mut btn_handled = false;
    let mut led_last_toggle: u64 = 0;
    let mut led_on = false;
    log::info!(
        "Setup-Fenster offen für {} min (BUT1 öffnet erneut)",
        (setup_until - now_ms()) / 60000
    );

    // OTA self-test: if this firmware boots as PENDING_VERIFY (first boot after an OTA),
    // it must run N minutes without a panic before the slot is made permanent. A panic/reboot
    // before that → the bootloader automatically rolls back to the previous firmware.
    let mut ota_self_test: Option<telenot_app::SelfTest> =
        app.lock().unwrap().ota_pending_verify.then(|| {
            telenot_app::SelfTest::new(now_ms(), telenot_app::SelfTest::DEFAULT_DURATION_S)
        });

    // Update check: persistence mirror + daily timer (worker thread, never blocks the loop).
    let mut applied_ota_check: Option<bool> = None;
    let mut last_update_check_ms: u64 = 0;
    const UPDATE_CHECK_EVERY_MS: u64 = 24 * 60 * 60 * 1000;

    // MQTT sink: starts log-only (unconfigured) and is (re)connected when settings change.
    let root0 = app.lock().unwrap().setup.mqtt.topic_root.clone();
    let mut sink = EspMqttSink::disconnected(root0);
    let mut applied_mqtt: Option<MqttSettings> = None;
    let mut next_mqtt_connect_at = 0;
    let mut online_announced = false;
    let mut last_diag_ms: u64 = 0;
    // Retained-chunk bookkeeping: how many `inventory/chunk/<i>` topics the LAST publish
    // produced, so a shrinking inventory can clear the orphaned retained topics.
    let mut last_inventory_chunks: usize = 0;

    loop {
        let now = now_ms();

        // OTA self-test complete → mark slot valid (close the rollback window).
        // Deliberately no panel criterion: an unplugged panel is not a firmware fault.
        if let Some(st) = ota_self_test {
            if st.due(now) {
                match crate::ota::mark_valid() {
                    Ok(()) => {
                        let mut a = app.lock().unwrap();
                        a.ota_pending_verify = false;
                        a.ota_self_test_s = None;
                        if let Some((running, _)) = &mut a.ota_slots {
                            running.state = "valid".into();
                        }
                        let offline = a.live.availability != "online";
                        a.ring.push(
                            now,
                            "info",
                            "Firmware-Self-Test bestanden — Update ist dauerhaft".into(),
                        );
                        if offline {
                            a.ring.push(
                                now,
                                "warn",
                                "Hinweis: Zentrale war beim Self-Test offline (kein \
                                 Rollback-Grund, aber prüfen)"
                                    .into(),
                            );
                        }
                        log::info!("OTA-Self-Test bestanden — Slot als gültig markiert");
                    }
                    Err(e) => log::error!("OTA mark_valid fehlgeschlagen: {e}"),
                }
                ota_self_test = None;
            } else if let Ok(mut a) = app.lock() {
                a.ota_self_test_s = Some(st.remaining_s(now));
            }
        }

        // 0. Setup window: BUT1 (active-low, time-debounced ~300 ms) opens/extends the window;
        //    on expiry the HTTP setup is stopped (drop the handle); LED blinks while open.
        if button.is_low() {
            if btn_low_since == 0 {
                btn_low_since = now;
            } else if !btn_handled && now.wrapping_sub(btn_low_since) >= 150 {
                btn_handled = true;
                setup_until = (now + WINDOW_MS).max(setup_until); // never shorten an already-longer window
                if http.is_none() {
                    http = start_http(&app);
                    log::info!("Setup-Fenster per Taster GEÖFFNET (30 min)");
                } else {
                    log::info!("Setup-Fenster per Taster aufgefrischt");
                }
            }
        } else {
            btn_low_since = 0;
            btn_handled = false;
        }
        if http.is_some() && now >= setup_until {
            http = None; // drop → httpd_stop
            let _ = led.set_low();
            led_on = false;
            log::info!("Setup-Fenster abgelaufen → HTTP-Setup aus (BUT1 zum Öffnen)");
        }
        if http.is_some() && now.wrapping_sub(led_last_toggle) >= 700 {
            led_last_toggle = now;
            led_on = !led_on;
            let _ = if led_on {
                led.set_high()
            } else {
                led.set_low()
            };
        }

        // (Re)subscribe the MQTT command topics once the client is actually connected (the event
        // pump only signals; subscribing from the pump thread deadlocks the esp-idf-svc client).
        let mqtt_reconnected = sink.poll_resubscribe();

        // 1. Drain intents + read current setup state (hold lock only briefly).
        let (intents, conn, mqtt, remote, pin_set): (
            Vec<Intent>,
            telenot_app::ConnSettings,
            MqttSettings,
            bool,
            bool,
        ) = {
            let mut a = app.lock().unwrap();
            // Mirror remaining setup window time for the UI (countdown/reminder/lock).
            a.setup_window_s_remaining = Some((setup_until.saturating_sub(now) / 1000) as u32);
            (
                std::mem::take(&mut a.intents),
                a.setup.conn.clone(),
                a.setup.mqtt.clone(),
                a.setup.remote_disarm,
                a.services.pin_set(),
            )
        };

        // Security step "remote disarm": persist the change + switch the core gate LIVE.
        // Previously the core gate stayed at the boot state (fail-closed OFF) even when the
        // user enabled the switch in setup — and every reboot forgot the decision.
        if applied_remote != Some(remote) {
            crate::storage::save_remote_disarm(&nvs, remote);
            runtime.set_disarm_enabled(telenot_app::security::disarm_enabled(remote, pin_set));
            log::info!(
                "Remote-Disarm: {} (PIN {})",
                if remote { "AKTIV" } else { "aus" },
                if pin_set { "gesetzt" } else { "FEHLT" },
            );
            applied_remote = Some(remote);
        }

        // Persist the update-check toggle + trigger the daily check if due (60 s after
        // boot, then every 24 h). Runs in a worker thread; result lands in app.ota_latest.
        {
            let check_on = {
                let a = app.lock().unwrap();
                a.update_check
            };
            if applied_ota_check != Some(check_on) {
                if applied_ota_check.is_some() {
                    log::info!("Update-Check: {}", if check_on { "AN" } else { "aus" });
                }
                crate::storage::save_ota_check(&nvs, check_on);
                applied_ota_check = Some(check_on);
            }
            let due = last_update_check_ms == 0 && now >= 60_000
                || last_update_check_ms != 0
                    && now.wrapping_sub(last_update_check_ms) >= UPDATE_CHECK_EVERY_MS;
            if check_on && due {
                last_update_check_ms = now;
                let app2 = Arc::clone(&app);
                let spawned = std::thread::Builder::new()
                    .name("ota-check".into())
                    .stack_size(16 * 1024)
                    .spawn(move || {
                        let Some(m) = crate::ota::fetch_latest_manifest() else {
                            return;
                        };
                        let current = env!("CARGO_PKG_VERSION");
                        if telenot_app::ota::version_newer(&m.version, current) {
                            log::info!("Update verfügbar: v{} (läuft: v{current})", m.version);
                            let mut a = app2.lock().unwrap();
                            let t = crate::storage::now_ms();
                            if a.ota_latest.as_deref() != Some(m.version.as_str()) {
                                a.ring.push(
                                    t,
                                    "info",
                                    format!("Firmware-Update v{} verfügbar (Diagnose → Firmware-Update)", m.version),
                                );
                            }
                            a.ota_latest = Some(m.version);
                        }
                    });
                if let Err(e) = spawned {
                    log::error!("ota-check-Thread: {e}");
                }
            }
        }

        // Setup step "connection": apply target (internal RS232 or TCP) to the transport
        // and persist it. Switching happens live, without a reboot.
        let target = match conn.kind {
            ConnKind::Internal => Some(PanelTarget::Internal),
            ConnKind::Tcp if !conn.ip.is_empty() && conn.port != 0 => {
                Some(PanelTarget::Tcp(conn.ip, conn.port))
            }
            _ => None,
        };
        if let Some(target) = target {
            if applied_conn.as_ref() != Some(&target) {
                if transport.set_target(&target) {
                    crate::storage::save_conn(&nvs, &target);
                }
                applied_conn = Some(target);
            }
        }
        // Baud change (GMS plus = 115200): applied live to the UART (No-Reboot) and
        // persisted. Value is allowlist-validated in the connection PUT handler; persisting
        // even when no UART is present (TCP fallback) keeps the choice across reboots.
        if applied_baud != Some(conn.baud) {
            transport.set_baud(conn.baud);
            crate::storage::save_baud(&nvs, conn.baud);
            applied_baud = Some(conn.baud);
        }

        // Settings change: ALWAYS persist (including HomeKit mode / empty host — otherwise the
        // HomeKit toggle doesn't survive a power cycle). Reconnect MQTT only in MQTT mode with
        // a host set.
        if applied_mqtt.as_ref() != Some(&mqtt) && now >= next_mqtt_connect_at {
            let mut applied = true;
            crate::storage::save_mqtt(&nvs, &mqtt);
            if !homekit_mode && !mqtt.host.is_empty() {
                let pw = app.lock().unwrap().services.mqtt_password();
                match EspMqttSink::connect(
                    mqtt.topic_root.clone(),
                    &mqtt.host,
                    mqtt.port,
                    mqtt.tls,
                    &mqtt.username,
                    pw.as_deref(),
                    mqtt.pinned_cert.as_deref(),
                    app.clone(),
                ) {
                    Ok(s) => {
                        sink = s;
                        sink.request_setup();
                        publish_setup(&mut sink, &mqtt, &runtime, &mut last_inventory_chunks);
                        online_announced = false;
                        let t = crate::storage::now_ms();
                        app.lock().unwrap().ring.push(
                            t,
                            "info",
                            format!("MQTT-Client gestartet: {}:{}", mqtt.host, mqtt.port),
                        );
                    }
                    Err(e) => {
                        applied = false;
                        next_mqtt_connect_at = now.saturating_add(30_000);
                        log::error!("MQTT-Connect fehlgeschlagen: {e}");
                        let t = crate::storage::now_ms();
                        app.lock().unwrap().ring.push(
                            t,
                            "warn",
                            format!("MQTT-Connect fehlgeschlagen: {}:{}", mqtt.host, mqtt.port),
                        );
                    }
                }
            }
            if applied {
                applied_mqtt = Some(mqtt.clone());
                next_mqtt_connect_at = 0;
            }
        }

        if mqtt_reconnected {
            app.lock().unwrap().ring.push(
                now,
                "info",
                "MQTT verbunden: Broker hat Verbindung bestaetigt".into(),
            );
        }
        if mqtt_reconnected || sink.retry_due(now) {
            sink.request_setup();

            online_announced = false;
        }
        if sink.setup_ready() {
            publish_setup(&mut sink, &mqtt, &runtime, &mut last_inventory_chunks);
        }
        for intent in intents {
            match intent {
                Intent::CaptureStart { mode } => {
                    let panel_kind = {
                        let mut a = app.lock().unwrap();
                        let panel_kind = a.setup.panel.kind;
                        a.capture.start(mode, now);
                        a.capture.hiplex_probe = mode == telenot_app::app::CaptureMode::Discover
                            && panel_kind == telenot_config::PanelKind::Hiplex8400;
                        panel_kind
                    };
                    log::info!("Debug-Capture: {}", mode.sends_desc());
                    // Discover also runs the occupied/text scan (read-only queries).
                    if mode == telenot_app::app::CaptureMode::Discover {
                        runtime.start_capture_scan(now, panel_kind);
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
                    log::info!("Debug-Capture gestoppt");
                }
                Intent::Command { cmd, pin: given } => {
                    // SECURITY: while a debug capture is active ALL commands are hard-blocked
                    // (belt-and-suspenders — the external panel must never be switched).
                    if app.lock().unwrap().capture.active {
                        runtime.note_command_denied(now, "Debug-Capture aktiv");
                        log::warn!("Befehl während Debug-Capture abgelehnt");
                        continue;
                    }
                    let decision = {
                        let mut a = app.lock().unwrap();
                        let hk = a.setup.mqtt.homekit_mode;
                        let remote = a.setup.remote_disarm;
                        authorize_command(cmd, hk, remote, given.as_deref(), a.services.as_mut())
                    };
                    match decision {
                        CommandAuth::Allow => {
                            let acts = runtime.on_command(now, cmd);
                            apply_actions(acts, transport.as_mut(), &mut sink, &nvs, &app);
                        }
                        CommandAuth::Deny(reason) => {
                            // Visible in the live test board (GET /state), not just the log.
                            runtime.note_command_denied(now, &reason);
                            log::warn!("Befehl abgelehnt: {reason}");
                        }
                    }
                }
                Intent::ReloadConfig(cfg) => match cfg_store.save(&cfg) {
                    Err(e) => {
                        // Fail-closed: do NOT apply — otherwise it looks saved but is gone
                        // after a reboot (this is exactly how a config was already lost here).
                        // Visible in the diagnostics log, not only on serial.
                        log::error!("Config speichern: {e} — Änderungen NICHT übernommen");
                        let mut a = app.lock().unwrap();
                        let _ = nvs.set_str("cfg_error", &e.chars().take(180).collect::<String>());
                        a.finish_commit(&cfg, Err(e.clone()));
                        a.ring.push(
                            now,
                            "error",
                            format!(
                                "Config speichern fehlgeschlagen ({e}) — Änderungen NICHT \
                                 übernommen, Gerät läuft mit der alten Config weiter"
                            ),
                        );
                    }
                    Ok(dual_write) => {
                        {
                            let mut a = app.lock().unwrap();
                            a.device.configured = !cfg.sensors.is_empty();
                            // Steady state holds ONE sensor table: app and core share the Arc.
                            a.persisted = std::sync::Arc::clone(&cfg);
                            let _ = nvs.remove("cfg_error");
                            a.finish_commit(&cfg, Ok(()));
                            // The commit dialog answers BEFORE this write — the ring is the
                            // only place the user can verify the config really hit flash.
                            a.ring.push(
                                now,
                                "info",
                                format!("Config gespeichert: {} Sensoren", cfg.sensors.len()),
                            );
                            // The legacy dual-write blob was dropped (inventory above the
                            // legacy budget) — a firmware downgrade would boot unconfigured.
                            // The user must know BEFORE downgrading, hence the ring warning.
                            if matches!(dual_write, crate::storage::DualWrite::Dropped) {
                                a.ring.push(
                                    now,
                                    "warn",
                                    "Inventar über Legacy-Limit — ein Firmware-Downgrade auf \
                                     Stände vor dem Chunked-Storage würde die Konfiguration \
                                     verlieren. Vorher Sicherung exportieren."
                                        .to_string(),
                                );
                            }
                        }
                        // Config lives only in the core afterwards (runtime.config() borrows it) —
                        // a third copy in the loop was part of the boot OOM.
                        if sink.is_connected() {
                            for old in runtime.config().sensors.iter().filter(|s| s.confirmed()) {
                                if !cfg
                                    .sensors
                                    .iter()
                                    .any(|s| s.confirmed() && s.address() == old.address())
                                {
                                    sink.publish_raw(
                                        &format!(
                                            "homeassistant/binary_sensor/{0}/{0}_{1:04x}/config",
                                            telenot_app::hadisco::HA_ID,
                                            old.address()
                                        ),
                                        "",
                                        true,
                                    );
                                    // Switchable outputs get a second (switch) entity in
                                    // HA discovery — clear it too, no orphaned entities.
                                    if old.switchable() {
                                        sink.publish_raw(
                                            &format!(
                                                "homeassistant/switch/{0}/{0}_{1:04x}_switch/config",
                                                telenot_app::hadisco::HA_ID,
                                                old.address()
                                            ),
                                            "",
                                            true,
                                        );
                                    }
                                }
                            }
                        }
                        runtime.reload(cfg, telenot_app::security::disarm_enabled(remote, pin_set));
                        // Align discovery/inventory with the new config (retained).
                        if sink.is_connected() {
                            sink.request_setup();
                            publish_setup(&mut sink, &mqtt, &runtime, &mut last_inventory_chunks);
                        }
                        log::info!("Config neu geladen + persistiert");
                    }
                },
                Intent::CheckConnection { ip, port } => {
                    let app2 = Arc::clone(&app);
                    let spawned = std::thread::Builder::new()
                        .name("conn-probe".into())
                        .stack_size(12 * 1024)
                        .spawn(move || {
                            let (connected, data_seen) = crate::transport::probe_tcp(&ip, port);
                            app2.lock().unwrap().conn_check =
                                ConnCheckView::from_probe(connected, data_seen);
                        });
                    if let Err(e) = spawned {
                        log::error!("Probe-Thread: {e}");
                        app.lock().unwrap().conn_check = ConnCheckView::from_probe(false, false);
                    }
                }
                Intent::TestMqtt(target) => {
                    // Probe in a worker thread with a generous stack (TLS handshake) — never
                    // blocks the serial owner. Mirrors telenot-sim serve, plus TOFU cert fetch.
                    let app2 = Arc::clone(&app);
                    let spawned = std::thread::Builder::new()
                        .name("mqtt-probe".into())
                        .stack_size(24 * 1024)
                        .spawn(move || {
                            // Layered classification (Ok/Tls/Auth/Connect) + retrieve the
                            // presented cert for pinning when TLS is untrusted.
                            let (result, cert) = crate::mqtt::probe_and_classify(&target);
                            let mut a = app2.lock().unwrap();
                            let ok = result.ok();
                            a.mqtt_test = MqttTestView {
                                state: TestState::Done,
                                result: Some(result),
                                cert,
                            };
                            if ok {
                                a.setup.mqtt_tested = true;
                            }
                        });
                    if let Err(e) = spawned {
                        log::error!("MQTT-Probe-Thread: {e}");
                        app.lock().unwrap().mqtt_test = MqttTestView {
                            state: TestState::Done,
                            result: Some(telenot_app::MqttTestResult::Connect),
                            cert: None,
                        };
                    }
                }
                Intent::HomekitCommand { cmd } => {
                    // SECURITY: no switching during a debug capture (same as REST/MQTT).
                    if app.lock().unwrap().capture.active {
                        log::warn!("HomeKit-Befehl während Debug-Capture abgelehnt");
                        continue;
                    }
                    // Dedicated HomeKit authorisation path: the paired HAP device is the proof
                    // (no per-action PIN). Disarm only if enabled in setup (fail-closed); arm/reset
                    // are free — the core remains additionally pre-arm-gated.
                    let allow = {
                        let a = app.lock().unwrap();
                        homekit_authorized(
                            cmd,
                            a.setup.mqtt.homekit_mode,
                            a.setup.mqtt.homekit_disarm,
                        )
                    };
                    if allow {
                        let acts = runtime.on_command(now, cmd);
                        apply_actions(acts, transport.as_mut(), &mut sink, &nvs, &app);
                    } else {
                        log::warn!("HomeKit-Befehl abgelehnt: Unscharf nicht freigegeben");
                    }
                }
                Intent::StartHomekit => {
                    // Live start (no reboot); idempotent — the HAP task runs only once. The
                    // currently marked detectors are created as accessory services.
                    crate::homekit::start(app.clone(), runtime.config().clone());
                }
                Intent::Reboot => {
                    log::warn!("Neustart angefordert …");
                    FreeRtos::delay_ms(300); // HTTP-Antwort noch rauslassen
                    unsafe { esp_idf_svc::sys::esp_restart() };
                }
                Intent::ExtendSetupWindow => {
                    // Web extension like BUT1, but capped at setup_cap (24 h since boot) and
                    // never shortening the current window. Reopens the HTTP server if needed.
                    setup_until = (now + WINDOW_MS).min(setup_cap).max(setup_until);
                    if http.is_none() && now < setup_until {
                        http = start_http(&app);
                        log::info!("Setup-Fenster per Web verlängert (30 min, gedeckelt)");
                    } else {
                        log::info!("Setup-Fenster per Web aufgefrischt (gedeckelt)");
                    }
                }
                Intent::ApplyHomekit => {
                    // Stage → apply (without reboot): ensure HAP is idempotently running and
                    // signal the HAP thread to reconcile the bridged accessory set against the
                    // (just persisted) config. The reconcile itself runs in the HAP thread.
                    crate::homekit::start(app.clone(), runtime.config().clone());
                    app.lock().unwrap().homekit_reconcile = true;
                }
                other => runtime.apply_intent(now, other),
            }
        }

        // 2. Read EMA → feed Runtime → execute actions.
        match transport.read_chunk() {
            Ok(Some(chunk)) if !chunk.is_empty() => {
                let mut acts = runtime.feed(now, &chunk);
                // Debug capture: record raw RX bytes; in "listen only" mode suppress any
                // transmission to the (external) panel.
                {
                    let mut a = app.lock().unwrap();
                    a.capture.record_rx(now_ms(), &chunk);
                    if a.capture.active && a.capture.mode.suppresses_tx() {
                        acts.retain(|act| !matches!(act, Action::SendFrame(_)));
                    }
                }
                apply_actions(acts, transport.as_mut(), &mut sink, &nvs, &app);
            }
            Ok(Some(_)) => {
                let acts = runtime.tick(now);
                apply_actions(acts, transport.as_mut(), &mut sink, &nvs, &app);
            }
            Ok(None) => {
                log::warn!("Panel-Transport geschlossen (EOF) — beende Loop-Iteration, warte");
                FreeRtos::delay_ms(1000);
            }
            Err(e) => {
                log::error!("EMA-Lesefehler: {e}");
                FreeRtos::delay_ms(1000);
            }
        }

        // 3. Mirror live snapshot + runtime diagnostics (MQTT link + heap) + seed scan result.
        {
            let mut a = app.lock().unwrap();
            a.live = runtime.snapshot();
            a.mqtt_connected = sink.is_connected();
            a.mqtt_publish_errors = sink.publish_errors;
            a.mqtt_reconnects = sink.reconnects();
            a.mqtt_setup_pending = sink.setup_pending();
            a.mqtt_discovery_count = runtime
                .config()
                .sensors
                .iter()
                .filter(|s| s.confirmed())
                .count();
            let stats = heap_stats();
            a.heap = Some(stats);
            // Heap warning threshold (latch, once per boot): low-water mark < 40 KB or largest
            // free block < 32 KB = critically close to OOM → make visible BEFORE it crashes
            // (ring log for the web UI, heap_low for MQTT diagnostics → HA alert).
            let (_, largest, low) = stats;
            if !a.heap_low && (low < 40 * 1024 || largest < 32 * 1024) {
                a.heap_low = true;
                a.ring.push(
                    now,
                    "warn",
                    format!(
                        "Heap-Warnschwelle unterschritten: Tiefpunkt {} KB, größter Block {} KB",
                        low / 1024,
                        largest / 1024
                    ),
                );
                log::warn!("Heap-Warnschwelle: low={low} largest={largest}");
            }
            if let Some((cfg, raw)) = runtime.take_scan_result() {
                a.setup.seed(cfg, raw);
            }
        }

        // 4. MQTT availability/diagnostics (retained), only after confirmed serial liveness.
        if sink.is_connected() {
            let announce = {
                let a = app.lock().unwrap();
                !online_announced
                    && a.live.availability == "online"
                    && a.live.last_frame_ms_ago.is_some()
            };
            if announce {
                sink.publish("availability", "online", true);
                sink.publish("diagnostics", &diagnostics_payload(&app, now), true);
                last_diag_ms = now;
                online_announced = true;
            }
            if online_announced && now.saturating_sub(last_diag_ms) >= 30_000 {
                last_diag_ms = now;
                sink.publish("diagnostics", &diagnostics_payload(&app, now), true);
            }
        }

        // Periodic status report on the console (bench verification).
        if now.saturating_sub(last_report_ms) >= 2000 {
            last_report_ms = now;
            let a = app.lock().unwrap();
            let l = &a.live;
            log::info!(
                "STATUS arm={} avail={} last_frame={:?}ms intern_ready={:?} extern_ready={:?} sensoren={} mqtt={}",
                l.arm_state,
                l.availability,
                l.last_frame_ms_ago,
                l.intern_ready,
                l.extern_ready,
                l.sensor_states.len(),
                sink.is_connected(),
            );
        }

        FreeRtos::delay_ms(80);
    }
}

/// Publishes HA discovery (if `ha_discovery` enabled) + the inventory manifest (retained).
/// Mirrors `publish_discovery`/`publish_inventory` in the host daemon.
/// `last_inventory_chunks` tracks the chunk count of the previous publish so a shrinking
/// inventory clears its orphaned retained `inventory/chunk/<i>` topics (empty payload).
/// Chunks left over from BEFORE a reboot are unknown here and stay — accepted per contract
/// (consumers must honor `chunk_count` from the envelope).
fn publish_setup(
    sink: &mut EspMqttSink,
    m: &MqttSettings,
    runtime: &Runtime,
    last_inventory_chunks: &mut usize,
) {
    let config = runtime.config();
    sink.begin_setup_pass();
    let mut index = 0;
    if m.ha_discovery {
        if config.panel.kind == telenot_config::PanelKind::Hiplex8400
            && config.panel.gms_variant == telenot_config::GmsVariant::Plus
        {
            // Remove the legacy complex-only entities after switching panel profiles.
            for (platform, suffix) in [
                ("alarm_control_panel", "alarm"),
                ("binary_sensor", "ready_intern"),
                ("binary_sensor", "ready_extern"),
            ] {
                sink.setup_publish(
                    &mut index,
                    &format!(
                        "homeassistant/{platform}/{0}/{0}_{suffix}/config",
                        telenot_app::hadisco::HA_ID
                    ),
                    "",
                    true,
                    false,
                );
            }
        }
        let mut opts = telenot_app::DiscoveryOpts::new(m.topic_root.clone());
        opts.sw_version = env!("CARGO_PKG_VERSION").into();
        opts.controllable = true; // controllable alarm_control_panel; command path is fail-closed
        opts.areas = telenot_app::areas_from_config(config); // hiplex: per-area entities
                                                             // STREAM, never collect: all ~150 discovery strings at once (~120 KB) caused
                                                             // an OOM loop at boot with a full config.
        telenot_app::discovery_for_each(config, &opts, &mut |msg| {
            sink.setup_publish(&mut index, &msg.topic, &msg.payload, msg.retain, false);
        });
    }
    // STREAM the inventory too: above the threshold each chunk string is built, published,
    // dropped — the single ~112 KB string at 500 sensors would hit the heap wall.
    let mut chunks = 0usize;
    telenot_app::inventory_for_each(config, &mut |topic, payload| {
        sink.setup_publish(&mut index, topic, payload, true, true);
        if topic != "inventory" {
            chunks += 1;
        }
    });
    // Shrink (fewer chunks, or chunked→single with chunks == 0): clear orphaned retained
    // chunk topics with an empty payload.
    for i in chunks..*last_inventory_chunks {
        sink.setup_publish(&mut index, &format!("inventory/chunk/{i}"), "", true, true);
    }
    runtime.republish_for_each(&mut |action| {
        if let Action::Publish {
            topic,
            payload,
            retain,
        } = action
        {
            sink.setup_publish(&mut index, &topic, &payload, retain, true);
        }
    });
    sink.end_setup_pass(index);
    if !sink.setup_pending() {
        *last_inventory_chunks = chunks;
    }
}

/// Heap figures in bytes: `(free, largest free block, low-water mark since boot)`. Plain
/// counter reads from the IDF heap allocator (cheap, non-critical in the 80 ms loop).
fn heap_stats() -> (u64, u64, u64) {
    use esp_idf_svc::sys::{
        esp_get_free_heap_size, esp_get_minimum_free_heap_size, heap_caps_get_largest_free_block,
        MALLOC_CAP_8BIT,
    };
    unsafe {
        (
            esp_get_free_heap_size() as u64,
            heap_caps_get_largest_free_block(MALLOC_CAP_8BIT) as u64,
            esp_get_minimum_free_heap_size() as u64,
        )
    }
}

/// Retained diagnostics JSON for the `diagnostics` topic (field diagnostics without web UI).
fn diagnostics_payload(app: &Arc<Mutex<App>>, now: u64) -> String {
    let a = app.lock().unwrap();
    let online = a.live.availability == "online";
    serde_json::json!({
        "serial": { "status": if online { "ok" } else { "stale" }, "last_frame_ms": a.live.last_frame_ms_ago },
        "uptime_s": now / 1000,
        "firmware_version": env!("CARGO_PKG_VERSION"),
        "schema_version": telenot_config::CURRENT_SCHEMA_VERSION,
        "reconnects": a.mqtt_reconnects,
        "mqtt_connected": a.mqtt_connected,
        "mqtt_publish_errors": a.mqtt_publish_errors,
        "mqtt_setup_pending": a.mqtt_setup_pending,
        "confirmed_sensors": a.mqtt_discovery_count,
        // Stability signals for HA: reboot forensics + heap early warning.
        "boot_reason": a.boot_reason,
        "boot_count": a.boot_count,
        "heap_low": a.heap_low,
        "heap_free_kib": a.heap.map(|(free, _, _)| free / 1024),
        "heap_low_water_kib": a.heap.map(|(_, _, low)| low / 1024),
    })
    .to_string()
}

fn apply_actions(
    actions: Vec<Action>,
    transport: &mut dyn PanelTransport,
    sink: &mut EspMqttSink,
    nvs: &EspDefaultNvs,
    app: &Arc<Mutex<App>>,
) {
    for a in actions {
        match a {
            Action::SendFrame(bytes) => {
                let result = transport.write_frame(&bytes);
                app.lock()
                    .unwrap()
                    .capture
                    .record_tx(now_ms(), &bytes, result.is_ok());
                if let Err(e) = result {
                    log::error!("Panel-TX fehlgeschlagen: {e}");
                }
            }
            Action::Publish {
                topic,
                payload,
                retain,
            } => {
                sink.publish(&topic, &payload, retain);
            }
            Action::PersistNightFlag(on) => {
                let _ = nvs.set_u8("night", on as u8);
            }
            Action::Log(msg) => {
                log::info!("core: {msg}");
            }
        }
    }
}

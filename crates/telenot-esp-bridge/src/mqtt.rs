//! Slice 6: MQTT client (`esp-mqtt`) — implements `telenot_app::MqttSink` and mirrors the
//! host daemon (`telenot-sim` `HostSink`), but with `EspMqttClient` instead of `rumqttc`.
//!
//! - `publish`/`publish_raw`: retained/live publishes (outbox = `enqueue`, never blocks the
//!   poll loop).
//! - LWT `availability=offline` (retained) on unplanned disconnect.
//! - Command topic subscribed → incoming commands enqueued ONLY as `Intent::Command` (never
//!   directly into the core); disarm stays fail-closed (PIN + remote disarm checked in loop).
//! - `test_connection`: one-shot probe in a worker thread (never in the serial owner).

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use esp_idf_svc::mqtt::client::{
    EspMqttClient, EventPayload, LwtConfiguration, MqttClientConfiguration, QoS,
};
use esp_idf_svc::sys::EspError;
use esp_idf_svc::tls::X509;
use telenot_app::app::PendingCert;
use telenot_app::mqtt::{MqttSink, MqttTarget, MqttTestResult};
use telenot_app::{App, Intent};

/// Turns a PEM into a `'static` NUL-terminated buffer for `X509::pem_until_nul`. Intentionally
/// leaked (the C mqtt/tls copies the cert on connect; (re)connects happen only on settings
/// changes, i.e. rarely — the leak is negligible).
fn leak_pem_nul(pem: &str) -> &'static [u8] {
    let mut v = pem.as_bytes().to_vec();
    if !v.ends_with(&[0]) {
        v.push(0);
    }
    Box::leak(v.into_boxed_slice())
}

/// Broker URI from host/port/TLS (`mqtt://` or `mqtts://`).
fn broker_url(host: &str, port: u16, tls: bool) -> String {
    let scheme = if tls { "mqtts" } else { "mqtt" };
    format!("{scheme}://{host}:{port}")
}

/// TLS configuration for `MqttClientConfiguration`:
/// - **pinned cert** → verify against exactly this cert (self-signed + MITM-resistant).
/// - otherwise → public CA bundle (`esp_crt_bundle_attach`).
///
/// There is deliberately no "skip verification" mode: esp-mqtt has no opt-out for chain
/// verification (`skip_cert_common_name_check` only skips the hostname check), so an
/// insecure mode could never connect anyway — self-signed brokers go through TOFU pinning.
fn tls_conf(conf: &mut MqttClientConfiguration, tls: bool, pinned: Option<&'static [u8]>) {
    if !tls {
        return;
    }
    if let Some(pem_nul) = pinned {
        conf.server_certificate = Some(X509::pem_until_nul(pem_nul));
        conf.skip_cert_common_name_check = true; // self-signed CN rarely matches the host
    } else {
        conf.crt_bundle_attach = Some(esp_idf_svc::sys::esp_crt_bundle_attach);
    }
}

/// Poll-loop MQTT sink. `client == None` = log-only (MQTT not configured).
pub struct EspMqttSink {
    root: String,
    client: Option<EspMqttClient<'static>>,
    /// Set by the event pump on every Connect. Drained by [`Self::poll_resubscribe`] on the
    /// POLL-LOOP thread — subscribing from the `conn.next()` pump thread deadlocks the
    /// esp-idf-svc client, so the pump only signals and the loop does the subscribe.
    resub: Arc<AtomicBool>,
    cmd_topic: String,
    area_cmd_topic: String,
}

impl EspMqttSink {
    /// Log-only sink (no broker) — while MQTT is not configured.
    pub fn disconnected(root: String) -> Self {
        Self {
            root,
            client: None,
            resub: Arc::new(AtomicBool::new(false)),
            cmd_topic: String::new(),
            area_cmd_topic: String::new(),
        }
    }

    /// (Re)subscribes the command topics after a (re)connect. MUST be called from the poll loop
    /// (owner of the client), NEVER from the `conn.next()` event pump — the pump only sets the
    /// flag. This is the fix for the deadlock/"HA commands never reached the bridge" bug.
    pub fn poll_resubscribe(&mut self) {
        if self.resub.swap(false, Ordering::SeqCst) {
            if let Some(c) = self.client.as_mut() {
                let _ = c.subscribe(&self.cmd_topic, QoS::AtLeastOnce);
                let _ = c.subscribe(&self.area_cmd_topic, QoS::AtLeastOnce);
                log::info!(
                    "MQTT subscribed: {}, {}",
                    self.cmd_topic,
                    self.area_cmd_topic
                );
            }
        }
    }

    /// Connects to the broker: LWT `availability=offline` retained, command topic subscribed,
    /// event pump in its own thread (keeps the client alive + forwards commands as intents).
    /// `app` is shared ONLY for incoming commands.
    #[allow(clippy::too_many_arguments)]
    pub fn connect(
        root: String,
        host: &str,
        port: u16,
        tls: bool,
        username: &str,
        password: Option<&str>,
        pinned_cert: Option<&str>,
        app: Arc<Mutex<App>>,
    ) -> Result<Self, EspError> {
        let url = broker_url(host, port, tls);
        let lwt_topic = format!("{root}/availability");
        let cmd_topic = format!("{root}/command");
        let user = (!username.is_empty()).then_some(username);
        let pinned = pinned_cert.map(leak_pem_nul);

        let mut conf = MqttClientConfiguration {
            client_id: Some("telenot-esp-bridge"),
            keep_alive_interval: Some(Duration::from_secs(15)),
            lwt: Some(LwtConfiguration {
                topic: &lwt_topic,
                payload: b"offline",
                qos: QoS::AtLeastOnce,
                retain: true,
            }),
            username: user,
            password: password.filter(|_| user.is_some()),
            ..Default::default()
        };
        tls_conf(&mut conf, tls, pinned);

        let (client, mut conn) = EspMqttClient::new(&url, &conf)?;
        // esp-mqtt connects ASYNCHRONOUSLY. Two traps: (1) subscribing right after `new()` races
        // the connect and fails silently (publish is unaffected — `enqueue` buffers); (2)
        // subscribing from THIS pump thread (the one draining `conn.next()`) DEADLOCKS the
        // esp-idf-svc client. So the pump only raises a flag on Connect; the poll loop does the
        // actual subscribe via `poll_resubscribe` (it owns the client).
        let resub = Arc::new(AtomicBool::new(false));
        let resub_pump = Arc::clone(&resub);

        // Event pump: MUST run, otherwise the client blocks. Signals (re)subscribe on Connect +
        // forwards command publishes as intents (the loop authorises fail-closed).
        let cmd_for_thread = cmd_topic.clone();
        // Multi-area panels (hiplex): {root}/area/{id}/command routes arm commands to the
        // given Sicherungsbereich; same payload contract and authorization as {root}/command.
        let area_prefix = format!("{root}/area/");
        let _ = std::thread::Builder::new()
            .name("mqtt-ev".into())
            // 8 KB instead of 6: TLS handshake callbacks run in this task — 6 KB was marginal
            // (a silent stack overflow would reboot without a useful log). Deliberately paying
            // 2 KB of heap for stability.
            .stack_size(8 * 1024)
            .spawn(move || {
                while let Ok(event) = conn.next() {
                    match event.payload() {
                        // Signal (re)subscribe on every (re)connect — never touch the client here.
                        EventPayload::Connected(_) => resub_pump.store(true, Ordering::SeqCst),
                        EventPayload::Received {
                            topic: Some(t),
                            data,
                            ..
                        } => {
                            // {root}/command → area None; {root}/area/{id}/command → Some(id).
                            let area = if t == cmd_for_thread {
                                Some(None)
                            } else {
                                t.strip_prefix(&area_prefix)
                                    .and_then(|rest| rest.strip_suffix("/command"))
                                    .and_then(|id| id.parse::<u8>().ok())
                                    .map(Some)
                            };
                            if let Some(area) = area {
                                // The ESP wrapper does not expose the retain flag → treat as a live
                                // command; disarm stays fail-closed via PIN + remote disarm.
                                let panel_kind = app.lock().unwrap().persisted.panel.kind;
                                match telenot_app::parse_command_message(data, false, panel_kind)
                                {
                                    Ok((cmd, pin)) => {
                                        // Route arm commands from an area topic to that area;
                                        // bypass/output are area-agnostic and pass unchanged.
                                        let cmd = match (cmd, area) {
                                            (telenot_app::BridgeCommand::Arm(a), Some(id))
                                                if id != 1 =>
                                            {
                                                telenot_app::BridgeCommand::ArmArea {
                                                    cmd: a,
                                                    area: id,
                                                }
                                            }
                                            (c, _) => c,
                                        };
                                        app.lock()
                                            .unwrap()
                                            .intents
                                            .push(Intent::Command { cmd, pin })
                                    }
                                    Err(reason) => log::warn!("MQTT-Command verworfen: {reason}"),
                                }
                            }
                        }
                        _ => {}
                    }
                }
                log::info!("MQTT-Event-Pump beendet");
            });

        let area_cmd_topic = format!("{root}/area/+/command");
        log::info!("MQTT verbunden: {url}");
        Ok(Self {
            root,
            client: Some(client),
            resub,
            cmd_topic,
            area_cmd_topic,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_some()
    }
}

impl MqttSink for EspMqttSink {
    fn publish(&mut self, topic: &str, payload: &str, retain: bool) {
        if let Some(c) = self.client.as_mut() {
            let full = format!("{}/{topic}", self.root);
            let _ = c.enqueue(&full, QoS::AtLeastOnce, retain, payload.as_bytes());
        }
    }
    fn publish_raw(&mut self, topic: &str, payload: &str, retain: bool) {
        if let Some(c) = self.client.as_mut() {
            let _ = c.enqueue(topic, QoS::AtLeastOnce, retain, payload.as_bytes());
        }
    }
    fn test_connection(&mut self, target: &MqttTarget) -> MqttTestResult {
        probe_mqtt(target)
    }
}

/// Single connection probe (runs in a worker thread). One MQTT connect attempt with the
/// configured TLS policy. Returns `true` if `Connected`.
fn mqtt_connect_ok(t: &MqttTarget) -> bool {
    let url = broker_url(&t.host, t.port, t.tls);
    let user = (!t.username.is_empty()).then_some(t.username.as_str());
    let mut conf = MqttClientConfiguration {
        client_id: Some("telenot-probe"),
        keep_alive_interval: Some(Duration::from_secs(10)),
        network_timeout: Duration::from_secs(8),
        username: user,
        password: t.password.as_deref().filter(|_| user.is_some()),
        ..Default::default()
    };
    let pinned = t.pinned_cert.as_deref().map(leak_pem_nul);
    tls_conf(&mut conf, t.tls, pinned);
    let (_client, mut conn) = match EspMqttClient::new(&url, &conf) {
        Ok(v) => v,
        Err(_) => return false,
    };
    for _ in 0..30 {
        match conn.next() {
            Ok(ev) => match ev.payload() {
                EventPayload::Connected(_) => return true,
                EventPayload::Error(_) | EventPayload::Disconnected => return false,
                _ => {}
            },
            Err(_) => return false,
        }
    }
    false
}

/// Plain TCP reachability check (plaintext case only, to distinguish auth failures from "host gone").
fn tcp_reachable(host: &str, port: u16) -> bool {
    format!("{host}:{port}")
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .map(|sa| TcpStream::connect_timeout(&sa, Duration::from_secs(5)).is_ok())
        .unwrap_or(false)
}

/// Connection test WITH cause classification. Because the ESP mqtt Rust wrapper does not
/// expose a detailed failure reason, we probe in layers ourselves:
/// - MQTT `Connected` → `Ok` (+ the presented cert so the UI can show the fingerprint).
/// - Host not even TCP-reachable → `Connect`.
/// - TLS: trust is probed separately from credentials — pinned broker: fetched peer cert
///   equals the pin → `Auth`, differs → `Tls` + cert ("certificate changed, re-pin?");
///   unpinned: a *verified* handshake against the CA bundle succeeds → `Auth`, fails →
///   `Tls` + presented cert for TOFU pinning.
/// - Plaintext: TCP reachable → `Auth`.
pub fn probe_and_classify(t: &MqttTarget) -> (MqttTestResult, Option<PendingCert>) {
    if t.host.trim().is_empty() {
        return (MqttTestResult::Connect, None);
    }
    if mqtt_connect_ok(t) {
        // Show the server cert fingerprint even on success (inspection + optional pinning).
        let cert = if t.tls {
            fetch_peer_cert(&t.host, t.port)
        } else {
            None
        };
        return (MqttTestResult::Ok, cert);
    }
    // MQTT connect failed. Check reachability via a plain TCP connect (reliable, independent
    // of TLS cert fetching) — not reachable → host truly gone.
    if !tcp_reachable(&t.host, t.port) {
        return (MqttTestResult::Connect, None);
    }
    if !t.tls {
        return (MqttTestResult::Auth, None);
    }
    if let Some(pinned) = t.pinned_cert.as_deref() {
        // Pinned broker: same cert as pinned → trust is fine, credentials are wrong.
        // Different cert → rotation/MITM; hand it to the UI for an explicit re-pin.
        return match fetch_peer_cert(&t.host, t.port) {
            Some(cert) if cert.pem.trim() == pinned.trim() => (MqttTestResult::Auth, None),
            Some(cert) => (MqttTestResult::Tls, Some(cert)),
            // TCP reachable but no TLS handshake at all → port speaks no TLS.
            None => (MqttTestResult::Tls, None),
        };
    }
    // Unpinned: a verified handshake decides whether the chain is the problem.
    if tls_handshake_ca_ok(&t.host, t.port) {
        return (MqttTestResult::Auth, None);
    }
    let cert = fetch_peer_cert(&t.host, t.port);
    (MqttTestResult::Tls, cert)
}

/// Verified TLS handshake probe against the public CA bundle. `true` = the broker's chain
/// is trusted → a failing MQTT connect is a credential problem, not a cert problem.
fn tls_handshake_ca_ok(host: &str, port: u16) -> bool {
    use esp_idf_svc::sys;
    let Ok(host_c) = std::ffi::CString::new(host) else {
        return false;
    };
    unsafe {
        let mut cfg: sys::esp_tls_cfg_t = core::mem::zeroed();
        cfg.timeout_ms = 8000;
        cfg.crt_bundle_attach = Some(sys::esp_crt_bundle_attach);
        let tls = sys::esp_tls_init();
        if tls.is_null() {
            log::warn!("TLS-Probe: esp_tls_init NULL (Heap zu knapp?)");
            return false;
        }
        let ret =
            sys::esp_tls_conn_new_sync(host_c.as_ptr(), host.len() as i32, port as i32, &cfg, tls);
        sys::esp_tls_conn_destroy(tls);
        ret == 1
    }
}

/// Result only (for the `MqttSink` trait).
pub fn probe_mqtt(t: &MqttTarget) -> MqttTestResult {
    probe_and_classify(t).0
}

/// TOFU: fetches the certificate presented by the broker via an UNVERIFIED TLS handshake
/// (esp-tls/mbedtls — the high-level MQTT API does not expose the peer cert). Returns
/// fingerprint (SHA-256) + subject/issuer + PEM for pinning. `None` if no TLS handshake
/// succeeds (then it is not "untrusted TLS" but unreachable/no TLS broker).
///
/// SAFETY: deliberately without verification (Trust-On-First-Use) — the fingerprint is shown
/// to the user for inspection before pinning.
pub fn fetch_peer_cert(host: &str, port: u16) -> Option<PendingCert> {
    use esp_idf_svc::sys;
    let host_c = std::ffi::CString::new(host).ok()?;
    unsafe {
        // NO crt_bundle_attach/cacert in cfg → esp-tls does NOT verify the chain
        // (VERIFY_NONE) → the handshake succeeds even for self-signed certs. The cert is
        // only read out (NO pinning without user confirmation); MBEDTLS_SSL_KEEP_PEER_CERTIFICATE
        // (sdkconfig) ensures mbedtls retains it after the handshake.
        let mut cfg: sys::esp_tls_cfg_t = core::mem::zeroed();
        cfg.skip_common_name = true;
        cfg.timeout_ms = 8000;

        let tls = sys::esp_tls_init();
        if tls.is_null() {
            log::warn!("TOFU: esp_tls_init NULL (Heap zu knapp?)");
            return None;
        }
        let ret =
            sys::esp_tls_conn_new_sync(host_c.as_ptr(), host.len() as i32, port as i32, &cfg, tls);
        let out = fetch_from_tls(tls, ret, host, port);
        sys::esp_tls_conn_destroy(tls);
        out
    }
}

/// Inner part (after connect): extract the peer cert from the mbedtls context. Logs every
/// error branch so failures are immediately visible on the serial monitor.
unsafe fn fetch_from_tls(
    tls: *mut esp_idf_svc::sys::esp_tls,
    ret: i32,
    host: &str,
    port: u16,
) -> Option<PendingCert> {
    use esp_idf_svc::sys;
    if ret != 1 {
        log::warn!("TOFU: TLS-Handshake zu {host}:{port} fehlgeschlagen (ret={ret})");
        return None;
    }
    let ssl = sys::esp_tls_get_ssl_context(tls) as *const sys::mbedtls_ssl_context;
    if ssl.is_null() {
        log::warn!("TOFU: kein mbedtls-SSL-Kontext");
        return None;
    }
    let cert = sys::mbedtls_ssl_get_peer_cert(ssl);
    if cert.is_null() {
        log::warn!("TOFU: Peer-Cert NULL — MBEDTLS_SSL_KEEP_PEER_CERTIFICATE aktiv?");
        return None;
    }
    let raw_p = (*cert).raw.p as *const u8;
    let raw_len = (*cert).raw.len;
    if raw_p.is_null() || raw_len == 0 {
        log::warn!("TOFU: Peer-Cert-DER leer (len={raw_len})");
        return None;
    }
    let der = core::slice::from_raw_parts(raw_p, raw_len);
    log::info!("TOFU: Peer-Cert geholt ({raw_len} B DER) von {host}:{port}");
    Some(PendingCert {
        sha256: sha256_hex(der),
        subject: dn_string(&(*cert).subject),
        issuer: dn_string(&(*cert).issuer),
        pem: der_to_pem(der),
    })
}

/// SHA-256 (mbedtls) as an `AA:BB:…` hex fingerprint.
fn sha256_hex(data: &[u8]) -> String {
    let mut out = [0u8; 32];
    unsafe {
        esp_idf_svc::sys::mbedtls_sha256(data.as_ptr(), data.len(), out.as_mut_ptr(), 0);
    }
    out.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// mbedtls distinguished name (subject/issuer) as a human-readable string.
fn dn_string(dn: *const esp_idf_svc::sys::mbedtls_x509_name) -> String {
    let mut buf = [0 as core::ffi::c_char; 256];
    unsafe {
        let n = esp_idf_svc::sys::mbedtls_x509_dn_gets(buf.as_mut_ptr(), buf.len(), dn);
        if n < 0 {
            return String::new();
        }
        core::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// DER → PEM (standard certificate format, 64-character lines) — for storing/pinning.
fn der_to_pem(der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap());
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

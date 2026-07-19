//! MQTT sink as a trait — the real implementation lives in the binary (host: `rumqttc` or
//! a stdout stub; firmware: `esp-mqtt`). The service layer only knows the trait, keeping
//! `telenot-app` free of network/async dependencies.

/// Connection target for an MQTT test (owned so it can travel as an Intent).
#[derive(Debug, Clone)]
pub struct MqttTarget {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: String,
    pub password: Option<String>,
    /// Pinned broker certificate (PEM). When set, verify against exactly this cert; otherwise
    /// the presented cert is retrieved for TOFU pinning on the first untrusted TLS connection.
    pub pinned_cert: Option<String>,
}

/// Result of a connection test — variants match the frontend i18n keys
/// (`s5.test.ok|connect|tls|auth`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttTestResult {
    Ok,
    /// Host unreachable (DNS/port).
    Connect,
    /// TLS or certificate error.
    Tls,
    /// Authentication rejected.
    Auth,
}

impl MqttTestResult {
    pub fn detail(&self) -> &'static str {
        match self {
            MqttTestResult::Ok => "ok",
            MqttTestResult::Connect => "connect",
            MqttTestResult::Tls => "tls",
            MqttTestResult::Auth => "auth",
        }
    }
    pub fn ok(&self) -> bool {
        matches!(self, MqttTestResult::Ok)
    }
}

/// Publish path + connection test. Implemented by the daemon/firmware.
pub trait MqttSink: Send {
    /// Publish to `<root>/<topic>` (the sink knows the root; topics are device-free).
    fn publish(&mut self, topic: &str, payload: &str, retain: bool);
    /// Publish to an ABSOLUTE topic (no root prefix) — for HA discovery configs under
    /// `homeassistant/…`, which live outside the device root.
    fn publish_raw(&mut self, topic: &str, payload: &str, retain: bool);
    /// Synchronous connection test (runs in the worker, NEVER in the serial-owner thread).
    fn test_connection(&mut self, target: &MqttTarget) -> MqttTestResult;
}

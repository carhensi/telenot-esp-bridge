//! Portable service/REST layer of the Telenot bridge.
//!
//! Contains the **transport-agnostic** logic of the setup web UI: serde DTOs (the wire
//! contract with the Preact frontend), pure request handlers, and a sans-IO [`Runtime`]
//! that drives `telenot-core` via frames/ticks/intents. No HTTP, no async, no `esp`
//! dependencies — the HTTP shell is interchangeable (host: `tiny_http` in `telenot-sim`;
//! firmware: `esp_http_server`), just as `telenot-sim` swaps `Transport`.
//!
//! Architecture invariants:
//! - **Serial-owner priority:** HTTP handlers NEVER call into the core synchronously. They
//!   read an atomically published [`LiveSnapshot`] and submit [`Intent`]s; the core runs in
//!   its own thread (daemon) / task (firmware).
//! - **Polling, no SSE:** scan/diagnostic progress is served as GET snapshots.
//! - **Secrets write-only:** PIN/MQTT password only flow in via [`Services`], never back out
//!   in a response (only `*_set` booleans are returned).
//! - **Disarm fail-closed:** remote disarm only when explicitly enabled + PIN set.

pub mod api;
pub mod app;
pub mod command;
mod diagnostic_probe;
pub mod dto;
pub mod dup;
pub mod hadisco;
pub mod http_policy;
pub mod inventory;
pub mod mqtt;
pub mod ota;
pub mod polarity;
pub mod runtime;
pub mod security;

pub use app::{App, ConnSettings, EditSession, MqttSettings, SetupState, BAUD_ALLOWED};
pub use command::{
    authorize_command, homekit_authorized, parse_arm_command, parse_bridge_command,
    parse_command_message, BridgeCommand, CommandAuth,
};
pub use dto::{DeviceInfo, HomekitPair};
pub use hadisco::{
    areas_from_config, discovery_for_each, discovery_messages, DiscoveryMsg, DiscoveryOpts,
};
pub use inventory::{
    inventory_for_each, inventory_payload, INVENTORY_CHUNK_ENTITIES, INVENTORY_SINGLE_MAX,
};
pub use mqtt::{MqttSink, MqttTarget, MqttTestResult};
pub use ota::{OtaPhase, OtaState, SelfTest, SlotInfo};
pub use runtime::{Intent, LiveSnapshot, Runtime, ScanMeta, ScanView};
pub use security::{
    ct_eq, disarm_authorized, disarm_enabled, InMemoryServices, PinCheck, Services,
};

/// HTTP method (transport-neutral).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn from_http(s: &str) -> Option<Method> {
        Some(match s.to_ascii_uppercase().as_str() {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            _ => return None,
        })
    }
    /// Mutating methods require CSRF protection + session.
    pub fn is_mutating(&self) -> bool {
        !matches!(self, Method::Get)
    }
}

/// An incoming request, translated from the transport (headers → fields).
#[derive(Debug, Clone)]
pub struct ApiRequest {
    pub method: Method,
    /// Path without query string, e.g. `/api/v1/sensors/0x0042` (leading `/api/v1` is optional).
    pub path: String,
    /// Query string without `?`, e.g. `since_seq=12`.
    pub query: String,
    pub body: Vec<u8>,
    /// Session token from the cookie (`None` = not logged in).
    pub session_token: Option<String>,
    /// `X-CSRF-Token`-Header.
    pub csrf_token: Option<String>,
    /// Whether the transport validated `Origin`/`Host` as same-origin.
    pub origin_ok: bool,
}

impl ApiRequest {
    /// Reads a query parameter (minimal parser, sufficient for `since_seq=N`).
    pub fn query_param(&self, key: &str) -> Option<&str> {
        self.query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == key).then_some(v)
        })
    }
}

/// An outgoing response that the transport converts into its own format.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: &'static str,
    /// `Set-Cookie` value (set only on login).
    pub set_cookie: Option<String>,
}

impl ApiResponse {
    pub fn json(status: u16, body: Vec<u8>) -> Self {
        ApiResponse {
            status,
            body,
            content_type: "application/json; charset=utf-8",
            set_cookie: None,
        }
    }
    pub fn empty(status: u16) -> Self {
        ApiResponse {
            status,
            body: Vec::new(),
            content_type: "application/json; charset=utf-8",
            set_cookie: None,
        }
    }
    /// Binary response (e.g. the `capture.bin` download). The browser fetches it via
    /// `fetch → blob`; the filename is set client-side (no Content-Disposition needed).
    pub fn binary(status: u16, body: Vec<u8>) -> Self {
        ApiResponse {
            status,
            body,
            content_type: "application/octet-stream",
            set_cookie: None,
        }
    }
}

pub mod deletions;

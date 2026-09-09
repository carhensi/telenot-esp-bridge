//! Pure request handlers. `dispatch` is the ONLY entry point: the transport layer (host
//! `tiny_http` or firmware `esp_http_server`) builds an [`ApiRequest`] and receives an
//! [`ApiResponse`]. No I/O, no hardware — everything goes through `App` + `Services` + `App::intents`.
//!
//! Routing, auth gates and shared helpers live here; the handlers are split by concern
//! into the submodules.

mod backup;
mod diagnostics;
mod mqtt;
mod ota;
mod security;
mod sensors;
mod session;

pub use backup::BACKUP_VERSION;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::App;
use crate::dto::*;
use crate::runtime::Intent;
use crate::{ApiRequest, ApiResponse, Method};

pub(super) fn err(status: u16, code: &str, message: &str) -> ApiResponse {
    let env = ErrorEnvelope {
        error: ErrorBody {
            code: code.into(),
            message: message.into(),
        },
    };
    match serde_json::to_vec(&env) {
        Ok(b) => ApiResponse::json(status, b),
        Err(_) => ApiResponse::json(status, b"{\"error\":{\"code\":\"internal\"}}".to_vec()),
    }
}

pub(super) fn ok_json<T: Serialize>(status: u16, val: &T) -> ApiResponse {
    match serde_json::to_vec(val) {
        Ok(b) => ApiResponse::json(status, b),
        Err(e) => err(500, "serialize", &e.to_string()),
    }
}

/// Like [`ok_json`] but with a pre-reserved buffer. `to_vec` grows by doubling — for the
/// full sensor list (~65 KB JSON) it holds old+new simultaneously (~96 KB contiguous),
/// which reliably caused OOM-aborts on the ESP32 heap. A slightly overestimated
/// single allocation halves the peak requirement.
pub(super) fn ok_json_cap<T: Serialize>(status: u16, val: &T, cap: usize) -> ApiResponse {
    let mut buf = Vec::with_capacity(cap);
    match serde_json::to_writer(&mut buf, val) {
        Ok(()) => ApiResponse::json(status, buf),
        Err(e) => err(500, "serialize", &e.to_string()),
    }
}

/// 429 with a stable code + `retry_after_s` (brute-force throttle, see REST-CONTRACT).
pub(super) fn rate_limited(retry_after_s: u64) -> ApiResponse {
    let body = serde_json::json!({
        "error": { "code": "rate_limited", "message": "Zu viele Versuche", "retry_after_s": retry_after_s }
    });
    ok_json(429, &body)
}

pub(super) fn parse_body<T: DeserializeOwned>(req: &ApiRequest) -> Result<T, ApiResponse> {
    serde_json::from_slice(&req.body)
        .map_err(|e| err(400, "bad_json", &format!("ungültiger Body: {e}")))
}

/// Parse an address from a path segment: `0x0042` (hex) or decimal.
fn parse_addr(seg: &str) -> Option<u16> {
    if let Some(h) = seg.strip_prefix("0x").or_else(|| seg.strip_prefix("0X")) {
        u16::from_str_radix(h, 16).ok()
    } else {
        seg.parse::<u16>().ok()
    }
}

/// Session/CSRF/origin gate as a standalone function: `dispatch` uses it for all routes;
/// the firmware OTA streaming route (httpd.rs) and the sim call it BEFORE reading the
/// (up to 4 MB) body. Error = ready-to-send HTTP response.
pub fn check_auth(
    app: &App,
    session_token: Option<&str>,
    csrf_token: Option<&str>,
    origin_ok: bool,
    mutating: bool,
) -> Result<(), ApiResponse> {
    // Constant-time: session/CSRF tokens are secrets, an early-exiting `==` would let an
    // attacker on the same LAN segment recover them byte-by-byte via response timing.
    let authed = match (app.session.as_deref(), session_token) {
        (Some(s), Some(t)) => crate::ct_eq(s.as_bytes(), t.as_bytes()),
        _ => false,
    };
    if !authed {
        return Err(err(401, "unauthorized", "Session erforderlich"));
    }
    if mutating {
        if !origin_ok {
            return Err(err(403, "bad_origin", "Origin abgelehnt"));
        }
        let csrf_ok = match (app.csrf.as_deref(), csrf_token) {
            (Some(c), Some(t)) => crate::ct_eq(c.as_bytes(), t.as_bytes()),
            _ => false,
        };
        if !csrf_ok {
            return Err(err(403, "csrf", "CSRF-Token fehlt/ungültig"));
        }
    }
    Ok(())
}

/// [`check_auth`] plus the first-boot password requirement, with NO route allowlist
/// (unlike [`dispatch`], which exempts password-change/logout). Used by the OTA streaming
/// route (firmware `httpd.rs` and the host sim), which never bypasses that requirement —
/// otherwise the (well-known) initial/default password could flash arbitrary firmware
/// before it's ever changed.
pub fn check_auth_and_setup_gate(
    app: &App,
    session_token: Option<&str>,
    csrf_token: Option<&str>,
    origin_ok: bool,
    mutating: bool,
) -> Result<(), ApiResponse> {
    check_auth(app, session_token, csrf_token, origin_ok, mutating)?;
    if app.services.password_change_required() {
        return Err(err(
            403,
            "password_change_required",
            "Erst ein neues Passwort vergeben",
        ));
    }
    Ok(())
}

/// Does the `Origin` header's authority (`scheme://host[:port]`, no path per the Fetch
/// spec) exactly match `Host`? Case-insensitive (host names aren't case-sensitive).
/// Deliberately NOT a substring check: `origin.contains(host)` would accept
/// `https://192.168.1.50.attacker.example` for `Host: 192.168.1.50`.
pub fn origin_matches_host(origin: &str, host: &str) -> bool {
    let authority = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    let authority = authority.split(['/', '?', '#']).next().unwrap_or(authority);
    authority.eq_ignore_ascii_case(host)
}

/// Entry point: routes the request. Public routes first, then the session+CSRF gate.
pub fn dispatch(app: &mut App, req: &ApiRequest) -> ApiResponse {
    let path = req.path.strip_prefix("/api/v1").unwrap_or(&req.path);
    let segs: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    // ── public (no login required) ──
    match (req.method, segs.as_slice()) {
        (Method::Post, ["session"]) => return session::handle_login(app, req),
        (Method::Get, ["device"]) => return ok_json(200, &app.device),
        _ => {}
    }

    // ── session/CSRF gate (shared with the OTA streaming route) ──
    if let Err(resp) = check_auth(
        app,
        req.session_token.as_deref(),
        req.csrf_token.as_deref(),
        req.origin_ok,
        req.method.is_mutating(),
    ) {
        return resp;
    }

    // ── First-boot: until a custom password is set, ONLY password change + logout are
    //    allowed (otherwise the initial password could be used indefinitely). ──
    if app.services.password_change_required() {
        let allowed = matches!(
            (req.method, segs.as_slice()),
            (Method::Post, ["session", "password"])
                | (Method::Delete, ["session"])
                | (Method::Get, ["session"])
        );
        if !allowed {
            return err(
                403,
                "password_change_required",
                "Erst ein neues Passwort vergeben",
            );
        }
    }

    match (req.method, segs.as_slice()) {
        (Method::Get, ["session"]) => session::handle_session_info(app),
        (Method::Delete, ["session"]) => {
            app.session = None;
            app.csrf = None;
            ApiResponse::empty(204)
        }
        (Method::Post, ["session", "password"]) => session::handle_set_password(app, req),
        // Extend the setup window by 30 min from the web interface (like device button BUT1).
        // Session- and CSRF-gated (POST). Capped firmware-side so the security window
        // cannot be held open indefinitely; the host sim ignores this intent.
        (Method::Post, ["session", "extend"]) => {
            app.intents.push(Intent::ExtendSetupWindow);
            ApiResponse::empty(204)
        }

        (Method::Get, ["connection"]) => sensors::handle_connection_get(app),
        (Method::Put, ["connection"]) => sensors::handle_connection_put(app, req),
        (Method::Post, ["connection", "check"]) => sensors::handle_connection_check_start(app),
        (Method::Get, ["connection", "check"]) => sensors::handle_connection_check_status(app),

        (Method::Post, ["scan", "start"]) => {
            app.intents.push(Intent::StartScan);
            ok_json(202, &serde_json::json!({"phase":"belegt"}))
        }
        (Method::Post, ["scan", "cancel"]) => sensors::handle_scan_cancel(app, req),
        (Method::Get, ["scan"]) => sensors::handle_scan_status(app),

        // HomeKit pairing info (setup code + QR payload) — only set in HomeKit mode.
        (Method::Get, ["homekit"]) => match &app.homekit_pair {
            Some(hp) => ok_json(200, hp),
            None => ApiResponse::empty(204),
        },
        // Apply pending HomeKit detector set LIVE: persist config (no reboot) and signal
        // the HAP thread to reconcile the accessory set. Session/CSRF-gated (POST).
        (Method::Post, ["homekit", "apply"]) => match app.working_config() {
            Ok(cfg) => {
                app.intents
                    .push(Intent::ReloadConfig(std::sync::Arc::new(cfg)));
                app.intents.push(Intent::ApplyHomekit);
                ApiResponse::empty(204)
            }
            Err(_) => err(
                500,
                "config_too_large",
                "Konfiguration zu groß für den Gerätespeicher",
            ),
        },
        // Reboot the device (picks up an MQTT↔HomeKit mode switch).
        (Method::Post, ["reboot"]) => {
            if app.pending_commit.is_some()
                || matches!(app.commit_status, crate::dto::CommitStatus::Failed { .. })
            {
                return err(
                    409,
                    "commit_not_saved",
                    "Konfiguration noch nicht erfolgreich gespeichert. Kein Neustart.",
                );
            }
            if app.setup.session.is_some() {
                return err(409, "unsaved_sensors", "Melderauswahl noch nicht übernommen. Bitte im letzten Setup-Schritt speichern.");
            }
            app.intents.push(Intent::Reboot);
            ApiResponse::empty(204)
        }

        (Method::Get, ["sensors"]) => sensors::handle_sensors_get(app, req),
        (Method::Post, ["sensors", "bulk"]) => sensors::handle_bulk(app, req),
        (Method::Patch, ["sensors", a]) => match parse_addr(a) {
            Some(addr) => sensors::handle_patch(app, req, addr),
            None => err(400, "bad_address", "ungültige Adresse"),
        },
        (Method::Post, ["sensors", a, "observe-polarity"]) => match parse_addr(a) {
            Some(addr) => {
                app.intents.push(Intent::ObservePolarity { address: addr });
                ApiResponse::empty(202)
            }
            None => err(400, "bad_address", "ungültige Adresse"),
        },

        (Method::Get, ["mqtt"]) => mqtt::handle_get(app),
        (Method::Put, ["mqtt"]) => mqtt::handle_put(app, req),
        (Method::Post, ["mqtt", "test"]) => mqtt::handle_test_start(app),
        (Method::Get, ["mqtt", "test"]) => mqtt::handle_test_status(app),
        (Method::Post, ["mqtt", "pin"]) => mqtt::handle_pin(app),
        (Method::Delete, ["mqtt", "pin"]) => mqtt::handle_unpin(app),

        (Method::Get, ["security"]) => security::handle_get(app),
        (Method::Put, ["security", "pin"]) => security::handle_put_pin(app, req),
        (Method::Put, ["security", "remote-disarm"]) => {
            security::handle_put_remote_disarm(app, req)
        }

        (Method::Get, ["diagnostics"]) => diagnostics::handle_diagnostics(app),
        (Method::Get, ["diagnostics", "log"]) => diagnostics::handle_log(app, req),
        (Method::Post, ["debug", "capture", "start"]) => {
            diagnostics::handle_capture_start(app, req)
        }
        (Method::Post, ["debug", "capture", "stop"]) => {
            app.intents.push(Intent::CaptureStop);
            ApiResponse::empty(202)
        }
        (Method::Get, ["debug", "capture"]) => diagnostics::handle_capture_status(app),
        (Method::Get, ["debug", "capture.bin"]) => ApiResponse::binary(200, app.capture.bytes()),

        (Method::Get, ["state"]) => diagnostics::handle_state(app),

        (Method::Get, ["review"]) => sensors::handle_review(app),
        (Method::Post, ["commit"]) => sensors::handle_commit(app, req),
        (Method::Get, ["commit"]) => ok_json(200, &app.commit_status),

        // Settings backup: export/import as JSON (migration safety net, e.g. before
        // repartitioning for OTA). Intentionally excludes secrets.
        (Method::Get, ["backup"]) => backup::handle_export(app),
        (Method::Post, ["backup"]) => backup::handle_import(app, req),

        // OTA status. The upload does NOT go through dispatch — it streams directly into
        // the flash partition (telenot-esp-bridge/src/httpd.rs or sim mock) because 2 MB
        // images don't fit in the buffered request path (256 KB limit).
        (Method::Get, ["ota"]) => ota::handle_status(app),
        (Method::Put, ["ota", "settings"]) => ota::handle_settings_put(app, req),

        // Live test board: arm/disarm/bypass. PIN gate + execution in the daemon
        // (security-reducing commands are fail-closed).
        (Method::Post, ["command"]) => match parse_body::<CommandReq>(req) {
            Ok(c) => match crate::command::parse_bridge_command(&c, app.persisted.panel.kind) {
                Ok(cmd) => {
                    app.intents.push(Intent::Command { cmd, pin: c.pin });
                    ApiResponse::empty(202)
                }
                Err(reason) => err(400, "bad_command", reason),
            },
            Err(e) => e,
        },

        _ => err(404, "not_found", "unbekannte Route"),
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    use crate::security::InMemoryServices;

    fn app_with(session: Option<&str>, csrf: Option<&str>) -> App {
        let mut app = App::new(
            crate::DeviceInfo::default(),
            Box::new(InMemoryServices::new("sticker-pw")),
        );
        app.session = session.map(String::from);
        app.csrf = csrf.map(String::from);
        app
    }

    #[test]
    fn origin_matches_host_exact() {
        assert!(origin_matches_host("http://192.168.1.50", "192.168.1.50"));
        assert!(origin_matches_host(
            "https://Device.Local:8080",
            "device.local:8080"
        ));
    }

    #[test]
    fn origin_matches_host_rejects_subdomain_attack() {
        // A naive `origin.contains(host)` would wrongly accept this.
        assert!(!origin_matches_host(
            "http://192.168.1.50.attacker.example",
            "192.168.1.50"
        ));
        assert!(!origin_matches_host("http://evil.com", "192.168.1.50"));
    }

    #[test]
    fn origin_matches_host_rejects_port_mismatch() {
        assert!(!origin_matches_host(
            "http://192.168.1.50:81",
            "192.168.1.50:80"
        ));
    }

    #[test]
    fn check_auth_rejects_wrong_session_and_csrf() {
        let app = app_with(Some("sess-abc"), Some("csrf-xyz"));
        assert!(check_auth(&app, Some("wrong"), Some("csrf-xyz"), true, true).is_err());
        assert!(check_auth(&app, Some("sess-abc"), Some("wrong"), true, true).is_err());
        assert!(check_auth(&app, Some("sess-abc"), Some("csrf-xyz"), true, true).is_ok());
    }

    #[test]
    fn setup_gate_denies_ota_before_password_change() {
        let app = app_with(Some("sess-abc"), Some("csrf-xyz"));
        assert!(app.services.password_change_required());
        let resp = check_auth_and_setup_gate(&app, Some("sess-abc"), Some("csrf-xyz"), true, true)
            .unwrap_err();
        assert_eq!(resp.status, 403);
    }

    #[test]
    fn setup_gate_allows_after_password_change() {
        let mut app = app_with(Some("sess-abc"), Some("csrf-xyz"));
        app.services.set_password("neues-geheim-1").unwrap();
        assert!(!app.services.password_change_required());
        assert!(
            check_auth_and_setup_gate(&app, Some("sess-abc"), Some("csrf-xyz"), true, true).is_ok()
        );
    }
}

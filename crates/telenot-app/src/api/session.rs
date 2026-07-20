//! Login, session info and password change.

use super::{err, ok_json, parse_body, rate_limited};
use crate::app::App;
use crate::dto::*;
use crate::{ApiRequest, ApiResponse};

/// Setup session cookie (loopback dev without `Secure`; the TLS transport adds `Secure`).
fn session_cookie(token: &str) -> String {
    format!("session={token}; HttpOnly; SameSite=Strict; Path=/api")
}

pub(super) fn handle_login(app: &mut App, req: &ApiRequest) -> ApiResponse {
    let body: LoginReq = match parse_body(req) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Some(retry_after_s) = app.services.login_locked() {
        return rate_limited(retry_after_s);
    }
    if !app.services.verify_login(&body.password) {
        app.services.note_login_fail();
        return err(401, "invalid_password", "Passwort falsch");
    }
    app.services.note_login_ok();
    let token = app.services.new_token();
    let csrf = app.services.new_token();
    app.session = Some(token.clone());
    app.csrf = Some(csrf.clone());
    let resp = LoginResp {
        csrf_token: csrf,
        password_change_required: app.services.password_change_required(),
        device: app.device.clone(),
    };
    let mut r = ok_json(200, &resp);
    r.set_cookie = Some(session_cookie(&token));
    r
}

/// Session info for an existing cookie: returns a FRESH CSRF token so the frontend
/// can continue after a reload/new tab without re-login (the session survives
/// server-side; only the client's in-memory CSRF token is gone).
pub(super) fn handle_session_info(app: &mut App) -> ApiResponse {
    let csrf = app.services.new_token();
    app.csrf = Some(csrf.clone());
    ok_json(
        200,
        &LoginResp {
            csrf_token: csrf,
            password_change_required: app.services.password_change_required(),
            device: app.device.clone(),
        },
    )
}

pub(super) fn handle_set_password(app: &mut App, req: &ApiRequest) -> ApiResponse {
    let body: PasswordReq = match parse_body(req) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if body.new_password.len() < 8 {
        return err(400, "password_too_short", "Mindestens 8 Zeichen");
    }
    // Rotating an existing password requires the current one (re-authentication).
    // This is skipped on the first-boot mandatory change: the login already proved the initial password.
    if !app.services.password_change_required() {
        let current = body.current_password.as_deref().unwrap_or("");
        if !app.services.verify_login(current) {
            return err(403, "current_password_invalid", "Aktuelles Passwort falsch");
        }
    }
    app.services.set_password(&body.new_password);
    ApiResponse::empty(204)
}

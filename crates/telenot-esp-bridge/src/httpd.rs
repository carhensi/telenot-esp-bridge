//! Slice 5: HTTP server (plain) + setup web.
//!
//! Mirrors the host glue (`telenot-sim` `handle_http`): builds an [`ApiRequest`] from each
//! request, calls the pure [`dispatch`] function, and writes back the [`ApiResponse`].
//! Everything outside `/api` serves the embedded Preact bundle (web/dist).
//!
//! HARDENING LATER: HTTPS (self-signed + SAN + fingerprint) and "active only in setup window"
//! (slice 7). For now permanently on port 80.

use std::sync::{Arc, Mutex};

use esp_idf_svc::http::server::{Configuration, EspHttpConnection, EspHttpServer, Request};
use esp_idf_svc::http::Method as EspMethod;
use esp_idf_svc::io::{EspIOError, Write};
use esp_idf_svc::sys::EspError;
use telenot_app::api::{check_auth, dispatch};
use telenot_app::ota::{len_ok, OtaState};
use telenot_app::{ApiRequest, App, Method, OtaPhase};

/// Embedded single-file bundle (vite + vite-plugin-singlefile). Rebuild before flashing if
/// needed: `web/scripts/assemble.sh && npm --prefix web run build`.
const INDEX_HTML: &[u8] = include_bytes!("../../../web/dist/index.html");

/// Upper limit for request bodies (config commit is the largest case).
const MAX_BODY: usize = 256 * 1024;

/// Starts the HTTP server. The returned handle must be kept alive (drop = stop).
pub fn start(app: Arc<Mutex<App>>, port: u16) -> Result<EspHttpServer<'static>, EspError> {
    let conf = Configuration {
        http_port: port,
        stack_size: 10240, // dispatch + serde_json need some stack
        uri_match_wildcard: true,
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&conf).map_err(|e| e.0)?;

    // OTA upload MUST be registered before the wildcard handlers (esp_httpd matches in
    // registration order). Streams the image directly into flash — the buffered
    // 256 KB path below is unaffected.
    {
        let app = app.clone();
        server.fn_handler("/api/v1/ota/upload", EspMethod::Post, move |req| {
            serve_ota_upload(&app, req)
        })?;
    }

    // One wildcard handler per method — path logic lives in `serve`.
    for method in [
        EspMethod::Get,
        EspMethod::Post,
        EspMethod::Put,
        EspMethod::Patch,
        EspMethod::Delete,
    ] {
        let app = app.clone();
        server.fn_handler("/*", method, move |req| serve(&app, req))?;
    }
    log::info!("HTTP-Server läuft auf Port {port}");
    Ok(server)
}

fn serve(
    app: &Arc<Mutex<App>>,
    mut req: Request<&mut EspHttpConnection>,
) -> Result<(), EspIOError> {
    let Some(method) = map_method(req.method()) else {
        req.into_status_response(405)?;
        return Ok(());
    };

    let url = req.uri().to_string();
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (url, String::new()),
    };

    // Static asset (everything outside /api) → web bundle.
    if !path.starts_with("/api") {
        if method == Method::Get {
            let headers = [("Content-Type", "text/html; charset=utf-8")];
            let mut resp = req.into_response(200, Some("OK"), &headers)?;
            for chunk in INDEX_HTML.chunks(4096) {
                resp.write_all(chunk)?;
            }
        } else {
            req.into_status_response(405)?;
        }
        return Ok(());
    }

    // Copy headers before reading the body (afterwards &mut req belongs to the reader).
    let session = req
        .header("Cookie")
        .and_then(|c| cookie_value(c, "session"));
    let csrf = req.header("X-CSRF-Token").map(str::to_string);
    let origin = req.header("Origin").map(str::to_string);
    let host = req.header("Host").map(str::to_string);
    let origin_ok = match (&origin, &host) {
        (None, _) => true,
        (Some(o), Some(h)) => o.contains(h.as_str()),
        (Some(_), None) => false,
    };

    // Read body (chunked until EOF/Content-Length).
    let mut body = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = req.read(&mut buf)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        if body.len() > MAX_BODY {
            req.into_status_response(413)?;
            return Ok(());
        }
    }

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

    let cookie = api_resp.set_cookie;
    let mut headers: Vec<(&str, &str)> = vec![("Content-Type", api_resp.content_type)];
    if let Some(c) = &cookie {
        headers.push(("Set-Cookie", c.as_str()));
    }
    let mut resp = req.into_response(api_resp.status, None, &headers)?;
    resp.write_all(&api_resp.body)?;
    Ok(())
}

/// OTA upload: authenticate BEFORE reading the body, then stream chunks directly into the
/// inactive slot. Progress is mirrored into the app only every ~64 KB (reduces lock churn).
fn serve_ota_upload(
    app: &Arc<Mutex<App>>,
    mut req: Request<&mut EspHttpConnection>,
) -> Result<(), EspIOError> {
    // Copy headers BEFORE reading the body (afterwards &mut req belongs to the reader).
    let session = req
        .header("Cookie")
        .and_then(|c| cookie_value(c, "session"));
    let csrf = req.header("X-CSRF-Token").map(str::to_string);
    let origin = req.header("Origin").map(str::to_string);
    let host = req.header("Host").map(str::to_string);
    let origin_ok = match (&origin, &host) {
        (None, _) => true,
        (Some(o), Some(h)) => o.contains(h.as_str()),
        (Some(_), None) => false,
    };
    let total: Option<usize> = req
        .header("Content-Length")
        .and_then(|v| v.trim().parse().ok());
    let expected_sha = req.header("X-Expected-Sha256").map(str::to_string);

    // Auth and state gate under a short lock — rejected BEFORE any bytes flow.
    {
        let mut a = app.lock().unwrap();
        if let Err(resp) = check_auth(&a, session.as_deref(), csrf.as_deref(), origin_ok, true) {
            return write_api_response(req, resp);
        }
        if a.ota.phase == OtaPhase::Receiving {
            return write_error(req, 409, "busy", "OTA-Upload läuft bereits");
        }
        if a.capture.active {
            // Capture holds the ring buffer + frame statistics — pumping 4 KB chunks
            // concurrently risks heap pressure. Stop capture first, then update.
            return write_error(req, 409, "capture_active", "Erst GMS-Mitschnitt stoppen");
        }
        let Some(total) = total else {
            return write_error(req, 411, "length_required", "Content-Length fehlt");
        };
        let Some(sha) = expected_sha
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return write_error(req, 400, "sha_required", "X-Expected-Sha256 fehlt");
        };
        let _ = sha;
        if let Err(code) = len_ok(total) {
            a.ota = OtaState::failed(code);
            return write_error(req, 400, code, "Image-Größe unplausibel");
        }
        a.ota = OtaState::receiving(total);
        let t = crate::storage::now_ms();
        a.ring.push(
            t,
            "info",
            format!("OTA-Upload gestartet ({} KB)", total / 1024),
        );
    }
    let total = total.unwrap_or(0);
    let expected_sha = expected_sha.unwrap_or_default();

    let progress_app = app.clone();
    let mut last_mirrored = 0usize;
    let result = crate::ota::stream_update(
        total,
        &expected_sha,
        |buf| req.read(buf).map_err(|_| ()),
        |received| {
            if received - last_mirrored >= 64 * 1024 || received == total {
                last_mirrored = received;
                if let Ok(mut a) = progress_app.lock() {
                    a.ota.received = received;
                }
            }
        },
    );

    match result {
        Ok(version) => {
            let mut a = app.lock().unwrap();
            a.ota.phase = OtaPhase::ReadyToReboot;
            a.ota.received = total;
            a.ota.new_version = Some(version.clone());
            let t = crate::storage::now_ms();
            a.ring.push(
                t,
                "info",
                format!("OTA-Image v{version} geschrieben — Neustart übernimmt es"),
            );
            drop(a);
            write_json(
                req,
                200,
                &format!("{{\"state\":\"ready_to_reboot\",\"version\":\"{version}\"}}"),
            )
        }
        Err(code) => {
            let mut a = app.lock().unwrap();
            a.ota = OtaState::failed(code);
            let t = crate::storage::now_ms();
            a.ring
                .push(t, "warn", format!("OTA-Upload fehlgeschlagen: {code}"));
            drop(a);
            let status = match code {
                "busy" => 409,
                "write_failed" | "read_failed" => 500,
                _ => 400,
            };
            write_error(req, status, code, "OTA-Upload fehlgeschlagen")
        }
    }
}

fn write_api_response(
    req: Request<&mut EspHttpConnection>,
    api: telenot_app::ApiResponse,
) -> Result<(), EspIOError> {
    let headers = [("Content-Type", api.content_type)];
    let mut resp = req.into_response(api.status, None, &headers)?;
    resp.write_all(&api.body)?;
    Ok(())
}

fn write_json(
    req: Request<&mut EspHttpConnection>,
    status: u16,
    body: &str,
) -> Result<(), EspIOError> {
    let headers = [("Content-Type", "application/json; charset=utf-8")];
    let mut resp = req.into_response(status, None, &headers)?;
    resp.write_all(body.as_bytes())?;
    Ok(())
}

/// Error in the same envelope format as `telenot_app::api` (`{"error":{code,message}}`).
fn write_error(
    req: Request<&mut EspHttpConnection>,
    status: u16,
    code: &str,
    message: &str,
) -> Result<(), EspIOError> {
    write_json(
        req,
        status,
        &format!("{{\"error\":{{\"code\":\"{code}\",\"message\":\"{message}\"}}}}"),
    )
}

fn map_method(m: EspMethod) -> Option<Method> {
    Some(match m {
        EspMethod::Get => Method::Get,
        EspMethod::Post => Method::Post,
        EspMethod::Put => Method::Put,
        EspMethod::Patch => Method::Patch,
        EspMethod::Delete => Method::Delete,
        _ => return None,
    })
}

fn cookie_value(cookie_header: &str, key: &str) -> Option<String> {
    cookie_header.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

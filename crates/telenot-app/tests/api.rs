//! Golden/contract tests: drive the full wizard happy-path through `api::dispatch` and
//! verify wire shape (snake_case, u16 addresses, no secrets) + session/CSRF gates.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use telenot_app::api::dispatch;
use telenot_app::{ApiRequest, App, DeviceInfo, InMemoryServices, Intent, Method};
use telenot_config::{Config, Polarity, Sensor, SensorKind, CURRENT_SCHEMA_VERSION};

fn app() -> App {
    App::new(
        DeviceInfo::default(),
        Box::new(InMemoryServices::new("dev-pw")),
    )
}

fn req(
    method: Method,
    path: &str,
    body: Value,
    session: Option<&str>,
    csrf: Option<&str>,
) -> ApiRequest {
    ApiRequest {
        method,
        path: path.into(),
        query: String::new(),
        body: if body.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(&body).unwrap()
        },
        session_token: session.map(Into::into),
        csrf_token: csrf.map(Into::into),
        origin_ok: true,
    }
}

fn jval(r: &telenot_app::ApiResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

/// Logs in and returns (session_token, csrf_token).
fn login(app: &mut App) -> (String, String) {
    let r = dispatch(
        app,
        &req(
            Method::Post,
            "/api/v1/session",
            json!({"password":"dev-pw"}),
            None,
            None,
        ),
    );
    assert_eq!(r.status, 200, "Login ok");
    let cookie = r.set_cookie.clone().expect("Set-Cookie");
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let token = cookie
        .strip_prefix("session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let csrf = jval(&r)["csrf_token"].as_str().unwrap().to_string();
    assert_eq!(
        jval(&r)["password_change_required"],
        true,
        "First-Boot meldet Pflichtwechsel"
    );
    // First-boot mandatory change: set a personal password before any other action.
    let r = dispatch(
        app,
        &req(
            Method::Post,
            "/api/v1/session/password",
            json!({"new_password":"neues-geheim"}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 204, "First-Boot-Passwortwechsel");
    (token, csrf)
}

fn seed(app: &mut App) {
    let s = |address, name: &str, kind, confirmed| Sensor {
        address,
        name: name.into(),
        name_ha: name.into(),
        kind,
        topic: "t".into(),
        location: "Erdgeschoss".into(),
        polarity: Polarity::ActiveLow,
        confirmed,
        switchable: false,
        show_in_homekit: false,
    };
    let cfg = Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        sensors: telenot_config::SensorTable::from_sensors(&[
            s(0x0042, "MK Haustür", SensorKind::Magnetkontakt, false),
            s(0x0050, "BM Flur", SensorKind::Bewegungsmelder, true),
        ])
        .unwrap(),
        panel: Default::default(),
    };
    let mut raw = BTreeMap::new();
    raw.insert(0x0042u16, "MK Haust\u{fc}r".to_string());
    app.setup.seed(cfg, raw);
}

#[test]
fn login_gate_and_wrong_password() {
    let mut app = app();
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session",
            json!({"password":"falsch"}),
            None,
            None,
        ),
    );
    assert_eq!(r.status, 401);
    assert_eq!(jval(&r)["error"]["code"], "invalid_password");

    // Without a session /sensors is locked …
    let r = dispatch(
        &mut app,
        &req(Method::Get, "/api/v1/sensors", Value::Null, None, None),
    );
    assert_eq!(r.status, 401);
    // … but /device is readable before login (S0 shows the fingerprint).
    let r = dispatch(
        &mut app,
        &req(Method::Get, "/api/v1/device", Value::Null, None, None),
    );
    assert_eq!(r.status, 200);
}

#[test]
fn first_boot_blocks_until_password_set() {
    let mut app = app();
    // Log in directly (without the password-change helper).
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session",
            json!({"password":"dev-pw"}),
            None,
            None,
        ),
    );
    let token = r
        .set_cookie
        .as_ref()
        .unwrap()
        .strip_prefix("session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let csrf = jval(&r)["csrf_token"].as_str().unwrap().to_string();

    // Before the password change, EVERY other route is locked (including reads).
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/sensors",
            Value::Null,
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 403);
    assert_eq!(jval(&r)["error"]["code"], "password_change_required");

    // After the change it works.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session/password",
            json!({"new_password":"neues-geheim"}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 204);
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/sensors",
            Value::Null,
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 200);
}

#[test]
fn login_lockout_after_repeated_failures() {
    let mut app = app();
    for _ in 0..5 {
        let r = dispatch(
            &mut app,
            &req(
                Method::Post,
                "/api/v1/session",
                json!({"password":"falsch"}),
                None,
                None,
            ),
        );
        assert_eq!(r.status, 401);
    }
    // Now locked out — even the CORRECT password is rejected with 429 + retry_after_s.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session",
            json!({"password":"dev-pw"}),
            None,
            None,
        ),
    );
    assert_eq!(r.status, 429);
    assert_eq!(jval(&r)["error"]["code"], "rate_limited");
    assert!(jval(&r)["error"]["retry_after_s"].as_u64().unwrap() > 0);
}

#[test]
fn password_rotation_requires_current_password() {
    let mut app = app();
    let (token, csrf) = login(&mut app); // setzt bereits "neues-geheim"

    // Rotation WITHOUT current password → 403.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session/password",
            json!({"new_password":"noch-geheimer"}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 403);
    assert_eq!(jval(&r)["error"]["code"], "current_password_invalid");

    // Rotation WITH correct current password → 204.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session/password",
            json!({"new_password":"noch-geheimer","current_password":"neues-geheim"}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 204);
}

#[test]
fn sensors_wire_shape_and_no_secrets() {
    let mut app = app();
    let (token, csrf) = login(&mut app);
    seed(&mut app);

    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/sensors",
            Value::Null,
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 200);
    let v = jval(&r);
    let s0 = &v["sensors"][0];
    assert_eq!(s0["address"], 0x0042, "Adresse als u16-Zahl");
    assert_eq!(s0["status"], "unconfirmed");
    assert_eq!(s0["raw_name"], "MK Haustür");
    assert_eq!(s0["polarity"], "active_low", "snake_case enum");
    assert_eq!(v["counts"]["confirmed"], 1);
    assert_eq!(v["counts"]["unconfirmed"], 1);

    // Never expose secrets in GET /mqtt.
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/mqtt",
            Value::Null,
            Some(&token),
            Some(&csrf),
        ),
    );
    let body = String::from_utf8_lossy(&r.body);
    assert!(body.contains("password_set"));
    assert!(
        !body.contains("\"password\""),
        "kein password-Feld in der Response"
    );
}

#[test]
fn csrf_required_for_mutations() {
    let mut app = app();
    let (token, csrf) = login(&mut app);
    seed(&mut app);

    // PATCH without CSRF token → 403
    let r = dispatch(
        &mut app,
        &req(
            Method::Patch,
            "/api/v1/sensors/0x0042",
            json!({"confirmed":true}),
            Some(&token),
            None,
        ),
    );
    assert_eq!(r.status, 403);

    // PATCH with CSRF → 200, confirmed
    let r = dispatch(
        &mut app,
        &req(
            Method::Patch,
            "/api/v1/sensors/0x0042",
            json!({"confirmed":true}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 200);
    assert_eq!(jval(&r)["status"], "confirmed");

    // Cross-origin (origin_ok=false) → 403
    let mut bad = req(
        Method::Patch,
        "/api/v1/sensors/0x0042",
        json!({"confirmed":true}),
        Some(&token),
        Some(&csrf),
    );
    bad.origin_ok = false;
    assert_eq!(dispatch(&mut app, &bad).status, 403);
}

#[test]
fn security_and_commit_flow() {
    let mut app = app();
    let (token, csrf) = login(&mut app);
    seed(&mut app);

    // Weak PIN is accepted but flagged.
    let r = dispatch(
        &mut app,
        &req(
            Method::Put,
            "/api/v1/security/pin",
            json!({"pin":"1234"}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 200);
    assert_eq!(jval(&r)["weak"], true);

    // Remote-disarm without ack → fail-closed (409 ack_required).
    let r = dispatch(
        &mut app,
        &req(
            Method::Put,
            "/api/v1/security/remote-disarm",
            json!({"enabled":true}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 409);
    // With ack → 204
    let r = dispatch(
        &mut app,
        &req(
            Method::Put,
            "/api/v1/security/remote-disarm",
            json!({"enabled":true,"acknowledged":true}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 204);

    // Review lists warnings (unconfirmed sensor + mqtt never tested + remote active).
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/review",
            Value::Null,
            Some(&token),
            Some(&csrf),
        ),
    );
    let codes: Vec<String> = jval(&r)["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap().to_string())
        .collect();
    assert!(codes.contains(&"sensor_unconfirmed".to_string()));
    assert!(codes.contains(&"remote_disarm_active".to_string()));
    assert!(codes.contains(&"mqtt_never_tested".to_string()));

    // Commit without ack → 409, with ack → 200 + ReloadConfig intent.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/commit",
            json!({}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 409);
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/commit",
            json!({"warnings_acknowledged":true}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 200);
    assert_eq!(jval(&r)["rebooting"], true);
    assert!(app
        .intents
        .iter()
        .any(|i| matches!(i, Intent::ReloadConfig(_))));
}

// ═══════════ Recent features: pagination, feed tail, command_result ═══════════

/// Seeds `n` confirmed detectors starting at 0x0100 into the setup state (pagination tests).
fn seed_many(app: &mut App, n: u16) {
    let sensors = (0..n)
        .map(|i| Sensor {
            address: 0x0100 + i,
            name: format!("Melder {i}"),
            name_ha: format!("Melder {i}"),
            kind: SensorKind::Magnetkontakt,
            topic: format!("melder_{i}"),
            location: "Raum".into(),
            polarity: Polarity::ActiveLow,
            confirmed: true,
            switchable: false,
            show_in_homekit: false,
        })
        .collect::<Vec<Sensor>>();
    app.setup.seed(
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: telenot_config::SensorTable::from_sensors(&sensors).unwrap(),
            panel: Default::default(),
        },
        BTreeMap::new(),
    );
}

fn get_sensors(app: &mut App, token: &str, query: &str) -> Value {
    let mut r = req(
        Method::Get,
        "/api/v1/sensors",
        Value::Null,
        Some(token),
        None,
    );
    r.query = query.into();
    let resp = dispatch(app, &r);
    assert_eq!(resp.status, 200, "GET /sensors ({query})");
    jval(&resp)
}

#[test]
fn sensors_pagination_limits_pages_and_stays_complete() {
    // The ESP32 must NEVER serialize the full list (OOM proven in the field) —
    // the server limit must also protect forgetful clients.
    let mut app = app();
    let (token, _csrf) = login(&mut app);
    seed_many(&mut app, 150);

    // Default: max 60, total/offset in wire format.
    let v = get_sensors(&mut app, &token, "");
    assert_eq!(v["total"], 150);
    assert_eq!(v["offset"], 0);
    assert_eq!(v["sensors"].as_array().unwrap().len(), 60);

    // Clamping: limit>60 → 60; limit=0 → 1.
    let v = get_sensors(&mut app, &token, "offset=0&limit=500");
    assert_eq!(
        v["sensors"].as_array().unwrap().len(),
        60,
        "Server-Limit hält"
    );
    let v = get_sensors(&mut app, &token, "offset=0&limit=0");
    assert_eq!(
        v["sensors"].as_array().unwrap().len(),
        1,
        "limit clampt auf ≥1"
    );

    // Offset past the end: empty page, total correct, offset clamped to total.
    let v = get_sensors(&mut app, &token, "offset=1000&limit=60");
    assert_eq!(v["sensors"].as_array().unwrap().len(), 0);
    assert_eq!(v["total"], 150);
    assert_eq!(
        v["offset"], 150,
        "offset auf total geclampt (kein Phantomwert)"
    );

    // Broken parameters → defaults instead of panic.
    let v = get_sensors(&mut app, &token, "offset=abc&limit=-1");
    assert_eq!(v["offset"], 0);
    assert_eq!(v["sensors"].as_array().unwrap().len(), 60);

    // Pages are disjoint and together complete.
    let mut seen = std::collections::BTreeSet::new();
    for off in [0usize, 60, 120] {
        let v = get_sensors(&mut app, &token, &format!("offset={off}&limit=60"));
        for s in v["sensors"].as_array().unwrap() {
            assert!(
                seen.insert(s["address"].as_u64().unwrap()),
                "address appears on multiple pages"
            );
        }
    }
    assert_eq!(seen.len(), 150, "all sensors reachable across 3 pages");
}

#[test]
fn scan_feed_delivers_only_the_tail() {
    // The scan feed once returned the FULL list (~55 KB JSON → 64-KB alloc → OOM).
    let mut app = app();
    let (token, _csrf) = login(&mut app);

    let sensors: Vec<Sensor> = (0..20u16)
        .map(|i| Sensor {
            address: 0x0200 + i,
            name: format!("Scan {i}"),
            name_ha: format!("Scan {i}"),
            kind: SensorKind::Magnetkontakt,
            topic: format!("scan_{i}"),
            location: String::new(),
            polarity: Polarity::ActiveLow,
            confirmed: false,
            switchable: false,
            show_in_homekit: false,
        })
        .collect();
    app.live.discovered = Some(std::sync::Arc::new((
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: telenot_config::SensorTable::from_sensors(&sensors).unwrap(),
            panel: Default::default(),
        },
        BTreeMap::new(),
    )));

    let r = dispatch(
        &mut app,
        &req(Method::Get, "/api/v1/scan", Value::Null, Some(&token), None),
    );
    assert_eq!(r.status, 200);
    let feed = jval(&r)["feed"].as_array().unwrap().clone();
    assert_eq!(feed.len(), 15, "nur der Tail, nie die volle Liste");
    assert_eq!(
        feed[0]["address"].as_u64().unwrap(),
        0x0205,
        "es sind die LETZTEN 15 (Adressen 0x0205..0x0214)"
    );
    assert_eq!(feed[14]["address"].as_u64().unwrap(), 0x0213);
}

#[test]
fn state_reports_last_command_result() {
    let mut app = app();
    let (token, _csrf) = login(&mut app);

    // No result yet: field is absent (skip_serializing_if).
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/state",
            Value::Null,
            Some(&token),
            None,
        ),
    );
    assert!(jval(&r).get("command_result").is_none());

    app.live.command_result = Some(("OK ArmHome".into(), 1234));
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/state",
            Value::Null,
            Some(&token),
            None,
        ),
    );
    let cr = &jval(&r)["command_result"];
    assert_eq!(cr["text"], "OK ArmHome");
    assert_eq!(cr["ms_ago"], 1234);
}

// ═══════════ Security negatives (gaps from the test inventory) ═══════════

#[test]
fn switchable_allowlist_is_fail_closed() {
    // Switch enable: ONLY real switch outputs — a hand-edited config must never
    // let the output path target inputs or the system-status block.
    let mut app = app();
    let (token, csrf) = login(&mut app);
    let s = |address| Sensor {
        address,
        name: "S".into(),
        name_ha: "S".into(),
        kind: SensorKind::Signalgeber,
        topic: format!("s_{address}"),
        location: String::new(),
        polarity: Polarity::ActiveLow,
        confirmed: true,
        switchable: false,
        show_in_homekit: false,
    };
    app.setup.seed(
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: telenot_config::SensorTable::from_sensors(&[s(0x0042), s(0x0515), s(0x0530)])
                .unwrap(),
            panel: Default::default(),
        },
        BTreeMap::new(),
    );

    // Input: stays false (fail-closed), response confirms it.
    let r = dispatch(
        &mut app,
        &req(
            Method::Patch,
            "/api/v1/sensors/0x0042",
            json!({"switchable": true}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 200);
    assert_eq!(jval(&r)["switchable"], false, "Eingang nie schaltbar");

    // System-status block (arm/bypass addresses): also off-limits.
    let r = dispatch(
        &mut app,
        &req(
            Method::Patch,
            "/api/v1/sensors/0x0530",
            json!({"switchable": true}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(jval(&r)["switchable"], false, "Status-Block nie schaltbar");

    // Real switch output: enable takes effect.
    let r = dispatch(
        &mut app,
        &req(
            Method::Patch,
            "/api/v1/sensors/0x0515",
            json!({"switchable": true}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(jval(&r)["switchable"], true, "Schaltausgang freigebbar");
}

#[test]
fn pin_format_is_validated() {
    let mut app = app();
    let (token, csrf) = login(&mut app);
    for bad in ["12", "abcd", ""] {
        let r = dispatch(
            &mut app,
            &req(
                Method::Put,
                "/api/v1/security/pin",
                json!({"pin": bad}),
                Some(&token),
                Some(&csrf),
            ),
        );
        assert_eq!(r.status, 400, "PIN '{bad}' muss abgelehnt werden");
        assert_eq!(jval(&r)["error"]["code"], "pin_invalid");
    }
}

#[test]
fn logout_invalidates_session() {
    let mut app = app();
    let (token, csrf) = login(&mut app);
    let r = dispatch(
        &mut app,
        &req(
            Method::Delete,
            "/api/v1/session",
            Value::Null,
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 204);
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/sensors",
            Value::Null,
            Some(&token),
            None,
        ),
    );
    assert_eq!(r.status, 401, "alter Token ist nach Logout wertlos");
}

#[test]
fn bulk_ops_update_known_and_skip_unknown() {
    let mut app = app();
    let (token, csrf) = login(&mut app);
    seed(&mut app); // 0x0042 (unconfirmed) + 0x0050 (confirmed)

    // Unknown address in the set is silently skipped — updated counts only matches.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/sensors/bulk",
            json!({"addresses": [0x0042, 0x0050, 0x0999], "op": "confirm"}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 200);
    assert_eq!(jval(&r)["updated"], 2, "only known addresses count");
    let v = get_sensors(&mut app, &token, "");
    assert!(v["sensors"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["confirmed"] == true));

    // Exclude/include round-trip via status field.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/sensors/bulk",
            json!({"addresses": [0x0042], "op": "exclude"}),
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(jval(&r)["updated"], 1);
    let v = get_sensors(&mut app, &token, "");
    let s42 = v["sensors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["address"] == 0x0042)
        .unwrap()
        .clone();
    assert_eq!(s42["status"], "excluded");
    assert_eq!(s42["include"], false);
}

#[test]
fn setup_window_extend_is_gated_and_queues_intent() {
    let mut app = app();
    // Without a session: locked (401).
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session/extend",
            Value::Null,
            None,
            None,
        ),
    );
    assert_eq!(r.status, 401);

    let (token, csrf) = login(&mut app);
    app.intents.clear();

    // Session but no CSRF → 403 (mutating method), no intent queued.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session/extend",
            Value::Null,
            Some(&token),
            None,
        ),
    );
    assert_eq!(r.status, 403);
    assert!(
        app.intents.is_empty(),
        "rejected request must not queue an intent"
    );

    // Session + CSRF → 204 and exactly one ExtendSetupWindow intent.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/session/extend",
            Value::Null,
            Some(&token),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 204);
    assert!(matches!(
        app.intents.as_slice(),
        [Intent::ExtendSetupWindow]
    ));
}

#[test]
fn diagnostics_omits_setup_window_on_host_but_serializes_when_set() {
    let mut app = app();
    let (token, _csrf) = login(&mut app);

    // Host default: field is None → omitted (skip_serializing_if) → old clients/sim unaffected.
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/diagnostics",
            Value::Null,
            Some(&token),
            None,
        ),
    );
    assert_eq!(r.status, 200);
    assert!(
        jval(&r).get("setup_window_s_remaining").is_none(),
        "None → field is omitted"
    );

    // Set (as mirrored by the firmware loop) → appears as a number.
    app.setup_window_s_remaining = Some(1800);
    let r = dispatch(
        &mut app,
        &req(
            Method::Get,
            "/api/v1/diagnostics",
            Value::Null,
            Some(&token),
            None,
        ),
    );
    assert_eq!(jval(&r)["setup_window_s_remaining"], 1800);
}

// ─────────────────────────────── Backup ───────────────────────────────

#[test]
fn backup_roundtrip_without_secrets() {
    let mut app = app();
    let (tok, csrf) = login(&mut app);
    seed(&mut app);
    app.setup.mqtt.host = "broker.local".into();
    app.setup.mqtt.username = "bridge".into();
    app.setup.mqtt.homekit_code = "123-45-678".into();
    app.setup.conn = telenot_app::ConnSettings {
        kind: telenot_app::dto::ConnKind::Tcp,
        ip: "192.0.2.10".into(),
        port: 1234,
        baud: 9600,
    };
    app.setup.remote_disarm = true;

    let r = dispatch(
        &mut app,
        &req(Method::Get, "/api/v1/backup", Value::Null, Some(&tok), None),
    );
    assert_eq!(r.status, 200);
    let export = jval(&r);
    assert_eq!(export["backup_version"], 1);
    assert_eq!(export["mqtt"]["host"], "broker.local");
    // Secrets/device-bound values are absent or cleared.
    assert_eq!(export["mqtt"]["homekit_code"], "");
    assert!(
        export["mqtt"].get("password").is_none(),
        "kein Passwort im Export"
    );
    assert_eq!(export["remote_disarm"], true);
    assert_eq!(export["conn"]["type"], "tcp");

    // Import into a FRESH app (as after erase-flash + first boot).
    let mut fresh = crate::app();
    let (tok2, csrf2) = login(&mut fresh);
    fresh.setup.mqtt.homekit_code = "999-99-999".into(); // device's own code
    let n_before = fresh.intents.len();
    let r = dispatch(
        &mut fresh,
        &req(
            Method::Post,
            "/api/v1/backup",
            export.clone(),
            Some(&tok2),
            Some(&csrf2),
        ),
    );
    assert_eq!(
        r.status,
        200,
        "Import ok: {:?}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(jval(&r)["sensors"], 2); // to_config drops only EXCLUDED, not unconfirmed
    assert_eq!(fresh.setup.mqtt.host, "broker.local");
    assert_eq!(
        fresh.setup.mqtt.homekit_code, "999-99-999",
        "eigener Code bleibt"
    );
    assert!(fresh.setup.remote_disarm);
    assert!(
        matches!(fresh.intents.get(n_before), Some(Intent::ReloadConfig(_))),
        "ReloadConfig intent for the daemon"
    );
    let _ = csrf;
}

#[test]
fn backup_import_rejects_bad_version_and_invalid_config() {
    let mut app = app();
    let (tok, csrf) = login(&mut app);

    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/backup",
            json!({"backup_version": 99, "config": {"schema_version": 1, "sensors": []},
                   "mqtt": {"host":"","port":8883,"tls":true,"verify_cert":true,
                            "username":"","topic_root":"t","ha_discovery":true},
                   "conn": {"type":"internal"}, "remote_disarm": false}),
            Some(&tok),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 400);
    assert_eq!(jval(&r)["error"]["code"], "backup_version");

    // schema_version mismatch in the config = Severity::Error → rejected.
    let r = dispatch(
        &mut app,
        &req(
            Method::Post,
            "/api/v1/backup",
            json!({"backup_version": 1, "config": {"schema_version": 0, "sensors": []},
                   "mqtt": {"host":"","port":8883,"tls":true,"verify_cert":true,
                            "username":"","topic_root":"t","ha_discovery":true},
                   "conn": {"type":"internal"}, "remote_disarm": false}),
            Some(&tok),
            Some(&csrf),
        ),
    );
    assert_eq!(r.status, 400);
    assert_eq!(jval(&r)["error"]["code"], "invalid_config");
}

#[test]
fn backup_requires_session() {
    let mut app = app();
    let r = dispatch(
        &mut app,
        &req(Method::Get, "/api/v1/backup", Value::Null, None, None),
    );
    assert_eq!(r.status, 401, "export requires a session");
}

//! MQTT broker settings, connection test and TOFU cert pinning (S5).

use super::{err, ok_json, parse_body};
use crate::app::{App, TestState};
use crate::dto::*;
use crate::mqtt::{MqttTarget, MqttTestResult};
use crate::runtime::Intent;
use crate::{ApiRequest, ApiResponse};

pub(super) fn handle_get(app: &App) -> ApiResponse {
    let m = &app.setup.mqtt;
    ok_json(
        200,
        &MqttDto {
            host: m.host.clone(),
            port: m.port,
            tls: m.tls,
            username: m.username.clone(),
            password_set: app.services.mqtt_password_set(),
            topic_root: m.topic_root.clone(),
            ha_discovery: m.ha_discovery,
            pinned: m.pinned_cert.is_some(),
            homekit_mode: m.homekit_mode,
            homekit_disarm: m.homekit_disarm,
        },
    )
}

pub(super) fn handle_put(app: &mut App, req: &ApiRequest) -> ApiResponse {
    match parse_body::<MqttPut>(req) {
        Ok(p) => {
            // Broker changed → any pinned cert is stale, discard it.
            if p.host != app.setup.mqtt.host {
                app.setup.mqtt.pinned_cert = None;
            }
            app.setup.mqtt.host = p.host;
            app.setup.mqtt.port = p.port;
            app.setup.mqtt.tls = p.tls;
            app.setup.mqtt.username = p.username;
            app.setup.mqtt.topic_root = p.topic_root;
            app.setup.mqtt.ha_discovery = p.ha_discovery;
            app.setup.mqtt.homekit_disarm = p.homekit_disarm;
            // HomeKit just enabled → start HAP live (no reboot). Idempotent:
            // firmware starts the HAP task only once (see homekit::start).
            let hk_turned_on = p.homekit_mode && !app.setup.mqtt.homekit_mode;
            app.setup.mqtt.homekit_mode = p.homekit_mode;
            if hk_turned_on {
                app.intents.push(Intent::StartHomekit);
            }
            if let Some(pw) = p.password {
                if !pw.is_empty() {
                    app.services.set_mqtt_password(&pw);
                }
            }
            app.setup.mqtt_tested = false;
            ApiResponse::empty(204)
        }
        Err(e) => e,
    }
}

pub(super) fn handle_test_start(app: &mut App) -> ApiResponse {
    let m = &app.setup.mqtt;
    let target = MqttTarget {
        host: m.host.clone(),
        port: m.port,
        tls: m.tls,
        username: m.username.clone(),
        password: app.services.mqtt_password(),
        pinned_cert: m.pinned_cert.clone(),
    };
    app.mqtt_test = crate::app::MqttTestView {
        state: TestState::Running,
        result: None,
        cert: None,
    };
    app.intents.push(Intent::TestMqtt(target));
    ok_json(202, &serde_json::json!({"state":"running"}))
}

pub(super) fn handle_test_status(app: &mut App) -> ApiResponse {
    let state = app.mqtt_test.state;
    let result = app.mqtt_test.result;
    if matches!(state, TestState::Done) && result.map(|r| r.ok()).unwrap_or(false) {
        app.setup.mqtt_tested = true;
    }
    let body = match state {
        TestState::Idle => serde_json::json!({"state":"idle"}),
        TestState::Running => serde_json::json!({"state":"running"}),
        TestState::Done => {
            let r = result.unwrap_or(MqttTestResult::Connect);
            // Presented broker cert (without PEM): on untrusted TLS the frontend shows
            // the TOFU pinning dialog; on success it shows the fingerprint for
            // inspection and optional pinning.
            let cert = app.mqtt_test.cert.as_ref().map(|c| {
                serde_json::json!({
                    "sha256": c.sha256,
                    "subject": c.subject,
                    "issuer": c.issuer,
                })
            });
            serde_json::json!({"state":"done","ok":r.ok(),"detail":r.detail(),"cert":cert})
        }
    };
    ok_json(200, &body)
}

/// TOFU: permanently trust (pin) the broker certificate presented during the test.
pub(super) fn handle_pin(app: &mut App) -> ApiResponse {
    if let Some(cert) = app.mqtt_test.cert.take() {
        app.setup.mqtt.pinned_cert = Some(cert.pem);
        app.setup.mqtt_tested = false; // re-test → now against the pinned cert
        ok_json(
            200,
            &serde_json::json!({"pinned": true, "sha256": cert.sha256}),
        )
    } else {
        err(
            409,
            "no_pending_cert",
            "Kein Zertifikat zum Pinnen vorhanden",
        )
    }
}

/// Remove pinning (broker changed / back to public CA).
pub(super) fn handle_unpin(app: &mut App) -> ApiResponse {
    app.setup.mqtt.pinned_cert = None;
    app.setup.mqtt_tested = false;
    ApiResponse::empty(204)
}

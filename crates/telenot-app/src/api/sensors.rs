//! EMA connection, discovery scan, sensor inventory (S2–S4) and the S8 review/commit.

use std::collections::BTreeMap;

use telenot_config::Severity;

use super::{err, ok_json, ok_json_cap, parse_body};
use crate::app::{App, TestState};
use crate::dto::*;
use crate::runtime::Intent;
use crate::{ApiRequest, ApiResponse};

/// Builds the live feed (S3) from a discovery snapshot — only the last `tail` entries as
/// `ApiSensor` (the scan view only shows the most recent rows anyway; materialising all
/// ~180 was an unnecessary ~40 KB transient on the ESP32 heap during a scan). Duplicate
/// detection still needs all items but works on cheap tuples.
fn feed_from(
    discovered: &Option<std::sync::Arc<(telenot_config::Config, BTreeMap<u16, String>)>>,
    tail: usize,
) -> Vec<ApiSensor> {
    let Some(d) = discovered else {
        return Vec::new();
    };
    let (cfg, raw) = &**d;
    let items: Vec<_> = cfg
        .sensors
        .iter()
        .map(|s| (s.address(), s.name().to_string(), s.kind()))
        .collect();
    let dups = crate::dup::compute_dup_map(&items);
    let skip = cfg.sensors.len().saturating_sub(tail);
    cfg.sensors
        .iter()
        .skip(skip)
        .map(|s| ApiSensor {
            address: s.address(),
            name: s.name().to_string(),
            name_ha: s.name_ha().to_string(),
            kind: s.kind(),
            topic: s.topic().to_string(),
            polarity: s.polarity(),
            confirmed: s.confirmed(),
            switchable: s.switchable(),
            switchable_eligible: telenot_core::profile::from_config_kind(cfg.panel.kind)
                .is_switchable_addr(s.address()),
            show_in_homekit: s.show_in_homekit(),
            raw_name: raw
                .get(&s.address())
                .cloned()
                .unwrap_or_else(|| s.name().to_string()),
            dup_of: dups.get(&s.address()).copied(),
            status: SensorStatus::Unconfirmed,
            include: true,
        })
        .collect()
}

/// S8 warnings: aggregated config issues plus state-based hints.
fn review_warnings(app: &App) -> Vec<WarningDto> {
    let mut agg: BTreeMap<&'static str, (Severity, usize, String)> = BTreeMap::new();
    for i in app.working_issues() {
        let e = agg
            .entry(i.code.as_str())
            .or_insert((i.severity, 0, i.message.clone()));
        e.1 += 1;
    }
    let mut out: Vec<WarningDto> = agg
        .into_iter()
        .map(|(code, (sev, count, msg))| WarningDto {
            code: code.into(),
            severity: sev,
            count,
            message: msg,
        })
        .collect();
    if !app.setup.mqtt_tested {
        out.push(WarningDto {
            code: "mqtt_never_tested".into(),
            severity: Severity::Warning,
            count: 1,
            message: "MQTT-Test war nie erfolgreich".into(),
        });
    }
    if app.setup.remote_disarm {
        out.push(WarningDto {
            code: "remote_disarm_active".into(),
            severity: Severity::Warning,
            count: 1,
            message: "Remote-Disarm ist aktiv".into(),
        });
    }
    out
}

pub(super) fn handle_connection_get(app: &App) -> ApiResponse {
    ok_json(
        200,
        &ConnectionDto {
            kind: app.setup.conn.kind,
            ip: app.setup.conn.ip.clone(),
            port: app.setup.conn.port,
            baud: app.setup.conn.baud,
            panel_kind: app.setup.panel.kind,
            gms_variant: app.setup.panel.gms_variant,
            hiplex_cmds_verified: app.setup.panel.hiplex_cmds_verified,
            eager_send: app.setup.panel.eager_send,
        },
    )
}

use crate::app::BAUD_ALLOWED;

pub(super) fn handle_connection_put(app: &mut App, req: &ApiRequest) -> ApiResponse {
    match parse_body::<ConnectionPut>(req) {
        Ok(p) => {
            // 0/absent = old web bundle without a baud field → keep the current value.
            let baud = match p.baud {
                0 => app.setup.conn.baud,
                b if BAUD_ALLOWED.contains(&b) => b,
                b => {
                    return super::err(
                        400,
                        "bad_baud",
                        &format!("Baudrate {b} nicht erlaubt (9600/115200)"),
                    )
                }
            };
            app.setup.conn = crate::app::ConnSettings {
                kind: p.kind,
                ip: p.ip,
                port: p.port,
                baud,
            };
            // Panel selection (S2): absent fields = keep (old web bundles). The command
            // gate is only meaningful for hiplex; validation flags it otherwise.
            if let Some(k) = p.panel_kind {
                app.setup.panel.kind = k;
            }
            if let Some(v) = p.gms_variant {
                app.setup.panel.gms_variant = v;
            }
            if let Some(f) = p.hiplex_cmds_verified {
                app.setup.panel.hiplex_cmds_verified = f;
            }
            if let Some(e) = p.eager_send {
                app.setup.panel.eager_send = e;
            }
            ApiResponse::empty(204)
        }
        Err(e) => e,
    }
}

pub(super) fn handle_connection_check_start(app: &mut App) -> ApiResponse {
    if matches!(app.setup.conn.kind, ConnKind::Tcp) {
        // Real probe of the configured TCP target (async in the daemon worker).
        app.conn_check = crate::app::ConnCheckView {
            state: TestState::Running,
            ok: false,
            detail: String::new(),
            last_frame_ms: None,
        };
        let (ip, port) = (app.setup.conn.ip.clone(), app.setup.conn.port);
        app.intents.push(Intent::CheckConnection { ip, port });
        ok_json(202, &serde_json::json!({"state":"checking"}))
    } else {
        // Internal UART: reflects the liveness of the actual link.
        let ago = app.live.last_frame_ms_ago;
        let ok = ago.map(|m| m < 12_000).unwrap_or(false);
        ok_json(
            200,
            &serde_json::json!({
                "state": if ok { "ok" } else { "error" },
                "last_frame_ms": ago,
                "detail": if ok { "reachable" } else { "no_frame" },
            }),
        )
    }
}

pub(super) fn handle_connection_check_status(app: &App) -> ApiResponse {
    let v = &app.conn_check;
    let state = match v.state {
        TestState::Idle => "idle",
        TestState::Running => "checking",
        TestState::Done => {
            if v.ok {
                "ok"
            } else {
                "error"
            }
        }
    };
    ok_json(
        200,
        &serde_json::json!({"state": state, "last_frame_ms": v.last_frame_ms, "detail": v.detail}),
    )
}

pub(super) fn handle_scan_cancel(app: &mut App, req: &ApiRequest) -> ApiResponse {
    let keep = if req.body.is_empty() {
        false
    } else {
        match parse_body::<ScanCancelReq>(req) {
            Ok(p) => p.keep_partial,
            Err(e) => return e,
        }
    };
    app.intents.push(Intent::CancelScan { keep_partial: keep });
    ok_json(
        200,
        &serde_json::json!({"phase": if keep {"done"} else {"idle"}}),
    )
}

pub(super) fn handle_scan_status(app: &App) -> ApiResponse {
    let s = app.live.scan.clone();
    // Only deliver the tail of the feed: the UI only shows the most recent rows
    // during a scan anyway, and the full list (~180 detectors ≈ 55 KB JSON) forced
    // 64 KB allocations during serialisation → OOM-abort on a fragmented heap.
    // The full view is available after the scan via GET /sensors.
    let feed = feed_from(&app.live.discovered, 15);
    ok_json(
        200,
        &ScanDto {
            phase: s.phase,
            total: s.total,
            scanned: s.scanned,
            named: s.named,
            elapsed: s.elapsed,
            remaining: s.remaining,
            current: s.current,
            feed,
        },
    )
}

pub(super) fn handle_sensors_get(app: &mut App, req: &ApiRequest) -> ApiResponse {
    // Lazy seed: populate if not yet done but a scan has completed.
    if !app.setup.seeded && app.live.scan.phase == ScanPhase::Done {
        if let Some(d) = app.live.discovered.clone() {
            let (cfg, raw) = (*d).clone();
            app.setup.seed(cfg, raw);
        }
    }
    // PAGINATED: the full list (~150 detectors ≈ 65 KB JSON) needs a contiguous
    // block during serialisation that the ESP32 heap reliably lacks after a scan
    // (OOM-abort in the field). The default limit also protects older clients.
    let total = app.sensors_len();
    // Clamp offset to `total` → consistent response (offset past the end yields
    // an empty page with the correct offset==total rather than a phantom value).
    let offset = req
        .query_param("offset")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
        .min(total);
    let limit = req
        .query_param("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(60)
        .clamp(1, 60);
    let sensors: Vec<ApiSensor> = app.sensor_page(offset, limit);
    let cap = 1024 + sensors.len() * 420;
    ok_json_cap(
        200,
        &SensorsResp {
            total,
            offset,
            sensors,
            counts: app.sensor_counts(),
        },
        cap,
    )
}

pub(super) fn handle_patch(app: &mut App, req: &ApiRequest, addr: u16) -> ApiResponse {
    let p: SensorPatch = match parse_body(req) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Read before `app.edit()` (which mutably borrows all of `app`) — the active edit
    // session's panel selection, not the persisted/committed one (PATCH edits the
    // in-progress setup state).
    let panel_kind = app.setup.panel.kind;
    let sess = app.edit();
    let Some(idx) = sess.find_idx(addr) else {
        return err(404, "not_found", "Sensor nicht gefunden");
    };
    let too_large = || {
        err(
            500,
            "config_too_large",
            "Konfiguration zu groß für den Gerätespeicher",
        )
    };
    {
        let mut m = sess.table.get_mut(idx).expect("idx from find_idx");
        // String setters append to the arena → can fail on an exhausted device heap.
        if let Some(v) = &p.name {
            if m.set_name(v).is_err() {
                return too_large();
            }
        }
        if let Some(v) = &p.name_ha {
            if m.set_name_ha(v).is_err() {
                return too_large();
            }
        }
        if let Some(v) = &p.topic {
            if m.set_topic(v).is_err() {
                return too_large();
            }
        }
        if let Some(v) = p.kind {
            m.set_kind(v);
        }
        if let Some(v) = p.polarity {
            m.set_polarity(v);
        }
        if let Some(v) = p.confirmed {
            m.set_confirmed(v);
        }
        if let Some(v) = p.switchable {
            // Fail-closed: only allow switching for real switch outputs — inputs and the
            // system-status block (arm/bypass addresses) always remain false.
            m.set_switchable(
                v && telenot_core::profile::from_config_kind(panel_kind).is_switchable_addr(addr),
            );
        }
        if let Some(v) = p.show_in_homekit {
            m.set_show_in_homekit(v);
        }
    }
    if let Some(inc) = p.include {
        sess.excluded[idx] = !inc;
    }
    let api = sess.api_at(idx, panel_kind);
    ok_json(200, &api)
}

pub(super) fn handle_bulk(app: &mut App, req: &ApiRequest) -> ApiResponse {
    let b: BulkReq = match parse_body(req) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let sess = app.edit();
    let mut updated = 0;
    for addr in &b.addresses {
        let Some(idx) = sess.find_idx(*addr) else {
            continue;
        };
        match b.op {
            BulkOp::Confirm => sess
                .table
                .get_mut(idx)
                .expect("idx from find_idx")
                .set_confirmed(true),
            BulkOp::Exclude => sess.excluded[idx] = true,
            BulkOp::Include => sess.excluded[idx] = false,
            BulkOp::SetKind => {
                if let Some(k) = b.kind {
                    sess.table
                        .get_mut(idx)
                        .expect("idx from find_idx")
                        .set_kind(k);
                }
            }
            BulkOp::SetPolarity => {
                if let Some(p) = b.polarity {
                    sess.table
                        .get_mut(idx)
                        .expect("idx from find_idx")
                        .set_polarity(p);
                }
            }
        }
        updated += 1;
    }
    ok_json(200, &BulkResp { updated })
}

pub(super) fn handle_review(app: &App) -> ApiResponse {
    let c = app.sensor_counts();
    ok_json(
        200,
        &ReviewDto {
            counts: c,
            mqtt_target: format!("{}:{}", app.setup.mqtt.host, app.setup.mqtt.port),
            mqtt_tested: app.setup.mqtt_tested,
            ha_discovery: app.setup.mqtt.ha_discovery,
            remote_disarm: app.setup.remote_disarm,
            schema_version: telenot_config::CURRENT_SCHEMA_VERSION,
            warnings: review_warnings(app),
        },
    )
}

pub(super) fn handle_commit(app: &mut App, req: &ApiRequest) -> ApiResponse {
    let body: CommitReq = if req.body.is_empty() {
        CommitReq {
            warnings_acknowledged: false,
        }
    } else {
        match parse_body(req) {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
    let issues = app.working_issues();
    if issues.iter().any(|i| i.severity == Severity::Error) {
        return err(400, "invalid_config", "Konfiguration enthält Fehler");
    }
    let warnings = review_warnings(app);
    if !warnings.is_empty() && !body.warnings_acknowledged {
        return err(
            409,
            "warnings_unacknowledged",
            "Offene Warnungen bestätigen",
        );
    }
    // Consumes the edit session in place (no second sensor table on the device heap).
    // Persistence and runtime reload are handled by the daemon when routing this intent.
    let cfg = app.take_commit_config();
    app.intents
        .push(Intent::ReloadConfig(std::sync::Arc::new(cfg)));
    ok_json(200, &CommitResp { rebooting: true })
}

//! Diagnostics (S7): live state, serial/MQTT/heap health, log ring and the GMS debug
//! capture for foreign panels.

use super::{err, ok_json, parse_body};
use crate::app::App;
use crate::dto::*;
use crate::runtime::Intent;
use crate::{ApiRequest, ApiResponse};

pub(super) fn handle_diagnostics(app: &App) -> ApiResponse {
    let l = &app.live;
    ok_json(
        200,
        &DiagnosticsDto {
            serial: SerialDiag {
                status: if l.availability == "online" {
                    "ok"
                } else {
                    "error"
                }
                .into(),
                last_frame_ms: l.last_frame_ms_ago,
            },
            mqtt: MqttDiag {
                status: if app.mqtt_connected {
                    "ok"
                } else if app.setup.mqtt.host.is_empty() {
                    "off"
                } else {
                    "connecting"
                }
                .into(),
                reconnects: 0,
                last_error: None,
                last_pub: "—".into(),
            },
            heap: app.heap.map(|(free, largest_free_block, low)| HeapDiag {
                free,
                largest_free_block,
                low,
            }),
            firmware_version: app.device.fw.clone(),
            schema_version: app.device.schema,
            uptime_s: app.services.now_ms() / 1000,
            reset_reason: app.boot_reason.into(),
            boot_count: app.boot_count,
            heap_low: app.heap_low,
            setup_window_s_remaining: app.setup_window_s_remaining,
        },
    )
}

pub(super) fn handle_log(app: &App, req: &ApiRequest) -> ApiResponse {
    let since = req
        .query_param("since_seq")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let (lines, dropped) = app.ring.since(since);
    let entries = lines
        .into_iter()
        .map(|e| LogEntryDto {
            seq: e.seq,
            t_ms: e.t_ms,
            level: e.level.into(),
            msg: e.msg.clone(),
        })
        .collect();
    ok_json(200, &LogResp { entries, dropped })
}

// Debug capture (GMS diagnostics for foreign panels). Start/Stop queue an intent
// (the serial loop owns the buffer + TX gate); status/download read `App::capture` directly.

pub(super) fn handle_capture_start(app: &mut App, req: &ApiRequest) -> ApiResponse {
    match parse_body::<CaptureStartReq>(req) {
        Ok(r) => match crate::app::CaptureMode::parse(&r.mode) {
            Some(mode) => {
                app.intents.push(Intent::CaptureStart { mode });
                ApiResponse::empty(202)
            }
            None => err(400, "bad_mode", "mode: listen|listen_ack|discover"),
        },
        Err(e) => e,
    }
}

pub(super) fn handle_capture_status(app: &App) -> ApiResponse {
    let c = &app.capture;
    let elapsed_s = c.elapsed_s(app.services.now_ms());
    ok_json(
        200,
        &CaptureStatusDto {
            active: c.active,
            mode: c.mode.as_str().into(),
            sends: c.mode.sends_desc().into(),
            bytes_total: c.bytes_total,
            frames_ok: c.frames_ok,
            frames_err: c.frames_err,
            rec_types: c
                .rec_types
                .iter()
                .map(|(&satztyp, &count)| RecTypeCount {
                    satztyp,
                    hex: format!("0x{satztyp:02X}"),
                    count,
                })
                .collect(),
            elapsed_s,
            buf_used: c.buf_used(),
            buf_cap: c.cap(),
        },
    )
}

pub(super) fn handle_state(app: &App) -> ApiResponse {
    let l = &app.live;
    ok_json(
        200,
        &StateDto {
            arm_state: l.arm_state.into(),
            availability: l.availability.into(),
            intern_ready: l.intern_ready,
            extern_ready: l.extern_ready,
            sensor_states: l
                .sensor_states
                .iter()
                .map(|&(address, active)| SensorStateDto { address, active })
                .collect(),
            polarity_observed: l
                .polarity_result
                .map(|(address, polarity)| PolarityObservedDto { address, polarity }),
            command_result: l
                .command_result
                .clone()
                .map(|(text, ms_ago)| crate::dto::CommandResultDto { text, ms_ago }),
        },
    )
}

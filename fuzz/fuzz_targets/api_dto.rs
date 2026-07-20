//! Fuzzes JSON deserialization of all mutating request DTOs (the path through
//! which untrusted HTTP bodies arrive) plus the shared REST/MQTT command parser
//! including the authorization pre-check.
#![no_main]

use libfuzzer_sys::fuzz_target;
use telenot_app::dto::{
    BulkReq, CaptureStartReq, CommandReq, CommitReq, ConnectionPut, LoginReq, MqttPut,
    PasswordReq, PinPut, RemoteDisarmPut, ScanCancelReq, SensorPatch,
};

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<LoginReq>(data);
    let _ = serde_json::from_slice::<PasswordReq>(data);
    let _ = serde_json::from_slice::<SensorPatch>(data);
    let _ = serde_json::from_slice::<BulkReq>(data);
    let _ = serde_json::from_slice::<ConnectionPut>(data);
    let _ = serde_json::from_slice::<MqttPut>(data);
    let _ = serde_json::from_slice::<PinPut>(data);
    let _ = serde_json::from_slice::<RemoteDisarmPut>(data);
    let _ = serde_json::from_slice::<CaptureStartReq>(data);
    let _ = serde_json::from_slice::<CommitReq>(data);
    let _ = serde_json::from_slice::<ScanCancelReq>(data);

    // Shared command path (REST + MQTT): JSON → BridgeCommand + PIN requirement.
    let _ = telenot_app::parse_command_message(data, false);
    let _ = telenot_app::parse_command_message(data, true);
    if let Ok(req) = serde_json::from_slice::<CommandReq>(data) {
        let _ = telenot_app::parse_bridge_command(&req);
    }
});

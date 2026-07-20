//! OTA status + settings. The image upload does NOT go through `dispatch` — it streams
//! directly into the flash partition (see mod.rs routing comment).

use super::{ok_json, parse_body};
use crate::app::App;
use crate::dto::*;
use crate::{ApiRequest, ApiResponse};

pub(super) fn handle_status(app: &App) -> ApiResponse {
    let (running, boot) = match &app.ota_slots {
        Some((r, b)) => (Some(r.clone()), Some(b.clone())),
        None => (None, None),
    };
    ok_json(
        200,
        &OtaStatusDto {
            state: app.ota.state_str(),
            received: app.ota.received,
            total: app.ota.total,
            progress_pct: app.ota.progress_pct(),
            error: app.ota.error,
            new_version: app.ota.new_version.clone(),
            running_slot: running,
            boot_slot: boot,
            pending_verify: app.ota_pending_verify,
            self_test_s_remaining: app.ota_self_test_s,
            latest_version: app.ota_latest.clone(),
            update_check: app.update_check,
        },
    )
}

pub(super) fn handle_settings_put(app: &mut App, req: &ApiRequest) -> ApiResponse {
    match parse_body::<OtaSettingsPut>(req) {
        Ok(p) => {
            app.update_check = p.update_check;
            if !p.update_check {
                app.ota_latest = None; // avoid stale hint after disabling
            }
            ApiResponse::empty(204)
        }
        Err(e) => e,
    }
}

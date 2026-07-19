//! PIN + Remote-Disarm opt-in (S6). Fail-closed: enabling remote disarm requires a PIN
//! and an explicit risk acknowledgement.

use super::{err, ok_json, parse_body};
use crate::app::App;
use crate::dto::*;
use crate::security::{is_weak_pin, pin_format_ok};
use crate::{ApiRequest, ApiResponse};

pub(super) fn handle_get(app: &App) -> ApiResponse {
    ok_json(
        200,
        &SecurityDto {
            pin_set: app.services.pin_set(),
            remote_disarm: app.setup.remote_disarm,
        },
    )
}

pub(super) fn handle_put_pin(app: &mut App, req: &ApiRequest) -> ApiResponse {
    match parse_body::<PinPut>(req) {
        Ok(p) => {
            if !pin_format_ok(&p.pin) {
                return err(400, "pin_invalid", "PIN numerisch, mindestens 4 Stellen");
            }
            let weak = is_weak_pin(&p.pin);
            app.services.set_pin(&p.pin);
            ok_json(
                200,
                &PinResp {
                    pin_set: true,
                    weak,
                },
            )
        }
        Err(e) => e,
    }
}

pub(super) fn handle_put_remote_disarm(app: &mut App, req: &ApiRequest) -> ApiResponse {
    match parse_body::<RemoteDisarmPut>(req) {
        Ok(p) => {
            if p.enabled {
                if !app.services.pin_set() {
                    return err(409, "pin_required", "Bitte zuerst eine PIN setzen");
                }
                if !p.acknowledged {
                    return err(409, "ack_required", "Risiko muss bestätigt werden");
                }
            }
            app.setup.remote_disarm = p.enabled;
            ApiResponse::empty(204)
        }
        Err(e) => e,
    }
}

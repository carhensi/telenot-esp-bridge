//! Settings backup export/import (migration safety net). Intentionally excludes secrets.

use telenot_config::Severity;

use super::{err, ok_json, parse_body};
use crate::app::App;
use crate::dto::*;
use crate::runtime::Intent;
use crate::{ApiRequest, ApiResponse};

/// Format version of the settings backup (`BackupDto.backup_version`).
pub const BACKUP_VERSION: u32 = 1;

pub(super) fn handle_export(app: &App) -> ApiResponse {
    let mut mqtt = app.setup.mqtt.clone();
    // Pairing code is device-bound (secret-like) — never include in the backup.
    mqtt.homekit_code = String::new();
    let config = match app.working_config() {
        Ok(c) => c,
        Err(_) => {
            return err(
                500,
                "config_too_large",
                "Konfiguration zu groß für den Gerätespeicher",
            )
        }
    };
    ok_json(
        200,
        &BackupDto {
            backup_version: BACKUP_VERSION,
            config,
            mqtt,
            conn: BackupConn {
                kind: app.setup.conn.kind,
                ip: app.setup.conn.ip.clone(),
                port: app.setup.conn.port,
                baud: app.setup.conn.baud,
            },
            remote_disarm: app.setup.remote_disarm,
        },
    )
}

pub(super) fn handle_import(app: &mut App, req: &ApiRequest) -> ApiResponse {
    let b: BackupDto = match parse_body(req) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if b.backup_version != BACKUP_VERSION {
        return err(400, "backup_version", "Unbekannte Sicherungs-Version");
    }
    let issues = b.config.validate();
    if issues.iter().any(|i| i.severity == Severity::Error) {
        return err(
            400,
            "invalid_config",
            "Sicherung enthält ungültige Konfiguration",
        );
    }
    // Preserve the device's own HomeKit code — the export clears it, and a foreign
    // code from a backup must never overwrite the local one.
    let own_code = std::mem::take(&mut app.setup.mqtt.homekit_code);
    app.setup.mqtt = b.mqtt;
    app.setup.mqtt.homekit_code = own_code;
    app.setup.conn = crate::app::ConnSettings {
        kind: b.conn.kind,
        ip: b.conn.ip,
        port: b.conn.port,
        // Older backups carry no baud (0) — keep the device's current value; foreign
        // values outside the allowlist are ignored the same way.
        baud: if crate::app::BAUD_ALLOWED.contains(&b.conn.baud) {
            b.conn.baud
        } else {
            app.setup.conn.baud
        },
    };
    app.setup.remote_disarm = b.remote_disarm;
    let sensors = b.config.sensors.len();
    // Session shows the import immediately (persisted seed: nothing auto-hidden).
    app.setup.seed_persisted(b.config.clone());
    // Persistence (cfg blob + settings) and runtime reload are handled by the daemon —
    // same path as /commit and the settings-change detection in the loop.
    app.intents
        .push(Intent::ReloadConfig(std::sync::Arc::new(b.config)));
    ok_json(200, &BackupImportResp { sensors })
}

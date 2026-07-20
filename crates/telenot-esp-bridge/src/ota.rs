//! EspOta wrapper: slot overview, mark-valid, and the streaming update write.
//! The state machine/validation logic is portable (`telenot_app::ota`); only the
//! flash side lives here. `EspOta::new()` is a singleton — each function scopes
//! its own instance so they don't block each other.

use esp_idf_svc::ota::{EspFirmwareInfoLoad, EspOta, Slot, SlotState};
use esp_idf_svc::sys::EspError;
use telenot_app::ota::{sha256_matches, Digest, Sha256, ESP_IMAGE_MAGIC};
use telenot_app::SlotInfo;

/// image_header (24) + segment_header (8) + app_desc (256) — enough header for magic check
/// + version string; 512 leaves headroom.
const HEAD_LEN: usize = 512;

fn state_str(s: &SlotState) -> &'static str {
    match s {
        SlotState::Factory => "factory",
        SlotState::Valid => "valid",
        SlotState::Invalid => "invalid",
        SlotState::Unverified => "unverified",
        SlotState::Unknown => "unknown",
    }
}

fn to_info(s: &Slot) -> SlotInfo {
    SlotInfo {
        label: s.label.as_str().to_string(),
        state: state_str(&s.state).to_string(),
        version: s.firmware.as_ref().map(|f| f.version.as_str().to_string()),
    }
}

/// Slot overview at boot: `(running, boot, pending_verify, rolled_back_from)`.
/// `pending_verify` = this firmware is booting for the first time after an OTA and must
/// pass the self-test; `rolled_back_from` = label of a slot marked INVALID (a rollback
/// occurred). `None` if no OTA partitions exist (old partition layout).
pub fn slot_overview() -> Option<(SlotInfo, SlotInfo, bool, Option<String>)> {
    let ota = EspOta::new().ok()?;
    let running = ota.get_running_slot().ok()?;
    let boot = ota.get_boot_slot().ok()?;
    let pending = running.state == SlotState::Unverified;
    let rolled_back = ota
        .get_last_invalid_slot()
        .ok()
        .flatten()
        .map(|s| s.label.as_str().to_string());
    Some((to_info(&running), to_info(&boot), pending, rolled_back))
}

/// Self-test passed → make the running slot permanent (close the rollback window).
pub fn mark_valid() -> Result<(), EspError> {
    EspOta::new()?.mark_running_slot_valid()
}

/// Streams an app image into the inactive slot: read chunks → update SHA256 → write to flash.
/// `read` behaves like `io::Read` returning the chunk length (0 = EOF).
/// Returns the version from the app descriptor. Errors are the stable UI codes from
/// `telenot_app::ota`. On every error path `EspOtaUpdate::drop` cleans up via esp_ota_abort.
pub fn stream_update(
    total: usize,
    expected_sha: &str,
    mut read: impl FnMut(&mut [u8]) -> Result<usize, ()>,
    mut progress: impl FnMut(usize),
) -> Result<String, &'static str> {
    let mut ota = EspOta::new().map_err(|_| "busy")?;
    // known_size: erases only the needed region instead of the full 4 MB (faster).
    let mut update = ota.initiate_update_with_known_size(total).map_err(|e| {
        log::error!("OTA begin fehlgeschlagen: {e}");
        "write_failed"
    })?;

    let mut sha = Sha256::new();
    let mut buf = [0u8; 4096];
    let mut head: Vec<u8> = Vec::with_capacity(HEAD_LEN);
    let mut received = 0usize;
    let mut version: Option<String> = None;

    loop {
        let n = read(&mut buf).map_err(|_| "read_failed")?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        received += n;
        if received > total {
            return Err("too_large"); // more bytes than announced by Content-Length
        }
        if head.len() < HEAD_LEN {
            let want = (HEAD_LEN - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..want]);
            // Early rejection BEFORE further flash wear: no esp-idf image magic
            // → this is not a firmware image (e.g. accidentally uploaded the merged
            // image or a text file).
            if head[0] != ESP_IMAGE_MAGIC {
                return Err("bad_image");
            }
            if version.is_none() {
                if let Some(fi) = EspFirmwareInfoLoad.fetch_native(&head) {
                    let v: Vec<u8> = fi
                        .app_desc
                        .version
                        .iter()
                        .take_while(|&&c| c != 0)
                        .map(|&c| c as u8)
                        .collect();
                    version = Some(String::from_utf8_lossy(&v).into_owned());
                }
            }
        }
        sha.update(chunk);
        update.write(chunk).map_err(|e| {
            log::error!("OTA write fehlgeschlagen bei {received} B: {e}");
            "write_failed"
        })?;
        progress(received);
    }

    if received != total {
        return Err("too_small"); // connection dropped / fewer bytes than announced
    }
    let digest = sha.finalize();
    if !sha256_matches(expected_sha, &digest) {
        return Err("sha_mismatch");
    }
    // esp_ota_end validates the complete image (magic, checksums, segments) and only
    // THEN sets the boot partition.
    update.complete().map_err(|e| {
        log::error!("OTA complete fehlgeschlagen: {e}");
        "bad_image"
    })?;
    Ok(version.unwrap_or_default())
}

/// Source for the update check: the `ota.json` asset on the latest GitHub release
/// (stable redirect URL). Outbound only; works only once the repo is public —
/// until then GitHub returns 404 and the check stays silent.
const OTA_MANIFEST_URL: &str =
    "https://github.com/carhensi/telenot-esp-bridge/releases/latest/download/ota.json";

/// Fetches the release manifest (TLS via CA bundle, follows GitHub redirects). `None` on
/// any error — the check is a convenience feature, never operationally critical.
pub fn fetch_latest_manifest() -> Option<telenot_app::ota::OtaManifest> {
    use esp_idf_svc::http::client::{Configuration, EspHttpConnection, FollowRedirectsPolicy};
    let mut conn = EspHttpConnection::new(&Configuration {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        follow_redirects_policy: FollowRedirectsPolicy::FollowAll,
        timeout: Some(core::time::Duration::from_secs(15)),
        ..Default::default()
    })
    .ok()?;
    conn.initiate_request(esp_idf_svc::http::Method::Get, OTA_MANIFEST_URL, &[])
        .ok()?;
    conn.initiate_response().ok()?;
    if conn.status() != 200 {
        log::info!("Update-Check: HTTP {} (Repo noch privat?)", conn.status());
        return None;
    }
    let mut body = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = conn.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        if body.len() > 4096 {
            return None; // ota.json is tiny — anything larger is not a manifest
        }
    }
    telenot_app::ota::parse_ota_manifest(&body)
}

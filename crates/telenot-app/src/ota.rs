//! Portable OTA state + host-testable building blocks. The actual flash writing (EspOta)
//! lives in `telenot-esp-bridge/src/ota.rs`; the sim mocks it. This module contains everything
//! that can be decided without hardware: state machine, SHA comparison, self-test timing,
//! and length plausibility.

use serde::Serialize;

// Re-export: firmware (streaming route) and sim hash with the SAME hasher
// without each having to declare sha2 separately.
pub use sha2::{Digest, Sha256};

/// Smaller than the smallest conceivable app image → reject immediately (no empty/broken file).
pub const OTA_MIN_LEN: usize = 64 * 1024;
/// Size of the OTA slots (`partitions.csv`: ota_0/ota_1, 4 MB each).
pub const OTA_MAX_LEN: usize = 0x40_0000;
/// First byte of an esp-idf app image (magic) — enables early rejection BEFORE the flash write.
pub const ESP_IMAGE_MAGIC: u8 = 0xE9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaPhase {
    Idle,
    Receiving,
    /// Image written + verified, boot slot set — only a reboot is missing.
    ReadyToReboot,
    Failed,
}

/// Maintained by the upload handler, read by `GET /ota` (under the app mutex).
#[derive(Debug, Clone)]
pub struct OtaState {
    pub phase: OtaPhase,
    pub received: usize,
    pub total: Option<usize>,
    /// Stable error code for the UI: `sha_mismatch`, `too_small`, `too_large`,
    /// `bad_image`, `write_failed`, `busy`, `length_required`.
    pub error: Option<&'static str>,
    /// Version from the app descriptor of the uploaded image.
    pub new_version: Option<String>,
}

impl Default for OtaState {
    fn default() -> Self {
        OtaState {
            phase: OtaPhase::Idle,
            received: 0,
            total: None,
            error: None,
            new_version: None,
        }
    }
}

impl OtaState {
    pub fn receiving(total: usize) -> Self {
        OtaState {
            phase: OtaPhase::Receiving,
            received: 0,
            total: Some(total),
            error: None,
            new_version: None,
        }
    }
    pub fn failed(code: &'static str) -> Self {
        OtaState {
            phase: OtaPhase::Failed,
            error: Some(code),
            ..OtaState::default()
        }
    }
    pub fn state_str(&self) -> &'static str {
        match self.phase {
            OtaPhase::Idle => "idle",
            OtaPhase::Receiving => "receiving",
            OtaPhase::ReadyToReboot => "ready_to_reboot",
            OtaPhase::Failed => "failed",
        }
    }
    pub fn progress_pct(&self) -> u8 {
        match self.total {
            Some(t) if t > 0 => ((self.received as u64 * 100) / t as u64).min(100) as u8,
            _ => 0,
        }
    }
}

/// Validates the announced Content-Length BEFORE the first byte is received.
pub fn len_ok(len: usize) -> Result<(), &'static str> {
    if len < OTA_MIN_LEN {
        return Err("too_small");
    }
    if len > OTA_MAX_LEN {
        return Err("too_large");
    }
    Ok(())
}

/// Compares an expected SHA-256 (hex, case-insensitive, trimmed) against a digest.
pub fn sha256_matches(expected_hex: &str, digest: &[u8]) -> bool {
    let expected = expected_hex.trim();
    if expected.len() != digest.len() * 2 {
        return false;
    }
    expected.as_bytes().chunks(2).zip(digest).all(|(pair, b)| {
        let hi = (pair[0] as char).to_digit(16);
        let lo = (pair[1] as char).to_digit(16);
        matches!((hi, lo), (Some(h), Some(l)) if (h * 16 + l) as u8 == *b)
    })
}

/// Reads the version from an esp-idf app image header: `esp_app_desc_t.version` is located
/// at image_header (24) + segment_header (8) + 16 B into the descriptor, 32 B NUL-terminated.
/// Used by the sim mock and tests; firmware uses the authoritative `EspFirmwareInfoLoad`.
pub fn app_desc_version(head: &[u8]) -> Option<String> {
    const OFF: usize = 24 + 8 + 16;
    let bytes = head.get(OFF..OFF + 32)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(32);
    core::str::from_utf8(&bytes[..end]).ok().map(str::to_string)
}

/// Self-test window after the first boot of new firmware: the slot is only marked valid once
/// the window has elapsed (device ran N minutes without a panic). A panic/reboot before that
/// triggers bootloader rollback. Deliberately NO panel-connection check: a disconnected panel
/// must not cause a firmware rollback.
#[derive(Debug, Clone, Copy)]
pub struct SelfTest {
    deadline_ms: u64,
}

impl SelfTest {
    pub const DEFAULT_DURATION_S: u32 = 300;
    pub fn new(now_ms: u64, duration_s: u32) -> Self {
        SelfTest {
            deadline_ms: now_ms + duration_s as u64 * 1000,
        }
    }
    pub fn due(&self, now_ms: u64) -> bool {
        now_ms >= self.deadline_ms
    }
    pub fn remaining_s(&self, now_ms: u64) -> u32 {
        (self.deadline_ms.saturating_sub(now_ms)).div_ceil(1000) as u32
    }
}

/// Release manifest (`ota.json` attached to the GitHub release): consumed by the device-side
/// update check. The check is informational only — installation is always manual.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct OtaManifest {
    pub version: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub url: String,
}

pub fn parse_ota_manifest(json: &[u8]) -> Option<OtaManifest> {
    serde_json::from_slice(json).ok()
}

/// Is `candidate` a NEWER version than `current`? Numeric comparison of `major.minor.patch`;
/// suffixes (`-dev`, etc.) are ignored. Unparseable versions are conservatively never "newer"
/// (no update prompt on garbage data).
pub fn version_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<[u64; 3]> {
        let core = v.trim().trim_start_matches('v');
        let core = core.split(['-', '+']).next()?;
        let mut it = core.split('.');
        let maj = it.next()?.parse().ok()?;
        let min = it.next().unwrap_or("0").parse().ok()?;
        let pat = it.next().unwrap_or("0").parse().ok()?;
        Some([maj, min, pat])
    }
    match (parse(candidate), parse(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

/// Display mirror of an app slot (populated by firmware at boot, mocked by the sim).
#[derive(Debug, Clone, Serialize)]
pub struct SlotInfo {
    pub label: String,
    /// `valid` | `unverified` | `invalid` | `factory` | `unknown`
    pub state: String,
    pub version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_matches_case_insensitive_and_rejects_garbage() {
        let mut h = Sha256::new();
        h.update(b"telenot");
        let d = h.finalize();
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        assert!(sha256_matches(&hex, &d));
        assert!(sha256_matches(&hex.to_uppercase(), &d));
        assert!(sha256_matches(&format!("  {hex}  "), &d));
        assert!(!sha256_matches(&hex[1..], &d), "wrong length");
        let mut wrong = hex.clone();
        wrong.replace_range(0..1, if &hex[0..1] == "0" { "1" } else { "0" });
        assert!(!sha256_matches(&wrong, &d));
        assert!(!sha256_matches("zz".repeat(32).as_str(), &d), "not hex");
    }

    #[test]
    fn len_bounds() {
        assert_eq!(len_ok(OTA_MIN_LEN - 1), Err("too_small"));
        assert_eq!(len_ok(OTA_MIN_LEN), Ok(()));
        assert_eq!(len_ok(OTA_MAX_LEN), Ok(()));
        assert_eq!(len_ok(OTA_MAX_LEN + 1), Err("too_large"));
    }

    #[test]
    fn self_test_timing() {
        let st = SelfTest::new(10_000, 300);
        assert!(!st.due(10_000));
        assert_eq!(st.remaining_s(10_000), 300);
        assert!(!st.due(309_999));
        assert_eq!(st.remaining_s(309_999), 1);
        assert!(st.due(310_000));
        assert_eq!(st.remaining_s(310_000), 0);
        // Monotonic time never runs backwards on the ESP32, but remaining_s saturates.
        assert_eq!(st.remaining_s(400_000), 0);
    }

    #[test]
    fn manifest_parse_and_version_compare() {
        let m = parse_ota_manifest(
            br#"{"version":"1.2.3","sha256":"ab","size":100,"url":"https://x"}"#,
        )
        .unwrap();
        assert_eq!(m.version, "1.2.3");
        assert!(parse_ota_manifest(b"garbage").is_none());
        assert!(
            parse_ota_manifest(br#"{"sha256":"x"}"#).is_none(),
            "version missing"
        );

        assert!(version_newer("1.2.3", "1.2.2"));
        assert!(version_newer("2.0.0", "1.9.9"));
        assert!(version_newer("v1.3.0", "1.2.9"));
        assert!(version_newer("0.2.0-rc1", "0.1.0"));
        assert!(!version_newer("1.2.3", "1.2.3"));
        assert!(!version_newer("1.2.2", "1.2.3"));
        assert!(
            !version_newer("kaputt", "1.0.0"),
            "unparseable is never newer"
        );
        assert!(!version_newer("1.0.0", "kaputt"));
    }

    #[test]
    fn progress_and_states() {
        let mut s = OtaState::receiving(1000);
        assert_eq!(s.state_str(), "receiving");
        s.received = 250;
        assert_eq!(s.progress_pct(), 25);
        s.received = 2000; // more than announced → clamped to 100%
        assert_eq!(s.progress_pct(), 100);
        assert_eq!(OtaState::default().progress_pct(), 0);
        assert_eq!(OtaState::failed("sha_mismatch").state_str(), "failed");
    }
}

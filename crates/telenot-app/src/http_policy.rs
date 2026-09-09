//! Shared, bounded HTTP ingress policy; no I/O or URL/network dependencies.
//!
//! Origin/CSRF checking lives in `api::origin_matches_host` (exact authority
//! match, added in the 2026-09 security review) — this module only bounds
//! request-body allocation per route.

/// Per-route request-body ceiling, applied BEFORE allocating/reading the body.
///
/// `/backup` keeps the previous global 256 KiB ceiling: a settings backup grows
/// with the sensor table (~234 bytes/sensor JSON; 600 sensors ≈ 137 KiB) and a
/// device MUST always be able to re-import its own export.
pub fn body_limit(path: &str) -> usize {
    if path == "/api/v1/backup" {
        256 * 1024
    } else if path == "/api/v1/sensors/bulk" {
        8 * 1024
    } else {
        2 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_routes_cannot_allocate_import_sized_bodies() {
        assert_eq!(body_limit("/api/v1/sensors/123"), 2048);
        assert_eq!(body_limit("/api/v1/sensors/bulk"), 8192);
    }

    #[test]
    fn backup_import_keeps_full_ceiling_for_max_sensor_configs() {
        // 600 sensors ≈ 137 KiB JSON — the limit must never lock a device out
        // of importing its own backup (regression guard for the 32 KiB cut).
        assert_eq!(body_limit("/api/v1/backup"), 262_144);
        assert!(body_limit("/api/v1/backup") > 600 * 234);
    }
}

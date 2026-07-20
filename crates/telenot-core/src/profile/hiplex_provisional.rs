//! # hiplex 8400H — PROVISIONAL address map. UNVERIFIED. DO NOT TRUST BLINDLY.
//!
//! Every value below is inferred, not captured. Sources per value are annotated:
//!
//! - `[§7]` — the GMS address model of the KNX-400-IP interface description (§7). The
//!   KNX 400 IP is Telenot's own GMS-lite consumer for the hiplex (Prospekt
//!   DE-6100034-05 S. 3, Featurepaket F06: "Interface KNX 400 IP / GMS lite"), so its
//!   address model is the best available proxy for hiplex GMS lite.
//! - `[Prospekt]` — official hiplex prospectus DE-6100034-05 (S. 7/9): 15 Sicherungs-
//!   bereiche + 1 Zentralen-Schutzbereich, 512 Meldebereiche.
//! - `[SecMap]` — complex/hiplex comparison (64 Schaltaktionen).
//! - `[ASSUMED]` — structural extrapolation of the §7 pattern. Weakest tier.
//!
//! **Verification protocol** (docs/hiplex.md H1): diff the first real capture's 0x24
//! layout against these constants, fix constants only, re-run `tests/hiplex.rs` (bit
//! positions there are computed from this profile, so tests stay valid). The COMMAND
//! path is additionally gated by `config.panel.hiplex_cmds_verified` (default `false`)
//! until this file has been capture-verified — reads are safe, writes are not.

use super::{AreaLayout, PanelKind, PanelProfile};

/// Telenot hiplex 8400H (GMS lite address model) — PROVISIONAL, see module docs.
pub const HIPLEX8400: PanelProfile = PanelProfile {
    kind: PanelKind::Hiplex8400,
    areas: AreaLayout {
        // [§7] complex area 1 block starts at 0x0530; [ASSUMED] hiplex shares the base.
        base: 0x0530,
        // [§7] complex reserves 0x0530..0x0570 for its 8 areas = 8 bits per area;
        // [ASSUMED] hiplex keeps the 8-bit stride.
        stride: 8,
        // [Prospekt] 15 Sicherungsbereiche + Zentralen-Schutzbereich (id 16).
        count: 16,
        // [§7] same per-area bit layout as the complex (pcap-verified there).
        off_unscharf: 0,
        off_intern_scharf: 1,
        off_extern_scharf: 2,
        off_alarm: 3,
        off_intern_bereit: 5,
        off_extern_bereit: 6,
    },
    // [ASSUMED] §7 pattern "MB status directly after the area blocks": complex has
    // 0x0570 = base + 8 areas × 8 bits; hiplex analog: 0x0530 + 16 × 8 = 0x05B0.
    mb_status_base: 0x05B0,
    // [ASSUMED] §7 pattern "MB bypassed after MB status": 0x05B0 + 512 = 0x07B0.
    mb_gesperrt_base: 0x07B0,
    // [Prospekt] 512 Meldebereiche.
    mb_max: 512,
    // [SecMap] 64 Schaltaktionen exist, but their GMS base address is UNKNOWN —
    // `None` keeps the Schaltaktion command path disabled until a capture names it.
    schaltaktion_base: None,
    schaltaktion_max: 64,
    // [ASSUMED] conservative: only the window BEFORE the status block is potentially
    // switchable — with unverified addresses nothing else may be switched anyway.
    output_addr_range: 0x0500..0x0530,
    // [ASSUMED] everything from the area blocks through the MB-bypassed window is
    // system status and must never be switchable: 0x0530..(0x07B0 + 512).
    status_addr_range: 0x0530..0x09B0,
};

#[cfg(test)]
mod tests {
    use super::HIPLEX8400;

    /// Structural sanity — independent of whether the absolute addresses are right:
    /// the windows must not overlap and the status range must cover them all.
    #[test]
    fn provisional_layout_is_self_consistent() {
        let p = &HIPLEX8400;
        let areas_end = p.area_addr(p.areas.count, 7) + 1;
        assert!(areas_end <= p.mb_status_base, "areas overlap MB status");
        assert!(
            p.mb_status_base + p.mb_max <= p.mb_gesperrt_base,
            "MB status overlaps MB bypassed"
        );
        let status_end = p.mb_gesperrt_base + p.mb_max;
        assert!(p.status_addr_range.start <= p.areas.base);
        assert!(p.status_addr_range.end >= status_end);
        // Nothing inside the status window is ever switchable.
        for addr in [
            p.areas.base,
            p.addr_alarm(16),
            p.mb_status_base,
            p.addr_mb_gesperrt(512),
        ] {
            assert!(
                !p.is_switchable_addr(addr),
                "0x{addr:04X} must not be switchable"
            );
        }
    }
}

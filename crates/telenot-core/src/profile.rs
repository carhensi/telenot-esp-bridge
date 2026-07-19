//! Panel profiles — ALL panel-specific addresses/topology live here, nowhere else.
//!
//! A profile is a `&'static` const struct selected once in [`crate::Core::new`]; the
//! sans-IO core stays free of panel `match`es on the hot path. Correcting an address
//! after a capture verification is a constants-only change.
//!
//! Invariant (guarded by `profile_matches_legacy_constants` in `lib.rs`): [`COMPLEX400`]
//! equals the historical literals byte-for-byte — the complex 400H behavior is verified
//! against real captures and must never drift.

mod hiplex_provisional;
pub use hiplex_provisional::HIPLEX8400;

use core::ops::Range;

/// Supported panel families. Serialized form lives in `telenot-config` (`panel.kind`);
/// this is the domain-side discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    Complex400,
    Hiplex8400,
}

/// Layout of the per-area status bits inside the cyclic 0x24 output block.
pub struct AreaLayout {
    /// Address of area 1's block (complex: 0x0530, pcap-verified).
    pub base: u16,
    /// Address distance between consecutive area blocks. Only relevant for `count > 1`;
    /// unverified for both panels until a multi-area capture exists.
    pub stride: u16,
    /// Number of areas exposed by this profile (complex today: 1 = today's behavior;
    /// hiplex: 15 Sicherungsbereiche + Zentralen-Schutzbereich = 16).
    pub count: u8,
    // Bit offsets within an area block:
    pub off_unscharf: u16,
    pub off_intern_scharf: u16,
    pub off_extern_scharf: u16,
    pub off_alarm: u16,
    pub off_intern_bereit: u16,
    pub off_extern_bereit: u16,
}

/// Panel-specific address map + topology limits.
pub struct PanelProfile {
    pub kind: PanelKind,
    pub areas: AreaLayout,
    /// Base of the cyclic "detection area N state" bits (complex: 0x0570).
    pub mb_status_base: u16,
    /// Base of "detection area N bypassed" (complex: 0x05F0 + (N-1)).
    pub mb_gesperrt_base: u16,
    /// Highest detection area number (complex: 128; hiplex: 512).
    pub mb_max: u16,
    /// Base address of remotely triggerable Schaltaktionen; `None` = feature absent
    /// (complex switches raw outputs via the allowlist instead).
    pub schaltaktion_base: Option<u16>,
    pub schaltaktion_max: u8,
    /// Address window that may contain switchable outputs.
    pub output_addr_range: Range<u16>,
    /// System-status window — NEVER switchable (arm/bypass state lives here).
    pub status_addr_range: Range<u16>,
}

impl PanelProfile {
    /// Absolute address of an area-status bit. `area` is 1-based.
    pub const fn area_addr(&self, area: u8, off: u16) -> u16 {
        self.areas.base + (area as u16 - 1) * self.areas.stride + off
    }
    pub const fn addr_unscharf(&self, area: u8) -> u16 {
        self.area_addr(area, self.areas.off_unscharf)
    }
    pub const fn addr_intern_scharf(&self, area: u8) -> u16 {
        self.area_addr(area, self.areas.off_intern_scharf)
    }
    pub const fn addr_extern_scharf(&self, area: u8) -> u16 {
        self.area_addr(area, self.areas.off_extern_scharf)
    }
    pub const fn addr_alarm(&self, area: u8) -> u16 {
        self.area_addr(area, self.areas.off_alarm)
    }
    pub const fn addr_intern_bereit(&self, area: u8) -> u16 {
        self.area_addr(area, self.areas.off_intern_bereit)
    }
    pub const fn addr_extern_bereit(&self, area: u8) -> u16 {
        self.area_addr(area, self.areas.off_extern_bereit)
    }
    /// Address of "detection area `mb` bypassed" (`mb` 1-based, ≤ `mb_max`).
    pub const fn addr_mb_gesperrt(&self, mb: u16) -> u16 {
        self.mb_gesperrt_base + (mb - 1)
    }
    /// Defense-in-depth for the output path: inside the output window AND outside the
    /// system-status block (mirrors `telenot_config::is_switchable_addr` for complex).
    pub fn is_switchable_addr(&self, addr: u16) -> bool {
        self.output_addr_range.contains(&addr) && !self.status_addr_range.contains(&addr)
    }
}

/// Profile for a config-level panel selection.
pub fn from_config_kind(kind: telenot_config::PanelKind) -> &'static PanelProfile {
    match kind {
        telenot_config::PanelKind::Complex400 => &COMPLEX400,
        telenot_config::PanelKind::Hiplex8400 => &HIPLEX8400,
    }
}

/// Telenot complex 400H — every value equals the historical, pcap-verified literal.
pub const COMPLEX400: PanelProfile = PanelProfile {
    kind: PanelKind::Complex400,
    areas: AreaLayout {
        base: 0x0530,
        stride: 8,
        count: 1,
        off_unscharf: 0,
        off_intern_scharf: 1,
        off_extern_scharf: 2,
        off_alarm: 3,
        off_intern_bereit: 5,
        off_extern_bereit: 6,
    },
    mb_status_base: 0x0570,
    mb_gesperrt_base: 0x05F0,
    mb_max: 128,
    schaltaktion_base: None,
    schaltaktion_max: 0,
    output_addr_range: 0x0500..0x0780,
    status_addr_range: 0x0530..0x0670,
};

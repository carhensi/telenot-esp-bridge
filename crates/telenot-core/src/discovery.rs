//! Auto-Discovery aggregator: builds an unconfirmed config from occupancy-status responses
//! (0x24/0x71+0x72) and text queries (0x0C+0x54) from the panel.
//!
//! Verified against real panel captures:
//! - **`'0'`-bit = occupied** (both inputs 0x71 and outputs 0x72).
//! - A text query for an occupied address returns 0x0C (area) + 0x54 (plain-text name);
//!   an unoccupied address does not respond at all (→ only query occupied ones).

use std::collections::{BTreeMap, BTreeSet};
use telenot_config::{
    Config, GmsVariant, PanelSettings, Polarity, Sensor, SensorKind, CURRENT_SCHEMA_VERSION,
    MAX_SENSORS,
};
use telenot_protocol::BlockStatus;

use crate::profile::{PanelKind, PanelProfile};

#[derive(Default)]
pub struct Discovery {
    occupied: BTreeSet<u16>,
    got_inputs: bool,
    got_outputs: bool,
    names: BTreeMap<u16, String>,
}

impl Discovery {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest a 0x24 occupancy-status response (`'0'` = occupied).
    pub fn ingest_belegt(&mut self, bs: &BlockStatus) {
        match bs.adresserweiterung {
            0x71 => self.got_inputs = true,
            0x72 => self.got_outputs = true,
            _ => return,
        }
        let base = bs.base_address();
        for (i, byte) in bs.status.iter().enumerate() {
            for bit in 0..8u16 {
                if (byte >> bit) & 1 == 0 {
                    // saturating: `base` comes off the wire and a corrupted/noisy frame
                    // could push it near 0xFFFF — must not overflow-panic on scan input.
                    let addr = base.saturating_add(i as u16 * 8 + bit);
                    // Cap at the device's own sensor limit: `into_config`/`into_config_for`
                    // build every sensor from `occupied`, and their `.expect()` on
                    // `SensorTable::from_sensors` assumes this can never exceed it. Idempotent
                    // re-insertion of already-known addresses (repeated occupancy telegrams)
                    // stays allowed even once the cap is hit.
                    if self.occupied.len() < MAX_SENSORS || self.occupied.contains(&addr) {
                        self.occupied.insert(addr);
                    }
                }
            }
        }
    }

    /// Both occupancy telegrams (inputs + outputs) received?
    pub fn belegt_complete(&self) -> bool {
        self.got_inputs && self.got_outputs
    }

    /// List of occupied addresses (the detection points to query).
    pub fn occupied(&self) -> Vec<u16> {
        self.occupied.iter().copied().collect()
    }

    /// Ingest a text-query response (address + plain-text name).
    pub fn ingest_text(&mut self, addr: u16, name: &str) {
        let n = name.trim();
        if !n.is_empty() {
            self.names.insert(addr, n.to_string());
        }
    }

    /// Number of detection points named so far (successfully queried).
    pub fn named_count(&self) -> usize {
        self.names.len()
    }

    /// Raw names (as read from the panel) per occupied address. Used in the setup view so the
    /// original name stays visible as `raw_name` while `name` is editable — the persistent
    /// config intentionally does NOT store this value.
    pub fn raw_names(&self) -> &BTreeMap<u16, String> {
        &self.names
    }

    /// Builds the (unconfirmed) config from occupied addresses + names. Only named detection
    /// points are included; type is guessed heuristically and must be confirmed in the setup UI.
    pub fn into_config(&self) -> Config {
        // Topics must be unique (otherwise two detectors publish to the same MQTT topic);
        // duplicate names (e.g. 2× "ESG Gehäuse") get the address appended.
        let mut seen_topics = std::collections::BTreeSet::new();
        let sensors = self
            .occupied
            .iter()
            .filter_map(|&addr| {
                let name = self.names.get(&addr)?;
                let mut topic = slug(name, addr);
                if !seen_topics.insert(topic.clone()) {
                    topic = format!("{topic}_{addr:04x}");
                    seen_topics.insert(topic.clone());
                }
                Some(Sensor {
                    address: addr,
                    name: name.clone(),
                    name_ha: name.clone(),
                    kind: classify(addr, name),
                    topic,
                    // Polarity matches the verified panel convention; confirmation in setup.
                    polarity: Polarity::ActiveLow,
                    confirmed: false,
                    // Enabling switch control is a deliberate user decision in setup, never in Discovery.
                    switchable: false,
                    show_in_homekit: false,
                })
            })
            .collect::<Vec<Sensor>>();
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            // The scan address space is limited to a few hundred addresses and can never
            // reach the table's device limits.
            sensors: telenot_config::SensorTable::from_sensors(&sensors)
                .expect("scan inventory exceeds device limits"),
            panel: Default::default(),
        }
    }

    /// Profile-aware variant of [`Self::into_config`]. For **hiplex GMS lite** it
    /// additionally synthesizes Meldebereich sensors: lite carries no Klartexte, so an
    /// occupied MB-status address never gets a 0x54 name and would be dropped by the
    /// name filter. Those get a generic `MB {n}` name (kind `Unbekannt`); the user
    /// renames/classifies them in the sensor setup step — that classification is also
    /// what makes an MB eligible for HomeKit mirroring later. The panel settings are
    /// carried into the result so a discovery commit cannot reset the panel selection.
    pub fn into_config_for(&self, profile: &PanelProfile, panel: &PanelSettings) -> Config {
        let mut cfg = self.into_config();
        cfg.panel = panel.clone();
        if profile.kind == PanelKind::Hiplex8400 && panel.gms_variant == GmsVariant::Lite {
            let mb_range = profile.mb_status_base..profile.mb_status_base + profile.mb_max;
            let known: BTreeSet<u16> = cfg.sensors.iter().map(|s| s.address()).collect();
            for &addr in self
                .occupied
                .iter()
                .filter(|a| mb_range.contains(a) && !known.contains(a))
            {
                let n = addr - profile.mb_status_base + 1;
                cfg.sensors
                    .push(&Sensor {
                        address: addr,
                        name: format!("MB {n}"),
                        name_ha: format!("MB {n}"),
                        kind: SensorKind::Unbekannt,
                        topic: format!("mb_{n}"),
                        polarity: Polarity::ActiveLow,
                        confirmed: false,
                        switchable: false,
                        show_in_homekit: false,
                    })
                    // The scan address space is limited to a few hundred addresses and can
                    // never reach the table's device limits.
                    .expect("scan inventory exceeds device limits");
            }
        }
        cfg
    }
}

/// Area/detection-area/bypass status addresses are system-status; otherwise name heuristic.
fn classify(addr: u16, name: &str) -> SensorKind {
    if telenot_config::STATUS_ADDR_RANGE.contains(&addr) {
        SensorKind::Systemstatus
    } else {
        SensorKind::guess_from_name(name)
    }
}

/// Converts a plain-text name into an MQTT-safe topic segment.
fn slug(name: &str, addr: u16) -> String {
    let mut s = String::new();
    let mut last_us = false;
    for c in name.to_lowercase().chars() {
        match c {
            'ä' | 'ö' | 'ü' | 'ß' => {
                s.push_str(match c {
                    'ä' => "ae",
                    'ö' => "oe",
                    'ü' => "ue",
                    _ => "ss",
                });
                last_us = false;
            }
            c if c.is_ascii_alphanumeric() => {
                s.push(c);
                last_us = false;
            }
            _ => {
                if !last_us {
                    s.push('_');
                    last_us = true;
                }
            }
        }
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        format!("addr_{addr:04x}")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn belegt(erw: u8, base_hi: u8, base_lo: u8, status: &[u8]) -> BlockStatus<'_> {
        BlockStatus {
            geraet_bereich: 0,
            adresse: base_hi,
            adressenzusatz: base_lo,
            adresserweiterung: erw,
            status,
        }
    }

    #[test]
    fn zero_bit_means_occupied() {
        let mut d = Discovery::new();
        // Inputs from 0x0000: byte 0 = 0xFE → bit0(0x0000)=0 occupied, bits 1..7 free.
        d.ingest_belegt(&belegt(0x71, 0x00, 0x00, &[0xFE]));
        assert!(d.occupied().contains(&0x0000));
        assert!(!d.occupied().contains(&0x0001));
        assert!(!d.belegt_complete(), "outputs still missing");
        d.ingest_belegt(&belegt(0x72, 0x05, 0x00, &[0xFF]));
        assert!(d.belegt_complete());
    }

    #[test]
    fn builds_config_from_names() {
        let mut d = Discovery::new();
        // 0x0075 occupied (input), 0x0530 occupied (output/area status).
        // 0x0075 = base 0x0000 + offset 117 → byte 14 bit 5.
        let mut inp = [0xFFu8; 16];
        inp[14] &= !(1 << 5); // bit for 0x0075 → 0 = occupied
        d.ingest_belegt(&belegt(0x71, 0x00, 0x00, &inp));
        // 0x0530 = base 0x0500 + 0x30 → byte 6 bit 0
        let mut out = [0xFFu8; 8];
        out[6] &= !1;
        d.ingest_belegt(&belegt(0x72, 0x05, 0x00, &out));

        d.ingest_text(0x0075, "IM Essen        EG");
        d.ingest_text(0x0530, "Haus:");

        let cfg = d.into_config();
        let s75 = cfg.sensors.iter().find(|s| s.address() == 0x0075).unwrap();
        assert_eq!(s75.kind(), SensorKind::Bewegungsmelder, "'IM ' → Bewegung");
        assert_eq!(s75.topic(), "im_essen_eg");
        assert!(!s75.confirmed(), "discovery -> unconfirmed");

        let s530 = cfg.sensors.iter().find(|s| s.address() == 0x0530).unwrap();
        assert_eq!(
            s530.kind(),
            SensorKind::Systemstatus,
            "Bereichsstatus-Range"
        );

        // Occupied but unnamed addresses are not included in the config.
        assert_eq!(cfg.sensors.len(), 2);
    }

    #[test]
    fn slug_transliterates_umlauts_and_dedupes_topics() {
        let mut d = Discovery::new();
        // 0x0000 + 0x0001 occupied: byte 0 = 0xFC (bit0+bit1 = 0).
        d.ingest_belegt(&belegt(0x71, 0x00, 0x00, &[0xFC]));
        d.ingest_belegt(&belegt(0x72, 0x05, 0x00, &[0xFF]));

        // Same name on two addresses (real: 2× "ESG Gehäuse" at 0x0000/0x0001).
        d.ingest_text(0x0000, "ESG Gehäuse");
        d.ingest_text(0x0001, "ESG Gehäuse");

        let cfg = d.into_config();
        let t0 = cfg
            .sensors
            .iter()
            .find(|s| s.address() == 0x0000)
            .unwrap()
            .topic();
        let t1 = cfg
            .sensors
            .iter()
            .find(|s| s.address() == 0x0001)
            .unwrap()
            .topic();
        assert_eq!(
            t0, "esg_gehaeuse",
            "Umlaute transliteriert statt verschluckt"
        );
        assert_eq!(t1, "esg_gehaeuse_0001", "duplicate gets address suffix");
    }

    #[test]
    fn ingest_belegt_saturates_instead_of_overflowing() {
        let mut d = Discovery::new();
        // base = 0xFFF8; second status byte's bit0 → addr = base + 8 = 0x10000, which
        // overflows u16. A corrupted/noisy frame can push `base` this high — must
        // saturate to 0xFFFF instead of panicking (overflow-checks builds) or wrapping.
        d.ingest_belegt(&belegt(0x71, 0xFF, 0xF8, &[0xFF, 0xFE]));
        assert!(d.occupied().contains(&0xFFFF));
    }

    #[test]
    fn occupied_set_is_capped_at_max_sensors() {
        let mut d = Discovery::new();
        // 100 blocks × 8 fully-occupied bits = 800 distinct addresses, well beyond
        // MAX_SENSORS(600) — a noisy/corrupted serial stream could report this many.
        for block in 0..100u16 {
            let base = block * 8;
            d.ingest_belegt(&belegt(
                0x71,
                (base >> 8) as u8,
                (base & 0xFF) as u8,
                &[0x00],
            ));
        }
        assert_eq!(d.occupied().len(), MAX_SENSORS);
        // Building a config from an over-full occupied set must not panic (the
        // `.expect()` in `into_config`/`into_config_for` assumes this can't happen).
        let cfg = d.into_config();
        assert!(cfg.sensors.len() <= MAX_SENSORS);
    }
}

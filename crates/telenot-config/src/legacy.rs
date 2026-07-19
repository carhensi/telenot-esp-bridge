//! One-shot import of the old Node.js config (group-relative hex addresses) into the
//! new schema with **canonical GMS 16-bit addresses**.
//!
//! The address mapping is verified against the real pcap capture (2026-06-02),
//! NOT guessed:
//! - `systemstatus` lives in the output/security area status block → canonical = `hex + 0x04F0`
//!   (verified: 0x40 → 0x0530 disarmed, 0x42 → 0x0532 external arm).
//! - all other sensors are inputs → canonical = `hex` directly
//!   (verified: 0x075 → 0x0075 "im_essen", toggled on window open/close).
//! - the 2 `sicherung` special cases (Sicherung1/Home, hex 0x0) are ambiguous →
//!   `confirmed = false`, must be confirmed in the setup web UI.

use crate::{Config, Issue, IssueCode, Polarity, Sensor, SensorKind, CURRENT_SCHEMA_VERSION};
use serde_json::Value;

/// Delta from the old output hex to the canonical security area status address (from 0x0530).
const BEREICHSSTATUS_DELTA: u16 = 0x04F0;
const LEGACY_TOPIC_PREFIX: &str = "telenot/alarm/";

/// Result of a legacy import: migrated config + findings (warnings/errors).
#[derive(Debug)]
pub struct LegacyImport {
    pub config: Config,
    pub issues: Vec<Issue>,
}

impl Config {
    /// Imports the old nested location/group config (JSON) into the new schema.
    pub fn import_legacy(json: &[u8]) -> Result<LegacyImport, crate::ConfigError> {
        let root: Value = serde_json::from_slice(json).map_err(crate::ConfigError::Json)?;
        let mut sensors = Vec::new();
        let mut issues = Vec::new();

        let Some(locations) = root.as_object() else {
            issues.push(Issue::error(
                IssueCode::LegacyMalformed,
                "Top-Level ist kein Objekt".into(),
            ));
            return Ok(LegacyImport {
                config: Config::default(),
                issues,
            });
        };

        for (loc_name, loc_val) in locations {
            let Some(groups) = loc_val.as_object() else {
                issues.push(Issue::warning(
                    IssueCode::LegacySkipped,
                    format!("Location {loc_name}: kein Objekt, übersprungen"),
                ));
                continue;
            };
            for (group_name, arr) in groups {
                let Some(list) = arr.as_array() else { continue };
                for entry in list {
                    match import_sensor(entry) {
                        Ok(s) => sensors.push(s),
                        Err(msg) => issues.push(Issue::warning(
                            IssueCode::LegacyEntry,
                            format!("{loc_name}/{group_name}: {msg}"),
                        )),
                    }
                }
            }
        }

        // Flag special cases: unconfirmed sensors require setup-web confirmation.
        for s in sensors.iter().filter(|s| !s.confirmed) {
            issues.push(Issue::warning(
                IssueCode::SensorUnconfirmed,
                format!(
                    "Sensor '{}' (0x{:04X}) unbestätigt — Adresse/Polarität im Setup bestätigen",
                    s.name, s.address
                ),
            ));
        }

        Ok(LegacyImport {
            config: Config {
                schema_version: CURRENT_SCHEMA_VERSION,
                sensors: crate::SensorTable::from_sensors(&sensors)?,
                panel: Default::default(),
            },
            issues,
        })
    }
}

fn import_sensor(v: &Value) -> Result<Sensor, String> {
    let get = |k: &str| v.get(k).and_then(Value::as_str);
    let hex_str = get("hex").ok_or("kein hex-Feld")?;
    let raw_hex = parse_hex(hex_str).ok_or_else(|| format!("ungültiges hex '{hex_str}'"))?;
    let kind = SensorKind::from_legacy_type(get("type").unwrap_or(""));
    let (address, confirmed) = canonical_address(kind, raw_hex);
    let inverted = v.get("inverted").and_then(Value::as_bool).unwrap_or(false);

    Ok(Sensor {
        address,
        name: get("name").unwrap_or("").to_string(),
        name_ha: get("name_ha")
            .unwrap_or_else(|| get("name").unwrap_or(""))
            .to_string(),
        kind,
        topic: strip_topic_prefix(get("topic").unwrap_or("")),
        location: get("location").unwrap_or("").to_string(),
        // Legacy `inverted=true` corresponds to the protocol polarity ('0' = active) = ActiveLow.
        polarity: if inverted {
            Polarity::ActiveLow
        } else {
            Polarity::ActiveHigh
        },
        confirmed,
        // Switchability is a NEW, deliberate decision — never derive it from legacy configs.
        switchable: false,
        show_in_homekit: false,
    })
}

/// Verified mapping of legacy-relative hex → canonical GMS address. Returns (address, confirmed).
fn canonical_address(kind: SensorKind, hex: u16) -> (u16, bool) {
    match kind {
        // Security area status in the output block.
        SensorKind::Systemstatus => (hex.wrapping_add(BEREICHSSTATUS_DELTA), true),
        // Sicherung1/Home: ambiguous → unconfirmed.
        SensorKind::Sicherung => (hex.wrapping_add(BEREICHSSTATUS_DELTA), false),
        // All inputs: canonical address == legacy hex.
        _ => (hex, true),
    }
}

fn parse_hex(s: &str) -> Option<u16> {
    let s = s.trim();
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u16::from_str_radix(s, 16).ok()
}

fn strip_topic_prefix(topic: &str) -> String {
    topic
        .strip_prefix(LEGACY_TOPIC_PREFIX)
        .unwrap_or(topic)
        .to_string()
}

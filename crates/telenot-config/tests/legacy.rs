//! Import of the old nested location/group config into the new schema.
//!
//! The fixture is **synthetic** (generic names) but covers all mapping branches.
//! The address mapping is verified against the real pcap capture (see `telenot-core`):
//! inputs = hex directly (0x075→0x0075), systemstatus = hex+0x04F0 (0x40→0x0530), the
//! `ema.meldebereiche` group is mixed → distinguish by type, not by group.

use telenot_config::{Config, Polarity, SensorKind, SensorRef};

const LEGACY: &[u8] = include_bytes!("fixtures/legacy-config.json");

fn find<'a>(cfg: &'a Config, name: &str) -> SensorRef<'a> {
    cfg.sensors
        .iter()
        .find(|s| s.name() == name)
        .unwrap_or_else(|| panic!("Sensor '{name}' nicht gefunden"))
}

#[test]
fn imports_all_sensors() {
    let imp = Config::import_legacy(LEGACY).unwrap();
    assert_eq!(imp.config.sensors.len(), 6);
}

#[test]
fn verified_address_mapping() {
    let cfg = &Config::import_legacy(LEGACY).unwrap().config;

    // systemstatus: hex + 0x04F0  (verified via security area status diff)
    assert_eq!(find(cfg, "unscharf").address(), 0x0530);
    assert_eq!(find(cfg, "extern_scharf").address(), 0x0532);
    assert_eq!(find(cfg, "unscharf").kind(), SensorKind::Systemstatus);

    // Input sensor: hex directly (verified via window toggle 0x075 -> 0x0075)
    assert_eq!(find(cfg, "motion_a").address(), 0x0075);
    assert_eq!(find(cfg, "motion_a").kind(), SensorKind::Bewegungsmelder);
    assert_eq!(find(cfg, "window_b").address(), 0x0088);

    // mixed detection-area group: one sensor entry remains an input (hex directly).
    assert_eq!(find(cfg, "contact_mixed").address(), 0x0085);
    assert_eq!(find(cfg, "contact_mixed").kind(), SensorKind::Magnetkontakt);
}

#[test]
fn legacy_inverted_maps_to_protocol_polarity() {
    let cfg = &Config::import_legacy(LEGACY).unwrap().config;
    // inverted:true -> ActiveLow ('0' = active, protocol default)
    assert_eq!(find(cfg, "unscharf").polarity(), Polarity::ActiveLow);
    assert_eq!(find(cfg, "motion_a").polarity(), Polarity::ActiveLow);
}

#[test]
fn sicherung_specials_are_unconfirmed() {
    let imp = Config::import_legacy(LEGACY).unwrap();
    let s = find(&imp.config, "sb_main");
    assert_eq!(s.kind(), SensorKind::Sicherung);
    assert!(!s.confirmed(), "imported sensor must be unconfirmed");
    assert!(imp.issues.iter().any(|i| i.message.contains("sb_main")));
}

#[test]
fn topic_prefix_is_stripped() {
    let cfg = &Config::import_legacy(LEGACY).unwrap().config;
    let s = find(cfg, "motion_a");
    assert!(
        !s.topic().starts_with("telenot/alarm/"),
        "Prefix entfernt: {}",
        s.topic()
    );
    assert!(
        s.topic().starts_with("floor/"),
        "relativer Pfad: {}",
        s.topic()
    );
}

#[test]
fn imported_config_round_trips_through_json() {
    let imp = Config::import_legacy(LEGACY).unwrap();
    let json = imp.config.to_json().unwrap();
    let reparsed = Config::from_json(json.as_bytes()).unwrap();
    assert_eq!(imp.config, reparsed);
}

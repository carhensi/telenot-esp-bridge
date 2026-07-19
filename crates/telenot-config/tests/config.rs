use telenot_config::{
    Config, ConfigError, Polarity, Sensor, SensorKind, SensorTable, Severity,
    CURRENT_SCHEMA_VERSION,
};

fn sensor(address: u16, name: &str, kind: SensorKind, confirmed: bool) -> Sensor {
    Sensor {
        address,
        name: name.into(),
        name_ha: name.into(),
        kind,
        topic: format!("eg/{name}"),
        location: "EG".into(),
        polarity: Polarity::ActiveLow,
        confirmed,
        switchable: false,
        show_in_homekit: false,
    }
}

#[test]
fn json_round_trip_is_lossless() {
    let cfg = Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        sensors: SensorTable::from_sensors(&[
            sensor(0x0532, "extern_scharf", SensorKind::Systemstatus, true),
            sensor(0x0075, "im_essen", SensorKind::Bewegungsmelder, true),
        ])
        .unwrap(),
        panel: Default::default(),
    };
    let json = cfg.to_json().unwrap();
    let parsed = Config::from_json(json.as_bytes()).unwrap();
    assert_eq!(cfg, parsed);
}

#[test]
fn full_inventory_round_trip_stays_compact() {
    // Regression for the ESP32 OOM panic while persisting ~147 sensors: to_json must
    // emit compact JSON into a pre-sized buffer, and from_json must take the direct
    // slice path on the current schema. This pins the serialized size well below the
    // 64 KB firmware load limit (CONFIG_BUF) with realistic name/topic lengths.
    let sensors: Vec<Sensor> = (0..150)
        .map(|i| {
            sensor(
                0x0100 + i,
                &format!("Bewegungsmelder Obergeschoss Flur {i:03}"),
                SensorKind::Bewegungsmelder,
                true,
            )
        })
        .collect();
    let cfg = Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        sensors: SensorTable::from_sensors(&sensors).unwrap(),
        panel: Default::default(),
    };
    let json = cfg.to_json().unwrap();
    assert!(
        json.len() < 48 * 1024,
        "Config-JSON unerwartet groß: {} B",
        json.len()
    );
    assert!(!json.contains("\n  "), "kompakt, nicht pretty-printed");
    let parsed = Config::from_json(json.as_bytes()).unwrap();
    assert_eq!(cfg, parsed);
}

#[test]
fn sensor_count_above_device_limit_is_a_validation_error() {
    // Commit/import must reject oversized inventories with a clear message instead of
    // letting the device run into its heap/NVS budget (OOM panic without this guard).
    let sensors: Vec<Sensor> = (0..=telenot_config::MAX_SENSORS as u16)
        .map(|i| {
            sensor(
                0x0100 + i,
                &format!("m{i}"),
                SensorKind::Magnetkontakt,
                true,
            )
        })
        .collect();
    let cfg = Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        sensors: SensorTable::from_sensors(&sensors).unwrap(),
        panel: Default::default(),
    };
    let issues = cfg.validate();
    assert!(issues
        .iter()
        .any(|i| i.code.as_str() == "too_many_sensors" && i.severity == Severity::Error));
}

#[test]
fn additive_fields_default_when_absent() {
    // Minimal sensor without polarity/confirmed → serde defaults apply.
    let json = r#"{
        "schema_version": 1,
        "sensors": [
            {"address": 1, "name": "mg2", "name_ha": "MG2",
             "kind": "magnetkontakt", "topic": "eg/x", "location": "EG"}
        ]
    }"#;
    let cfg = Config::from_json(json.as_bytes()).unwrap();
    assert_eq!(cfg.sensors.get(0).unwrap().polarity(), Polarity::ActiveLow);
    assert!(!cfg.sensors.get(0).unwrap().confirmed());
}

#[test]
fn future_schema_version_is_rejected_not_destroyed() {
    let json = format!(
        r#"{{"schema_version": {}, "sensors": []}}"#,
        CURRENT_SCHEMA_VERSION + 5
    );
    match Config::from_json(json.as_bytes()) {
        Err(ConfigError::UnsupportedVersion(v)) => assert_eq!(v, CURRENT_SCHEMA_VERSION + 5),
        other => panic!("erwartete UnsupportedVersion, bekam {other:?}"),
    }
}

#[test]
fn missing_schema_version_errors() {
    let json = r#"{"sensors": []}"#;
    assert!(matches!(
        Config::from_json(json.as_bytes()),
        Err(ConfigError::MissingVersion)
    ));
}

#[test]
fn older_schema_version_migrates_to_current() {
    let json = r#"{"schema_version": 0, "sensors": []}"#;
    let cfg = Config::from_json(json.as_bytes()).unwrap();
    assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
}

#[test]
fn type_heuristic_matches_real_names() {
    use SensorKind::*;
    assert_eq!(
        SensorKind::guess_from_name("Magnetkontakt Fenster (Links)"),
        Magnetkontakt
    );
    assert_eq!(SensorKind::guess_from_name("IM Essen"), Bewegungsmelder);
    assert_eq!(
        SensorKind::guess_from_name("Bewegungsmelder"),
        Bewegungsmelder
    );
    assert_eq!(SensorKind::guess_from_name("Haustür"), Schliesskontakt);
    assert_eq!(SensorKind::guess_from_name("Rauchmelder Flur"), Rauchmelder);
    assert_eq!(
        SensorKind::guess_from_name("Wassermelder Keller"),
        Wassermelder
    );
    assert_eq!(
        SensorKind::guess_from_name("System Extern Scharf"),
        Systemstatus
    );
    assert_eq!(
        SensorKind::guess_from_name("Irgendwas Komisches"),
        Unbekannt
    );
}

#[test]
fn heuristic_matches_installer_dump() {
    // Verified against the real installer scan (147 sensors).
    use SensorKind::*;
    let g = SensorKind::guess_from_name;

    // Abbreviation fallback: RM/IM/BM at the start of the name (the actual fix — previously Unbekannt).
    assert_eq!(g("RM Flur         OG"), Rauchmelder);
    assert_eq!(g("RM Kamin        EG"), Rauchmelder);
    assert_eq!(g("IM Essen        EG"), Bewegungsmelder);

    // Abbreviation beats word (real installer config): "MK Tür" → Magnetkontakt (opening contact),
    // "SK Tür" → Schliesskontakt (bolt). Both are on the same door.
    assert_eq!(g("MK Fenster EssenEG"), Magnetkontakt);
    assert_eq!(g("MK Tür DachbodenDG"), Magnetkontakt);
    assert_eq!(g("MK Tuer Garage"), Magnetkontakt);
    assert_eq!(g("SK Tuer Garage"), Schliesskontakt);
    // Without MK/SK prefix the word decides: bare "Haustür" → Schliesskontakt.
    assert_eq!(g("Haustür"), Schliesskontakt);

    // Signal sounder: named, via abbreviation (ESG/ISG standalone) — but Gehaeuse wins as a word.
    assert_eq!(g("Akustisch. SG 1"), Signalgeber);
    assert_eq!(g("Optischer SG"), Signalgeber);
    assert_eq!(g("ESG vorne"), Signalgeber);
    assert_eq!(g("ISG"), Signalgeber);
    assert_eq!(g("ESG Gehäuse"), Gehaeuse);

    // Diagnostic/bus: faults, comlock access, keypads.
    assert_eq!(g("Akku-Stoerung"), Batterie);
    assert_eq!(g("Netz-Stoerung"), Netzstoerung);
    assert_eq!(g("MA - comlock 1"), Gebaeudetechnik);
    assert_eq!(g("CL410-1 Garage"), Gebaeudetechnik);
    assert_eq!(g("CL0-DK"), Gebaeudetechnik);
    assert_eq!(g("Service-Bedient."), Bedienteil);
    assert_eq!(g("BT-Adr.2 - EG"), Bedienteil);
    // BuildSec = Telenot app (remote keypad), structurally a BT slot → Bedienteil.
    assert_eq!(g("BuildSec-Adr.15"), Bedienteil);

    // Token-EXACT: "RM" must NOT falsely match as a substring inside "Warmwasser".
    assert_eq!(g("Warmwasser Keller"), Wassermelder);

    // Generic bus slots detected (default hidden); installer-named entries remain visible.
    assert!(telenot_config::is_generic_bus_label("BT-Adr.1 - Z"));
    assert!(telenot_config::is_generic_bus_label("BuildSec-Adr.15"));
    assert!(!telenot_config::is_generic_bus_label("Service-Bedient."));
    assert!(!telenot_config::is_generic_bus_label("CL410-1 Garage"));
}

#[test]
fn device_class_mapping() {
    assert_eq!(SensorKind::Bewegungsmelder.device_class(), "motion");
    assert_eq!(SensorKind::Magnetkontakt.device_class(), "window");
    assert_eq!(SensorKind::Schliesskontakt.device_class(), "door");
    assert_eq!(SensorKind::Sabotage.device_class(), "tamper");
}

#[test]
fn diagnostic_kinds_match_gms_and_ha() {
    // GMS message type → standard HA binary_sensor device_class
    assert_eq!(SensorKind::Batterie.device_class(), "battery"); // Akku 0x33
    assert_eq!(SensorKind::Netzstoerung.device_class(), "power"); // Netz 0x32
    assert_eq!(SensorKind::Uebertragung.device_class(), "connectivity"); // transmission device 0x34
    assert_eq!(SensorKind::Stoerung.device_class(), "problem"); // fault 0x30
                                                                // Heuristic verified against real panel texts (from the discovery run)
    use SensorKind::*;
    assert_eq!(SensorKind::guess_from_name("Akku-Stoerung"), Batterie);
    assert_eq!(SensorKind::guess_from_name("Netz-Stoerung"), Netzstoerung);
    assert_eq!(
        SensorKind::guess_from_name("Stoerung Übertragungsgeraet"),
        Uebertragung
    );
}

#[test]
fn polarity_semantics() {
    // Spec: bit '0' (raw_bit=false) = active.
    assert_eq!(Polarity::ActiveLow.is_active(false), Some(true));
    assert_eq!(Polarity::ActiveLow.is_active(true), Some(false));
    assert_eq!(Polarity::ActiveHigh.is_active(true), Some(true));
    // Unconfirmed (discovery) → fail-safe unknown.
    assert_eq!(Polarity::Unconfirmed.is_active(false), None);
}

#[test]
fn validation_flags_issues() {
    let mut cfg = Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        sensors: SensorTable::from_sensors(&[
            sensor(0x0001, "a", SensorKind::Magnetkontakt, true),
            sensor(0x0001, "b", SensorKind::Magnetkontakt, true), // doppelte Adresse
            sensor(0x0002, "c", SensorKind::Bewegungsmelder, false), // unconfirmed
        ])
        .unwrap(),
        panel: Default::default(),
    };
    cfg.sensors.get_mut(0).unwrap().set_topic("  ").unwrap(); // leeres Topic

    let issues = cfg.validate();
    assert!(issues
        .iter()
        .any(|i| i.severity == Severity::Error && i.message.contains("Topic")));
    assert!(issues
        .iter()
        .any(|i| i.severity == Severity::Warning && i.message.contains("unbestätigt")));
    assert!(issues
        .iter()
        .any(|i| i.severity == Severity::Warning && i.message.contains("mehrfach")));
}

#[test]
fn export_never_contains_a_pin() {
    // The PIN is deliberately not part of the schema (it lives in NVS) — an export must never contain it.
    let cfg = Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        sensors: SensorTable::from_sensors(&[sensor(
            0x0532,
            "extern_scharf",
            SensorKind::Systemstatus,
            true,
        )])
        .unwrap(),
        panel: Default::default(),
    };
    let json = cfg.to_json().unwrap().to_lowercase();
    assert!(!json.contains("pin"));
    assert!(!json.contains("passwo"));
}

#[test]
fn switchable_validation() {
    // switchable on an input address → error; on a signal sounder output → warning.
    let mut bad = sensor(0x0075, "im_essen", SensorKind::Bewegungsmelder, true);
    bad.switchable = true;
    let mut siren = sensor(0x050C, "asg1", SensorKind::Signalgeber, true);
    siren.switchable = true;
    let mut ok = sensor(0x0515, "relais", SensorKind::Unbekannt, true);
    ok.switchable = true;
    // System status block (arm/bypass addresses) must NEVER be switchable.
    let mut status = sensor(0x0530, "unscharf_b1", SensorKind::Systemstatus, true);
    status.switchable = true;
    let cfg = Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        sensors: SensorTable::from_sensors(&[bad, siren, ok, status]).unwrap(),
        panel: Default::default(),
    };
    let issues = cfg.validate();
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.severity == Severity::Error
                && i.code == telenot_config::IssueCode::SwitchableNotOutput)
            .count(),
        2,
        "Eingang UND Status-Block-Adresse müssen als Fehler markiert sein"
    );
    assert!(issues.iter().any(|i| i.severity == Severity::Warning
        && i.code == telenot_config::IssueCode::SwitchableSignalgeber));
    // The regular output (0x0515) produces NO switchable finding:
    // 2× NotOutput (input + status-block) + 1× Signalgeber warning.
    assert_eq!(
        issues
            .iter()
            .filter(|i| matches!(
                i.code,
                telenot_config::IssueCode::SwitchableNotOutput
                    | telenot_config::IssueCode::SwitchableSignalgeber
            ))
            .count(),
        3
    );
}

#[test]
fn config_without_panel_defaults_to_complex400() {
    // Additive schema: configs written before the panel field existed must load with
    // complex-400H defaults — bit-identical behavior, no migration.
    let json = br#"{"schema_version":1,"sensors":[]}"#;
    let cfg = Config::from_json(json).unwrap();
    assert_eq!(cfg.panel.kind, telenot_config::PanelKind::Complex400);
    assert_eq!(cfg.panel.gms_variant, telenot_config::GmsVariant::Lite);
    assert!(!cfg.panel.hiplex_cmds_verified);
    assert!(cfg.panel.areas.is_empty());
    // And it round-trips (panel now serialized explicitly).
    let out = cfg.to_json().unwrap();
    let back = Config::from_json(out.as_bytes()).unwrap();
    assert_eq!(cfg, back);
}

#[test]
fn panel_validation_flags_bad_area_ids_and_ignored_flag() {
    use telenot_config::{AreaCfg, IssueCode, PanelKind};
    let mut cfg = Config::default();
    cfg.panel.areas = vec![
        AreaCfg {
            id: 0,
            name: "x".into(),
        },
        AreaCfg {
            id: 17,
            name: "y".into(),
        },
        AreaCfg {
            id: 3,
            name: "a".into(),
        },
        AreaCfg {
            id: 3,
            name: "b".into(),
        },
    ];
    cfg.panel.hiplex_cmds_verified = true; // on complex → warning
    cfg.panel.kind = PanelKind::Complex400;
    let issues = cfg.validate();
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.code == IssueCode::AreaIdInvalid)
            .count(),
        3,
        "0, 17 und das Duplikat müssen markiert sein"
    );
    assert!(issues.iter().any(|i| i.code == IssueCode::PanelFlagIgnored));
}

//! SensorTable contract tests: JSON byte-identity with the `Vec<Sensor>` derive,
//! roundtrips at target scale, mutation semantics and memory bounds.

use telenot_config::{Polarity, Sensor, SensorKind, SensorTable};

fn sensor(address: u16, name: &str) -> Sensor {
    Sensor {
        address,
        name: name.into(),
        name_ha: format!("{name} (HA)"),
        kind: SensorKind::Magnetkontakt,
        topic: format!("eg/{}", name.to_lowercase().replace(' ', "_")),
        polarity: Polarity::ActiveLow,
        confirmed: true,
        switchable: false,
        show_in_homekit: false,
    }
}

/// Tricky content: umlauts, quotes, backslash, emoji, empty strings — the
/// serialized form must match the derive byte for byte.
fn tricky_sensors() -> Vec<Sensor> {
    vec![
        sensor(0x0001, "ESG Gehäuse \"vorn\""),
        sensor(0x0042, "Tür \\ Süd / <ÖL>"),
        Sensor {
            address: 0x0533,
            name: String::new(),
            name_ha: "🚨 Alarm".into(),
            kind: SensorKind::Systemstatus,
            topic: String::new(),
            polarity: Polarity::Unconfirmed,
            confirmed: false,
            switchable: false,
            show_in_homekit: true,
        },
    ]
}

#[test]
fn json_is_byte_identical_to_vec_sensor_derive() {
    let sensors = tricky_sensors();
    let table = SensorTable::from_sensors(&sensors).unwrap();
    let via_vec = serde_json::to_string(&sensors).unwrap();
    let via_table = serde_json::to_string(&table).unwrap();
    assert_eq!(via_vec, via_table);
}

#[test]
fn roundtrip_at_600_sensors() {
    let sensors: Vec<Sensor> = (0..600)
        .map(|i| {
            sensor(
                0x0100 + i,
                &format!("Bewegungsmelder Obergeschoss Flur {i:03}"),
            )
        })
        .collect();
    let table = SensorTable::from_sensors(&sensors).unwrap();
    assert_eq!(table.len(), 600);
    let json = serde_json::to_string(&table).unwrap();
    let back: SensorTable = serde_json::from_str(&json).unwrap();
    assert_eq!(table, back);
    // Memory bound: arena + padding must stay well under the naive per-String
    // cost (~600 × 3 × (24 B overhead + payload)).
    assert!(
        back.arena_bytes() < 96 * 1024,
        "Arena unerwartet groß: {} B",
        back.arena_bytes()
    );
}

#[test]
fn mutation_retain_compact_roundtrip() {
    let mut table = SensorTable::from_sensors(&tricky_sensors()).unwrap();

    // Edit via SensorMut (appends to the arena, old span becomes waste).
    {
        let mut m = table.find_mut(0x0042).unwrap();
        m.set_name("Tür Süd neu").unwrap();
        m.set_confirmed(false);
        m.set_kind(SensorKind::Schliesskontakt);
    }
    let s = table.find(0x0042).unwrap();
    assert_eq!(s.name(), "Tür Süd neu");
    assert!(!s.confirmed());
    assert_eq!(s.kind(), SensorKind::Schliesskontakt);
    // name_ha untouched by the edit.
    assert_eq!(s.name_ha(), "Tür \\ Süd / <ÖL> (HA)");

    // Drop one, compact, verify the rest survives with correct strings.
    table.retain(|s| s.address() != 0x0001);
    table.compact().unwrap();
    assert_eq!(table.len(), 2);
    assert!(table.find(0x0001).is_none());
    assert_eq!(table.find(0x0042).unwrap().name(), "Tür Süd neu");
    assert_eq!(table.find(0x0533).unwrap().name_ha(), "🚨 Alarm");
}

#[test]
fn clone_is_equal_and_independent() {
    let mut table = SensorTable::from_sensors(&tricky_sensors()).unwrap();
    let copy = table.clone();
    assert_eq!(table, copy);
    table
        .find_mut(0x0001)
        .unwrap()
        .set_name("geändert")
        .unwrap();
    assert_ne!(table, copy);
}

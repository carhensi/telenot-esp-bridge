//! Machine-readable entity manifest (MQTT topic `inventory`, retained). Lets a custom HA
//! integration (path B) or any other consumer know the complete entity set without having
//! to parse HA-Discovery (path A, `homeassistant/…`). **HA-agnostic:** only generic fields
//! (id/name/platform/device_class/state_topic/area). State topics are relative to the bridge
//! root (`<root>/…`), as per the MQTT contract.

use serde_json::{json, Value};
use telenot_config::{Config, SensorKind};

/// Confirmed entities per chunk payload when the inventory is chunked (~64 × ~220 B ≈ 14 KB).
pub const INVENTORY_CHUNK_ENTITIES: usize = 64;
/// Up to this many confirmed entities the inventory stays a SINGLE v1 payload on `inventory`
/// (~130 × ~220 B ≈ 29 KB — safely under the 32 KB transmit-buffer pain threshold). Above,
/// `inventory_for_each` switches to envelope + `inventory/chunk/<i>` topics.
pub const INVENTORY_SINGLE_MAX: usize = 130;

/// Diagnostic sensor kinds → `entity_category: diagnostic` (mirrors `hadisco`).
fn is_diagnostic(k: SensorKind) -> bool {
    matches!(
        k,
        SensorKind::Batterie
            | SensorKind::Netzstoerung
            | SensorKind::Uebertragung
            | SensorKind::Stoerung
            | SensorKind::Technik
            | SensorKind::Systemstatus
            | SensorKind::Sicherung
            | SensorKind::Bedienteil
            | SensorKind::Gebaeudetechnik
    )
}

/// Builds the retained inventory JSON for `<root>/inventory` — thin v1 wrapper over
/// [`payload_slice`]. `device_id` is the fixed internal HA identifier (used only for entity
/// `id`s/grouping, not in the topic path — single device). For large inventories use
/// [`inventory_for_each`] instead (chunked, bounded per-message heap).
pub fn inventory_payload(config: &Config) -> String {
    payload_slice(config, 0, usize::MAX, None)
}

/// Emits the inventory as one or more retained MQTT messages via `emit(topic_suffix, payload)`.
///
/// - ≤ [`INVENTORY_SINGLE_MAX`] confirmed entities: exactly one
///   `emit("inventory", <payload>)`, byte-identical to [`inventory_payload`] (v1 unchanged).
/// - above: first a compact envelope on `inventory`
///   (`{"schema_version":1,"chunked":true,"chunk_count":N,"entities_total":M}`, where M counts
///   all entities across the chunks incl. alarm + readiness), then one
///   `emit("inventory/chunk/<i>", …)` per chunk. Each chunk string is built, emitted, dropped —
///   never all at once (the single ~112 KB string at 500 sensors was a heap wall).
pub fn inventory_for_each(config: &Config, emit: &mut dyn FnMut(&str, &str)) {
    let confirmed = config.sensors.iter().filter(|s| s.confirmed()).count();
    if confirmed <= INVENTORY_SINGLE_MAX {
        emit("inventory", &inventory_payload(config));
        return;
    }
    let chunk_count = confirmed.div_ceil(INVENTORY_CHUNK_ENTITIES);
    // 3 fixed entities (alarm + 2 readiness, chunk 0 only) + confirmed sensors.
    let envelope = format!(
        "{{\"schema_version\":{},\"chunked\":true,\"chunk_count\":{},\"entities_total\":{}}}",
        telenot_config::CURRENT_SCHEMA_VERSION,
        chunk_count,
        confirmed + 3
    );
    emit("inventory", &envelope);
    for i in 0..chunk_count {
        let payload = payload_slice(
            config,
            i * INVENTORY_CHUNK_ENTITIES,
            INVENTORY_CHUNK_ENTITIES,
            Some((i, chunk_count)),
        );
        emit(&format!("inventory/chunk/{i}"), &payload);
    }
}

/// Builds one inventory JSON document over the confirmed-sensor window `[skip, skip+take)`.
/// With `skip=0, take=all, chunked_of=None` the output is byte-identical to the historic
/// `inventory_payload` (v1 contract). With `chunked_of=Some((i, n))` the fields
/// `"chunk":i,"chunk_count":n` are inserted directly after `device_id` (before `entities`).
/// The fixed entities (alarm + readiness) are emitted only for `skip == 0`, i.e. exactly once
/// across all chunks.
fn payload_slice(
    config: &Config,
    skip: usize,
    take: usize,
    chunked_of: Option<(usize, usize)>,
) -> String {
    let device_id = crate::hadisco::HA_ID;
    // Build into a single pre-sized string incrementally — only one small transient Value
    // per entity. The old Vec<Value> over all sensors (~70 KB tree overhead at 147 detectors)
    // contributed to the boot OOM on the ESP32.
    let mut buf = String::with_capacity(2048 + take.min(config.sensors.len()) * 220);
    buf.push_str(&format!(
        "{{\"schema_version\":{},\"device_id\":\"{}\"",
        telenot_config::CURRENT_SCHEMA_VERSION,
        device_id
    ));
    if let Some((i, n)) = chunked_of {
        buf.push_str(&format!(",\"chunk\":{i},\"chunk_count\":{n}"));
    }
    buf.push_str(",\"entities\":[");
    let mut first = true;
    let mut push = |buf: &mut String, e: Value| {
        if !first {
            buf.push(',');
        }
        first = false;
        buf.push_str(&e.to_string());
    };

    if skip == 0 {
        // Alarm state (panel).
        push(
            &mut buf,
            json!({
                "id": format!("{device_id}_alarm"),
                "name": "Alarmzustand",
                "platform": "alarm_control_panel",
                "state_topic": "state",
            }),
        );

        // Arm-readiness sensors (diagnostic).
        for (sub, name) in [
            ("ready/intern", "Scharf intern bereit"),
            ("ready/extern", "Scharf extern bereit"),
        ] {
            push(
                &mut buf,
                json!({
                    "id": format!("{device_id}_{}", sub.replace('/', "_")),
                    "name": name,
                    "platform": "binary_sensor",
                    "device_class": "safety",
                    "state_topic": sub,
                    "entity_category": "diagnostic",
                }),
            );
        }
    }

    // Confirmed sensors in the requested window.
    for s in config
        .sensors
        .iter()
        .filter(|s| s.confirmed())
        .skip(skip)
        .take(take)
    {
        let name = if s.name_ha().trim().is_empty() {
            s.name()
        } else {
            s.name_ha()
        };
        let mut e = json!({
            "id": format!("{device_id}_{:04x}", s.address()),
            "name": name,
            "platform": "binary_sensor",
            "device_class": s.kind().device_class(),
            "state_topic": format!("sensor/{}/state", s.topic()),
        });
        if is_diagnostic(s.kind()) {
            e["entity_category"] = json!("diagnostic");
        }
        push(&mut buf, e);
    }

    buf.push_str("]}");
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use telenot_config::{Polarity, Sensor, CURRENT_SCHEMA_VERSION};

    fn cfg() -> Config {
        let s = |address, name: &str, kind, confirmed| Sensor {
            address,
            name: name.into(),
            name_ha: name.into(),
            kind,
            topic: "eg/tuer".into(),
            polarity: Polarity::ActiveLow,
            confirmed,
            switchable: false,
            show_in_homekit: false,
        };
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: telenot_config::SensorTable::from_sensors(&[
                s(0x0042, "MK Haustür", SensorKind::Magnetkontakt, true),
                s(0x0099, "Akku", SensorKind::Batterie, true),
                s(0x0050, "Unbestätigt", SensorKind::Bewegungsmelder, false),
            ])
            .unwrap(),
            panel: Default::default(),
        }
    }

    #[test]
    fn inventory_lists_confirmed_plus_alarm_and_readiness() {
        let payload = inventory_payload(&cfg());
        let v: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["device_id"], "telenot-bridge");
        let ents = v["entities"].as_array().unwrap();
        // alarm + 2 readiness + 2 confirmed (unconfirmed is excluded)
        assert_eq!(ents.len(), 5);

        let alarm = ents
            .iter()
            .find(|e| e["platform"] == "alarm_control_panel")
            .unwrap();
        assert_eq!(alarm["state_topic"], "state");

        let mk = ents
            .iter()
            .find(|e| e["id"] == "telenot-bridge_0042")
            .unwrap();
        assert_eq!(mk["device_class"], "window");
        assert_eq!(mk["state_topic"], "sensor/eg/tuer/state");
        assert!(
            mk.get("entity_category").is_none(),
            "window sensor is not a diagnostic"
        );

        let akku = ents
            .iter()
            .find(|e| e["id"] == "telenot-bridge_0099")
            .unwrap();
        assert_eq!(akku["device_class"], "battery");
        assert_eq!(akku["entity_category"], "diagnostic");

        assert!(
            !ents.iter().any(|e| e["id"] == "telenot-bridge_0050"),
            "unconfirmed sensor must be absent"
        );
    }

    /// Config with `n` confirmed motion sensors (distinct addresses/topics).
    fn cfg_with_confirmed(n: usize) -> Config {
        let sensors: Vec<Sensor> = (0..n)
            .map(|i| Sensor {
                address: 0x0100 + i as u16,
                name: format!("Melder {i}"),
                name_ha: format!("Melder {i}"),
                kind: SensorKind::Bewegungsmelder,
                topic: format!("bereich/melder{i}"),
                polarity: Polarity::ActiveLow,
                confirmed: true,
                switchable: false,
                show_in_homekit: false,
            })
            .collect();
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: telenot_config::SensorTable::from_sensors(&sensors).unwrap(),
            panel: Default::default(),
        }
    }

    #[test]
    fn for_each_below_threshold_is_single_and_byte_identical() {
        for config in [cfg(), cfg_with_confirmed(INVENTORY_SINGLE_MAX)] {
            let mut emits: Vec<(String, String)> = Vec::new();
            inventory_for_each(&config, &mut |topic, payload| {
                emits.push((topic.to_string(), payload.to_string()));
            });
            assert_eq!(emits.len(), 1, "≤ threshold → exactly one emit");
            assert_eq!(emits[0].0, "inventory");
            assert_eq!(
                emits[0].1,
                inventory_payload(&config),
                "single payload must be byte-identical to v1 inventory_payload"
            );
        }
    }

    #[test]
    fn for_each_above_threshold_chunks_completely() {
        let config = cfg_with_confirmed(200);
        let mut emits: Vec<(String, String)> = Vec::new();
        inventory_for_each(&config, &mut |topic, payload| {
            emits.push((topic.to_string(), payload.to_string()));
        });

        // Envelope on `inventory` + ceil(200/64) = 4 chunks.
        assert_eq!(emits.len(), 5);
        assert_eq!(emits[0].0, "inventory");
        let env: Value = serde_json::from_str(&emits[0].1).unwrap();
        assert_eq!(env["schema_version"], CURRENT_SCHEMA_VERSION);
        assert_eq!(env["chunked"], true);
        assert_eq!(env["chunk_count"], 4);
        assert_eq!(env["entities_total"], 203); // 200 confirmed + alarm + 2 readiness

        let mut ids: Vec<String> = Vec::new();
        for (i, (topic, payload)) in emits[1..].iter().enumerate() {
            assert_eq!(topic, &format!("inventory/chunk/{i}"));
            assert!(
                payload.len() < 32 * 1024,
                "chunk {i} is {} B — must stay < 32 KB",
                payload.len()
            );
            let v: Value = serde_json::from_str(payload).unwrap();
            assert_eq!(v["chunk"], i);
            assert_eq!(v["chunk_count"], 4);
            assert_eq!(v["device_id"], "telenot-bridge");
            for e in v["entities"].as_array().unwrap() {
                ids.push(e["id"].as_str().unwrap().to_string());
            }
        }
        // All entities exactly once across the chunks (200 sensors + 3 fixed in chunk 0).
        assert_eq!(ids.len(), 203);
        let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), 203, "no entity may appear twice");
        for i in 0..200usize {
            assert!(ids.contains(&format!("telenot-bridge_{:04x}", 0x0100 + i)));
        }
        assert!(ids.contains(&"telenot-bridge_alarm".to_string()));
    }
}

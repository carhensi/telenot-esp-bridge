//! Home Assistant MQTT Discovery: builds `homeassistant/.../config` messages from the config
//! so that HA creates entities **automatically**. Optional module (core stays HA-agnostic);
//! pure and testable, shared between host daemon and firmware.
//!
//! Produces (read direction): one `binary_sensor` per confirmed sensor with the matching
//! `device_class`, a `sensor` for the alarm state, and two diagnostic arm-readiness
//! `binary_sensor`s. Control from HA (alarm_control_panel + command path) is deliberately
//! NOT part of this read module.

use serde_json::{json, Value};
use telenot_config::{Config, SensorKind};

/// Fixed internal HA identifier. The bridge runs as a **single device** → no dynamic device ID
/// needed. Used only for HA `unique_id`/`identifiers` + discovery node, NEVER in the MQTT
/// topic path (topics are device-free: `<root>/state`, `<root>/sensor/<slug>/state`).
pub const HA_ID: &str = "telenot-bridge";

/// Parameters for discovery generation.
pub struct DiscoveryOpts {
    /// HA discovery prefix (default `homeassistant`).
    pub discovery_prefix: String,
    /// Topic root of the bridge (e.g. `telenot/v1`).
    pub topic_root: String,
    pub device_name: String,
    pub model: String,
    pub sw_version: String,
    /// HA receives a controllable `alarm_control_panel` (with command topic) instead of a
    /// read-only alarm-state sensor. Set only when the bridge also accepts commands.
    pub controllable: bool,
    /// Sicherungsbereiche `(id, display name)` for multi-area panels (hiplex). Empty
    /// (default/complex) = exactly the historical single-panel entity set. Non-empty =
    /// one panel entity per area (`area/{id}/state|command`) plus a read-only aggregate.
    pub areas: Vec<(u8, String)>,
}

impl DiscoveryOpts {
    pub fn new(topic_root: impl Into<String>) -> Self {
        DiscoveryOpts {
            discovery_prefix: "homeassistant".into(),
            topic_root: topic_root.into(),
            device_name: "Telenot Bridge".into(),
            model: "EMA-Bridge".into(),
            sw_version: env!("CARGO_PKG_VERSION").into(),
            controllable: false,
            areas: Vec::new(),
        }
    }
}

/// Area list for [`DiscoveryOpts::areas`] from the config: empty for single-area panels
/// (complex → historical entity set); for hiplex the user-named areas, defaulting to
/// area 1 when none are named yet (16 unnamed panels would only clutter HA).
pub fn areas_from_config(config: &Config) -> Vec<(u8, String)> {
    if config.panel.kind != telenot_config::PanelKind::Hiplex8400 {
        return Vec::new();
    }
    if config.panel.areas.is_empty() {
        return vec![(1, "Bereich 1".into())];
    }
    config
        .panel
        .areas
        .iter()
        .map(|a| {
            let name = if a.name.trim().is_empty() {
                format!("Bereich {}", a.id)
            } else {
                a.name.clone()
            };
            (a.id, name)
        })
        .collect()
}

/// A ready-to-publish discovery message (absolute topic, retained).
pub struct DiscoveryMsg {
    pub topic: String,
    pub payload: String,
    pub retain: bool,
}

/// Sanitize to a valid HA object ID (HA allows `[a-zA-Z0-9_-]`).
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

/// Diagnostic sensor kinds → `entity_category: diagnostic` (not a primary room-control sensor).
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

/// Collecting variant of [`discovery_for_each`] (host/tests — RAM is plentiful).
pub fn discovery_messages(config: &Config, o: &DiscoveryOpts) -> Vec<DiscoveryMsg> {
    let mut out = Vec::new();
    discovery_for_each(config, o, &mut |m| out.push(m));
    out
}

/// Streams all discovery messages one at a time to `emit` (all retained). On the ESP32,
/// all ~150 messages (~120 KB of payload strings) must never be in memory simultaneously —
/// keeping them all caused an OOM boot loop with a full config.
pub fn discovery_for_each(config: &Config, o: &DiscoveryOpts, emit: &mut dyn FnMut(DiscoveryMsg)) {
    let dev = json!({
        "identifiers": [HA_ID],
        "name": o.device_name,
        "manufacturer": "telenot-bridge",
        "model": o.model,
        "sw_version": o.sw_version,
    });
    let avail = json!([{
        "topic": format!("{}/availability", o.topic_root),
        "payload_available": "online",
        "payload_not_available": "offline",
    }]);

    if o.areas.is_empty() {
        if o.controllable {
            // Controllable alarm_control_panel: HA gets arm/disarm buttons + PIN prompt.
            // command_template translates HA action/code into our generic v1 `command` (`{cmd,pin}`).
            emit(DiscoveryMsg {
                topic: format!(
                    "{}/alarm_control_panel/{}/{}_alarm/config",
                    o.discovery_prefix, HA_ID, HA_ID
                ),
                payload: json!({
                    "name": "Alarm",
                    "unique_id": format!("{}_alarm", HA_ID),
                    "state_topic": format!("{}/state", o.topic_root),
                    "value_template": "{{ value | lower }}",
                    "command_topic": format!("{}/command", o.topic_root),
                    "command_template": "{\"cmd\": \"{{ action }}\", \"pin\": \"{{ code }}\"}",
                    "payload_arm_away": "ARM_AWAY",
                    "payload_arm_home": "ARM_HOME",
                    "payload_arm_night": "ARM_NIGHT",
                    "payload_disarm": "DISARM",
                    "code_arm_required": false,
                    "code_disarm_required": true,
                    "availability": avail,
                    "device": dev,
                })
                .to_string(),
                retain: true,
            });
        } else {
            // Read-only alarm state sensor (DISARMED/ARMED_*/TRIGGERED/unknown).
            emit(DiscoveryMsg {
                topic: format!(
                    "{}/sensor/{}/{}_alarm_state/config",
                    o.discovery_prefix, HA_ID, HA_ID
                ),
                payload: json!({
                    "name": "Alarmzustand",
                    "unique_id": format!("{}_alarm_state", HA_ID),
                    "state_topic": format!("{}/state", o.topic_root),
                    "icon": "mdi:shield-home",
                    "availability": avail,
                    "device": dev,
                })
                .to_string(),
                retain: true,
            });
        }

        // Arm-readiness sensors (diagnostic).
        for (sub, name) in [
            ("ready/intern", "Scharf intern bereit"),
            ("ready/extern", "Scharf extern bereit"),
        ] {
            let oid = sanitize(&format!("{}_{}", HA_ID, sub));
            emit(DiscoveryMsg {
                topic: format!(
                    "{}/binary_sensor/{}/{}/config",
                    o.discovery_prefix, HA_ID, oid
                ),
                payload: json!({
                    "name": name,
                    "unique_id": oid,
                    "state_topic": format!("{}/{}", o.topic_root, sub),
                    "payload_on": "yes",
                    "payload_off": "no",
                    "device_class": "safety",
                    "entity_category": "diagnostic",
                    "availability": avail,
                    "device": dev,
                })
                .to_string(),
                retain: true,
            });
        }
    } else {
        // Multi-area panel (hiplex): read-only aggregate + one panel entity per area.
        // The aggregate carries no command topic — commands go through the per-area
        // entities (or the legacy {root}/command, which targets area 1).
        emit(DiscoveryMsg {
            topic: format!(
                "{}/sensor/{}/{}_alarm_state/config",
                o.discovery_prefix, HA_ID, HA_ID
            ),
            payload: json!({
                "name": "Alarmzustand (Gesamt)",
                "unique_id": format!("{}_alarm_state", HA_ID),
                "state_topic": format!("{}/state", o.topic_root),
                "icon": "mdi:shield-home",
                "availability": avail,
                "device": dev,
            })
            .to_string(),
            retain: true,
        });
        for (id, area_name) in &o.areas {
            if o.controllable {
                emit(DiscoveryMsg {
                    topic: format!(
                        "{}/alarm_control_panel/{}/{}_area{}_alarm/config",
                        o.discovery_prefix, HA_ID, HA_ID, id
                    ),
                    payload: json!({
                        "name": area_name,
                        "unique_id": format!("{}_area{}_alarm", HA_ID, id),
                        "state_topic": format!("{}/area/{}/state", o.topic_root, id),
                        "value_template": "{{ value | lower }}",
                        "command_topic": format!("{}/area/{}/command", o.topic_root, id),
                        "command_template": "{\"cmd\": \"{{ action }}\", \"pin\": \"{{ code }}\"}",
                        "payload_arm_away": "ARM_AWAY",
                        "payload_arm_home": "ARM_HOME",
                        "payload_arm_night": "ARM_NIGHT",
                        "payload_disarm": "DISARM",
                        "code_arm_required": false,
                        "code_disarm_required": true,
                        "availability": avail,
                        "device": dev,
                    })
                    .to_string(),
                    retain: true,
                });
            } else {
                emit(DiscoveryMsg {
                    topic: format!(
                        "{}/sensor/{}/{}_area{}_state/config",
                        o.discovery_prefix, HA_ID, HA_ID, id
                    ),
                    payload: json!({
                        "name": area_name,
                        "unique_id": format!("{}_area{}_state", HA_ID, id),
                        "state_topic": format!("{}/area/{}/state", o.topic_root, id),
                        "icon": "mdi:shield-home",
                        "availability": avail,
                        "device": dev,
                    })
                    .to_string(),
                    retain: true,
                });
            }
            for (sub, ready_name) in [("intern", "intern bereit"), ("extern", "extern bereit")] {
                let oid = sanitize(&format!("{}_area{}_ready_{}", HA_ID, id, sub));
                emit(DiscoveryMsg {
                    topic: format!(
                        "{}/binary_sensor/{}/{}/config",
                        o.discovery_prefix, HA_ID, oid
                    ),
                    payload: json!({
                        "name": format!("{area_name} {ready_name}"),
                        "unique_id": oid,
                        "state_topic": format!("{}/area/{}/ready/{}", o.topic_root, id, sub),
                        "payload_on": "yes",
                        "payload_off": "no",
                        "device_class": "safety",
                        "entity_category": "diagnostic",
                        "availability": avail,
                        "device": dev,
                    })
                    .to_string(),
                    retain: true,
                });
            }
        }
    }

    // One binary_sensor per confirmed sensor.
    for s in config.sensors.iter().filter(|s| s.confirmed()) {
        let oid = format!("{}_{:04x}", HA_ID, s.address());
        let name = if s.name_ha().trim().is_empty() {
            s.name()
        } else {
            s.name_ha()
        };
        let mut payload: Value = json!({
            "name": name,
            "unique_id": oid,
            "state_topic": format!("{}/sensor/{}/state", o.topic_root, s.topic()),
            "payload_on": "ON",
            "payload_off": "OFF",
            "device_class": s.kind().device_class(),
            "availability": avail,
            "device": dev,
        });
        if is_diagnostic(s.kind()) {
            payload["entity_category"] = json!("diagnostic");
        }
        emit(DiscoveryMsg {
            topic: format!(
                "{}/binary_sensor/{}/{}/config",
                o.discovery_prefix, HA_ID, oid
            ),
            payload: payload.to_string(),
            retain: true,
        });
    }

    // Switch outputs: one `switch` entity per enabled (`switchable`) output — only when the
    // command path is active (otherwise the switch entity would be dead). State comes from
    // readback (`optimistic: false`); switching goes through the generic v1 `command`.
    if o.controllable {
        for s in config.sensors.iter().filter(|s| {
            s.confirmed()
                && s.switchable()
                // Hand-edited configs could set switchable on invalid addresses
                // (validation error) — avoid building a dead or dangerous switch entity.
                && telenot_config::is_switchable_addr(s.address())
        }) {
            let oid = format!("{}_{:04x}_switch", HA_ID, s.address());
            let name = if s.name_ha().trim().is_empty() {
                s.name()
            } else {
                s.name_ha()
            };
            emit(DiscoveryMsg {
                topic: format!("{}/switch/{}/{}/config", o.discovery_prefix, HA_ID, oid),
                payload: json!({
                    "name": name,
                    "unique_id": oid,
                    "state_topic": format!("{}/sensor/{}/state", o.topic_root, s.topic()),
                    "command_topic": format!("{}/command", o.topic_root),
                    "payload_on": format!("{{\"cmd\":\"OUTPUT_ON\",\"addr\":{}}}", s.address()),
                    "payload_off": format!("{{\"cmd\":\"OUTPUT_OFF\",\"addr\":{}}}", s.address()),
                    "state_on": "ON",
                    "state_off": "OFF",
                    "optimistic": false,
                    "availability": avail,
                    "device": dev,
                })
                .to_string(),
                retain: true,
            });
        }
    }
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
            location: "EG".into(),
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
    fn multi_area_discovery_builds_per_area_panels() {
        use telenot_config::{AreaCfg, PanelKind};
        let mut config = cfg();
        config.panel.kind = PanelKind::Hiplex8400;
        config.panel.areas = vec![
            AreaCfg {
                id: 1,
                name: "EG".into(),
            },
            AreaCfg {
                id: 16,
                name: String::new(),
            },
        ];
        let mut o = DiscoveryOpts::new("telenot/v1");
        o.controllable = true;
        o.areas = areas_from_config(&config);
        let msgs = discovery_messages(&config, &o);

        let a1 = msgs
            .iter()
            .find(|m| m.topic.contains("area1_alarm"))
            .expect("per-area panel for area 1");
        let v: Value = serde_json::from_str(&a1.payload).unwrap();
        assert_eq!(v["name"], "EG");
        assert_eq!(v["state_topic"], "telenot/v1/area/1/state");
        assert_eq!(v["command_topic"], "telenot/v1/area/1/command");

        // Unnamed area falls back to a generic name (Zentralen-Schutzbereich = 16).
        let a16 = msgs
            .iter()
            .find(|m| m.topic.contains("area16_alarm"))
            .expect("per-area panel for area 16");
        let v: Value = serde_json::from_str(&a16.payload).unwrap();
        assert_eq!(v["name"], "Bereich 16");

        // The aggregate is a read-only sensor; no aggregate alarm_control_panel.
        assert!(msgs.iter().any(|m| m.topic.contains("_alarm_state/config")));
        assert!(!msgs.iter().any(|m| m.topic
            == "homeassistant/alarm_control_panel/telenot-bridge/telenot-bridge_alarm/config"));
        // Per-area readiness entities exist; legacy ready/* topics do not appear.
        assert!(msgs
            .iter()
            .any(|m| m.payload.contains("telenot/v1/area/1/ready/intern")));
        assert!(!msgs
            .iter()
            .any(|m| m.payload.contains("\"telenot/v1/ready/intern\"")));
    }

    #[test]
    fn complex_config_yields_empty_area_list() {
        assert!(areas_from_config(&cfg()).is_empty());
        let mut hiplex = cfg();
        hiplex.panel.kind = telenot_config::PanelKind::Hiplex8400;
        // hiplex without named areas: default to area 1 only (no 16-panel HA clutter).
        assert_eq!(
            areas_from_config(&hiplex),
            vec![(1, "Bereich 1".to_string())]
        );
    }

    #[test]
    fn builds_valid_discovery() {
        let o = DiscoveryOpts::new("telenot/v1");
        let msgs = discovery_messages(&cfg(), &o);
        // alarm + 2 readiness + 2 confirmed (unconfirmed is excluded)
        assert_eq!(msgs.len(), 5);

        let mk = msgs
            .iter()
            .find(|m| m.topic.contains("0042"))
            .expect("MK-Sensor");
        assert_eq!(
            mk.topic,
            "homeassistant/binary_sensor/telenot-bridge/telenot-bridge_0042/config"
        );
        let v: Value = serde_json::from_str(&mk.payload).unwrap();
        assert_eq!(v["device_class"], "window");
        assert_eq!(v["state_topic"], "telenot/v1/sensor/eg/tuer/state");
        assert_eq!(v["payload_on"], "ON");
        assert_eq!(v["device"]["identifiers"][0], "telenot-bridge");
        assert!(
            v.get("entity_category").is_none(),
            "window sensor is not a diagnostic"
        );

        let akku: Value = serde_json::from_str(
            &msgs
                .iter()
                .find(|m| m.topic.contains("0099"))
                .unwrap()
                .payload,
        )
        .unwrap();
        assert_eq!(akku["device_class"], "battery");
        assert_eq!(akku["entity_category"], "diagnostic");

        let alarm: Value = serde_json::from_str(
            &msgs
                .iter()
                .find(|m| m.topic.contains("alarm_state"))
                .unwrap()
                .payload,
        )
        .unwrap();
        assert_eq!(alarm["state_topic"], "telenot/v1/state");

        assert!(
            msgs.iter().all(|m| m.retain),
            "discovery messages must be retained"
        );
    }

    #[test]
    fn switchable_output_gets_switch_entity_only_when_controllable() {
        let mut config = cfg();
        let mut out = config.sensors.get(0).unwrap().to_sensor();
        out.address = 0x0515;
        out.topic = "keller/relais".into();
        out.switchable = true;
        config.sensors.push(&out).unwrap();

        // Without command path: no switch entity (would be dead).
        let o = DiscoveryOpts::new("telenot/v1");
        assert!(!discovery_messages(&config, &o)
            .iter()
            .any(|m| m.topic.contains("/switch/")));

        // With command path: exactly one switch with OUTPUT_ON/OFF payloads + readback state.
        let mut o = DiscoveryOpts::new("telenot/v1");
        o.controllable = true;
        let msgs = discovery_messages(&config, &o);
        let sw = msgs
            .iter()
            .find(|m| m.topic.contains("/switch/"))
            .expect("switch entity");
        assert_eq!(
            sw.topic,
            "homeassistant/switch/telenot-bridge/telenot-bridge_0515_switch/config"
        );
        let p: serde_json::Value = serde_json::from_str(&sw.payload).unwrap();
        assert_eq!(p["payload_on"], "{\"cmd\":\"OUTPUT_ON\",\"addr\":1301}");
        assert_eq!(p["optimistic"], false);
        assert_eq!(p["state_topic"], "telenot/v1/sensor/keller/relais/state");
    }

    #[test]
    fn controllable_publishes_alarm_control_panel() {
        let mut o = DiscoveryOpts::new("telenot/v1");
        o.controllable = true;
        let msgs = discovery_messages(&cfg(), &o);

        let panel = msgs
            .iter()
            .find(|m| m.topic.contains("alarm_control_panel"))
            .expect("alarm_control_panel");
        assert_eq!(
            panel.topic,
            "homeassistant/alarm_control_panel/telenot-bridge/telenot-bridge_alarm/config"
        );
        let v: Value = serde_json::from_str(&panel.payload).unwrap();
        assert_eq!(v["state_topic"], "telenot/v1/state");
        assert_eq!(v["command_topic"], "telenot/v1/command");
        assert_eq!(v["value_template"], "{{ value | lower }}");
        assert!(v["command_template"].as_str().unwrap().contains("\"cmd\""));
        assert_eq!(v["payload_disarm"], "DISARM");
        assert_eq!(v["code_disarm_required"], true);

        // in controllable mode, NO read-only alarm-state sensor (no duplicate entity)
        assert!(!msgs.iter().any(|m| m.topic.contains("alarm_state")));
    }
}

# Home Assistant — Anbindung der Telenot-Bridge

Die Bridge publiziert per **MQTT** und legt alle Entities über MQTT-Discovery automatisch an.

## 1. Voraussetzung: MQTT-Integration in HA

1. In HA **Einstellungen → Geräte & Dienste → Integration hinzufügen → MQTT**.
2. Broker/User/Passwort = **derselbe Broker**, den du im Bridge-Setup (Schritt „MQTT / Home
   Assistant") eingetragen hast (z.B. Mosquitto-Add-on oder externer Broker).
3. Fertig — sobald die Bridge läuft und `HA-Discovery` an ist, tauchen die Entities auf.

## 2. Was automatisch entsteht (Discovery)

Mit `HA-Discovery = an` publiziert die Bridge nach `homeassistant/…`:

| Entity | Typ | Beispiel entity_id |
|---|---|---|
| Alarm-Zentrale (steuerbar) | `alarm_control_panel` | `alarm_control_panel.alarm` |
| Scharf-intern-bereit | `binary_sensor` (safety) | `binary_sensor.scharf_intern_bereit` |
| Scharf-extern-bereit | `binary_sensor` (safety) | `binary_sensor.scharf_extern_bereit` |
| je bestätigter Melder | `binary_sensor` (passende device_class) | `binary_sensor.<name_ha>` |

Das Alarm-Panel kann `ARM_HOME`/`ARM_AWAY`/`ARM_NIGHT`/`DISARM`, States
`disarmed`/`armed_home`/`armed_away`/`armed_night`/`triggered`. **Disarm braucht die PIN**
(`code_disarm_required: true`), wenn im Setup Remote-Disarm aktiviert ist — ohne das Opt-in
ist Disarm über die Bridge gesperrt (fail-closed).

## 3. Umstieg vom Alt-System

Ein vorhandenes **manuelles** `alarm_control_panel:` in HA **löschen** — es nutzt die alten
Topics (`telenot/alarm/state|command`); die Bridge publiziert jetzt unter
`telenot/v1/state|command`. Das Dashboard ist wiederverwendbar, nur die entity_ids umbiegen:

| Alt | Neu (Discovery) |
|---|---|
| `alarm_control_panel.alarmanlage` | `alarm_control_panel.alarm` |
| `binary_sensor.telenot_zentrale_system_intern_bereit` | `binary_sensor.scharf_intern_bereit` |
| `binary_sensor.telenot_zentrale_system_extern_bereit` | `binary_sensor.scharf_extern_bereit` |

Alternativ: das Discovery-Alarm-Entity in HA auf **„Alarmanlage"** umbenennen → entity_id
wird `alarm_control_panel.alarmanlage`, dann läuft das alte Dashboard **1:1** weiter.

## 4. Starter-Card (kein HACS nötig)

Die eingebaute Alarm-Card (auch der Kopieren-Button im Setup liefert genau das):

```yaml
type: alarm-panel
name: Alarmanlage
entity: alarm_control_panel.alarm
states:
  - arm_home
  - arm_away
  - arm_night
```

Ein ausführliches **Button-Card-Dashboard** funktioniert weiter — entity_ids gemäß Tabelle
in Abschnitt 3 ersetzen. `auto-entities`-Filter (HACS) filtern am robustesten nach
`device_class` statt nach Namensmustern wie `*bewegungsmelder*`.

## 5. HomeKit

- **Direkt vom ESP32 (Default, ohne MQTT/HA):** die Firmware spricht natives HAP —
  SecuritySystem-Kachel + einzelne Melder per `show_in_homekit` (siehe README).
- **Über HA:** HAs **HomeKit-Bridge** (Einstellungen → Geräte & Dienste → HomeKit) oder
  Matter-Bridge exponiert die Entities nach Apple Home.
- **Über Homebridge (ohne HA):** `homebridge-mqttthing` kann dieselben MQTT-Topics direkt
  zu HomeKit bringen.

MQTT ist die **neutrale Weiche**: derselbe Datenstrom bedient HA *und* Homebridge.

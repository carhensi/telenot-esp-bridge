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
| Alarm-Zentrale (steuerbar) | `alarm_control_panel` | `alarm_control_panel.telenot_bridge_alarm` |
| Scharf-intern-bereit | `binary_sensor` (safety) | `binary_sensor.telenot_bridge_scharf_intern_bereit` |
| Scharf-extern-bereit | `binary_sensor` (safety) | `binary_sensor.telenot_bridge_scharf_extern_bereit` |
| je bestätigter Melder | `binary_sensor` (passende device_class) | `binary_sensor.telenot_bridge_<name>` |

> **entity_id-Präfix:** Alle Entities hängen an **einem** Gerät „Telenot Bridge" → HA setzt
> `telenot_bridge_` vor die entity_id **und** stellt den Gerätenamen dem Anzeigenamen voran
> („Telenot Bridge Küche Fenster"). Willst du saubere Anzeigenamen ohne Präfix, gib der Entity
> in HA einen **Namen-Override** (Entität → Einstellungen → Name) — der ersetzt den Präfix.

Das Alarm-Panel kann `ARM_HOME`/`ARM_AWAY`/`ARM_NIGHT`/`DISARM`, States
`disarmed`/`armed_home`/`armed_away`/`armed_night`/`triggered`. **Disarm braucht die PIN**
(`code_disarm_required: true`), wenn im Setup Remote-Disarm aktiviert ist — ohne das Opt-in
ist Disarm über die Bridge gesperrt (fail-closed). Siehe **Abschnitt 5** für Arm/Disarm mit PIN.

### Namen, Räume & Icons — Home Assistant ist führend

- **Typ & Icon** kommen automatisch über die `device_class` je Melderart (Magnetkontakt →
  `window`, Bewegungsmelder → `motion`, Rauchmelder → `smoke`, …). HA wählt daraus das
  passende Icon und die Auf/Zu-Darstellung — nichts einzustellen.
- **Namen:** Die Bridge liefert als Entity-Name den **Anlagen-Klartext** (aus der EMA
  ausgelesen). Wer einen Melder eindeutiger benennen will (z.B. mehrere Fenster im selben
  Raum), setzt in der Bridge optional den **HA-Namen** (`name_ha`) als Override; ist er leer,
  gilt der Anlagenname.
- **Räume/Bereiche** verwaltest du **in Home Assistant** (Einstellungen → Bereiche), nicht in
  der Bridge. Ebenso: eine Umbenennung einer Entity in HA bleibt bestehen — die Bridge liefert
  nur den **Startwert** und überschreibt deine HA-Änderungen nie. (Gleiches gilt für Apple
  Home: Räume/Namen dort sind ebenfalls führend.)

## 3. Umstieg vom Alt-System

Ein vorhandenes **manuelles** `alarm_control_panel:` in HA **löschen** — es nutzt die alten
Topics (`telenot/alarm/state|command`); die Bridge publiziert jetzt unter
`telenot/v1/state|command`. Das Dashboard ist wiederverwendbar, nur die entity_ids umbiegen:

| Alt | Neu (Discovery) |
|---|---|
| `alarm_control_panel.alarmanlage` | `alarm_control_panel.telenot_bridge_alarm` |
| `binary_sensor.telenot_zentrale_system_intern_bereit` | `binary_sensor.telenot_bridge_scharf_intern_bereit` |
| `binary_sensor.telenot_zentrale_system_extern_bereit` | `binary_sensor.telenot_bridge_scharf_extern_bereit` |

Alternativ: das Discovery-Alarm-Entity in HA auf **„Alarmanlage"** umbenennen → entity_id
wird `alarm_control_panel.alarmanlage`, dann läuft das alte Dashboard **1:1** weiter.
(Vorher das **alte** manuelle/discovery-Entity mit diesem Namen entfernen — sonst kollidiert
die entity_id. War die Alt-Entity per MQTT-Discovery angelegt und ist die alte Quelle tot,
zeigt HA sie als „nicht verfügbar" → in der Entitätenliste löschen.)

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

## 5. Steuern: Arm/Disarm mit PIN

Arm (`ARM_HOME/AWAY/NIGHT`) braucht **keine** PIN. **Disarm** ist PIN-pflichtig (fail-closed),
sofern im Bridge-Setup **Remote-Disarm** aktiviert ist (sonst ist Disarm über die Bridge ganz
gesperrt). Die PIN wird **einmal in HA** hinterlegt, nicht bei jeder Aktion eingetippt.

**1. PIN in `secrets.yaml`** (nicht im Dashboard/Log/State sichtbar):
```yaml
# secrets.yaml
telenot_pin: "1234"
```

**2. Dashboard-Disarm über ein Script** (der „Unscharf"-Button ruft es; es schickt die PIN mit):
```yaml
# scripts.yaml
telenot_disarm:
  alias: Telenot entschärfen
  sequence:
    - action: alarm_control_panel.alarm_disarm
      target: { entity_id: alarm_control_panel.telenot_bridge_alarm }
      data: { code: !secret telenot_pin }
```
Button: `tap_action: { action: call-service, service: script.telenot_disarm }`. Die Arm-Buttons
rufen `alarm_control_panel.alarm_arm_home`/`…_away` direkt (kein Code nötig). Alternativ ganz
ohne Script: die Standard-`alarm-panel`-Karte zeigt einfach ein PIN-Keypad.

### HomeKit-Disarm ohne Code (Template-Panel)

Apple Home hat **keine** PIN-Eingabe für Alarmanlagen. Damit Disarm in Apple Home trotzdem geht,
**ohne** die Firmware-PIN-Sperre aufzuweichen, legt man ein **Template-Alarm-Panel** an, das das
echte Panel spiegelt und die PIN **selbst** durchreicht (HA 2024.9+, modernes `template:`-Format):
```yaml
# configuration.yaml
template:
  - alarm_control_panel:
      - name: "Telenot"
        unique_id: telenot_homekit
        state: "{{ states('alarm_control_panel.telenot_bridge_alarm') }}"
        code_arm_required: false
        code_format: no_code            # kein Code-Zwang → HomeKit-Disarm ohne Eingabe
        arm_home:
          - action: alarm_control_panel.alarm_arm_home
            target: { entity_id: alarm_control_panel.telenot_bridge_alarm }
        arm_away:
          - action: alarm_control_panel.alarm_arm_away
            target: { entity_id: alarm_control_panel.telenot_bridge_alarm }
        arm_night:
          - action: alarm_control_panel.alarm_arm_night
            target: { entity_id: alarm_control_panel.telenot_bridge_alarm }
        disarm:
          - action: alarm_control_panel.alarm_disarm
            target: { entity_id: alarm_control_panel.telenot_bridge_alarm }
            data: { code: !secret telenot_pin }
```
Dann in der **HomeKit-Bridge nur `alarm_control_panel.telenot`** (das Template) exponieren und das
echte `…telenot_bridge_alarm` **ausschließen** (sonst zwei Kacheln, eine mit Code-Zwang). Ergebnis:
in Apple Home entschärfst du **ohne Code** — die **Firmware bleibt PIN-gesichert**, nur HA kennt die
PIN. Ein direkter MQTT-Angreifer ohne PIN kommt weiterhin nicht rein. (Bewusst **nicht** die
Firmware auf „Disarm ohne PIN" stellen — das wäre ein echtes Sicherheits-Downgrade.)

## 6. HomeKit

> **Direkt-HomeKit und MQTT/HA schließen sich aus.** Die Bridge läuft **entweder** als
> nativer HAP-Server **oder** als MQTT-Client — nie beides gleichzeitig. Der Direkt-HomeKit-
> Modus deaktiviert also MQTT/Home Assistant (und umgekehrt); die Web-UI weist im
> Integrations-Schritt darauf hin. „Über HA" bzw. „über Homebridge" (unten) laufen dagegen
> **im MQTT-Modus** und sind damit die Wahl, wenn du HA *und* HomeKit willst.

- **Direkt vom ESP32 (ohne MQTT/HA):** die Firmware spricht natives HAP —
  SecuritySystem-Kachel + einzelne Melder per `show_in_homekit`. Nutzt denselben Namen
  (`name_ha`, sonst Anlagenname) wie HA. Apple-Home-Räume vergibt man in der Home-App.
- **Über HA:** HAs **HomeKit-Bridge** (Einstellungen → Geräte & Dienste → HomeKit) oder
  Matter-Bridge exponiert die Entities nach Apple Home. Für **Disarm ohne Code** in Apple Home
  → das Template-Panel aus **Abschnitt 5** exponieren (nicht das echte, code-pflichtige Panel).
- **Über Homebridge (ohne HA):** `homebridge-mqttthing` kann dieselben MQTT-Topics direkt
  zu HomeKit bringen.

MQTT ist die **neutrale Weiche**: derselbe Datenstrom bedient HA *und* Homebridge.

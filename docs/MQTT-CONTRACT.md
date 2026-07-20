# MQTT-Vertrag v1

Versionierter Vertrag mit **HA-agnostischem Kern, aber HA-first ausgelegt** (siehe Abschnitt
„Home-Assistant-Integration"). Der Kern (`telenot-core`) emittiert **relative**
Topics; die Firmware (B2) stellt den Root `telenot/v1/<device_id>/` voran. `<device_id>`
ist gerätespezifisch (aus MAC/eFuse abgeleitet), sodass mehrere Bridges denselben Broker
teilen können.

## Topics

| relativ | Richtung | Payload | retain |
|---|---|---|---|
| `state` | out | Arm-Zustand (s.u.) | ja |
| `availability` | out | `online` \| `offline` | ja |
| `<sensor.topic>` | out | `ON` \| `OFF` \| `unavailable` | ja |
| `event` | out | spontane Alarme/Sabotage/Störungen (`<Art>@0x<addr>=ein\|aus`) | nein |
| `diag/<art>` | out | `ON`\|`OFF` — Störung `akku`/`netz`/`uebertragung`/`stoerung`/`technik`/`abschaltung` | ja |
| `ready/intern` · `ready/extern` | out | `yes` \| `no` — Scharfschaltbereitschaft | ja |
| `mb/<n>/bypassed` | out | `ON`\|`OFF` — Meldebereich n gesperrt (complex 1–128, hiplex 1–512) | ja |
| `command_result` | out | Ergebnis eines Befehls (z.B. `REJECTED …`) | nein |
| `diagnostics` | out | JSON (s.u.) | ja |
| `inventory` | out | JSON-Manifest aller Entities (s. HA-Abschnitt) | ja |
| `command` | **in** | Befehl (s.u.) | **nein — retained wird verworfen** |
| `homeassistant/...` | out | optionales HA-Discovery-Modul (default AUS) | ja |

### Multi-Bereich (nur hiplex — complex behält exakt das obige Topic-Set)

| relativ | Richtung | Payload | retain |
|---|---|---|---|
| `area/<id>/state` | out | Arm-Zustand des Sicherungsbereichs `id` (1–15, 16 = Zentralen-Schutzbereich) | ja |
| `area/<id>/ready/intern` · `…/extern` | out | `yes` \| `no` je Bereich (statt Top-Level `ready/*`) | ja |
| `area/<id>/command` | **in** | wie `command`; Arm-Befehle wirken auf Bereich `id` | nein |

`state` (Top-Level) ist bei Multi-Bereich das **Schwere-Aggregat** (TRIGGERED >
ARMED_AWAY > ARMED_NIGHT > ARMED_HOME > DISARMED > unknown) — Single-Panel-Dashboards
und HomeKit funktionieren damit weiter. `command` (Top-Level) zielt weiterhin auf
Bereich 1. Arm-Befehle akzeptieren zusätzlich ein `area`-Feld (1–16) im JSON.
`ARM_NIGHT` gibt es nur für Bereich 1 (virtueller Nacht-Modus, globales Flag).

`<sensor.topic>` ist der pro Sensor in der Config hinterlegte relative Pfad (z.B.
`eg/esszimmer/bewegung`).

## Arm-Zustand (`state`)

`unknown` · `DISARMED` · `ARMED_HOME` · `ARMED_NIGHT` · `ARMED_AWAY` · `TRIGGERED`

- Wird **ausschließlich aus dem zyklischen Blockstatus** abgeleitet (Snapshot = Wahrheit),
  nie optimistisch aus einem gesendeten Befehl.
- `unavailable`/`unknown` bei Serial-Stille (Fail-Safe), nie veraltet „DISARMED".

## Command (`command`, eingehend)

JSON: `{ "cmd": "ARM_HOME|ARM_AWAY|ARM_NIGHT|DISARM|RESET|BYPASS_ON|BYPASS_OFF|OUTPUT_ON|OUTPUT_OFF", "pin": "…", "mb": n, "addr": a }`

`mb` (1–128) nur für `BYPASS_ON`/`BYPASS_OFF` (Meldebereich sperren/entsperren,
0x51/0xD1). **`BYPASS_ON` ist sicherheitsreduzierend** und folgt exakt der
Disarm-Policy (Opt-in-Schalter + PIN, fail-closed); `BYPASS_OFF` ist wie Arm immer
erlaubt. Der Gesperrt-Zustand kommt ausschließlich aus dem Readback
(`mb/<n>/bypassed`), nie optimistisch.

`addr` (GMS-Ausgangsadresse) nur für `OUTPUT_ON`/`OUTPUT_OFF` (Schaltausgang,
Meldungsart 0x00/0x80). Autorisierung über die **`switchable`-Allowlist der Config** (bewusste
Freigabe je Ausgang im physischen Setup, default aus) statt PIN — der Core verwirft
alles außerhalb fail-closed. Signalgeber als schaltbar zu markieren erzeugt eine
Validierungs-Warnung (Sirene wäre remote stumm-/einschaltbar). Schaltzustand =
Readback über das Sensor-Topic des Ausgangs; HA-Discovery legt je freigegebenem
Ausgang einen `switch` an (`optimistic: false`).

- **`retain=false` ist Pflicht** — ein mit retain empfangener Befehl wird verworfen + als
  Diagnose gemeldet (sonst stille Re-Zustellung bei jedem Reconnect).
- **PIN** wird firmware-seitig gegen NVS geprüft (Argon2/PBKDF2), mit **persistentem
  Lockout** (überlebt Reboot). Die PIN wird **nie geloggt**.
- **`DISARM` ist default-off** (fail-closed): nur wirksam, wenn Remote-Disarm im Setup
  explizit aktiviert wurde. Ohne gesetzte PIN ist der Command-Pfad komplett deaktiviert.
- `ARM_NIGHT` = intern scharf + virtuelles Night-Flag (NVS-persistiert).

**Befehls-Transaktion:** Befehle gehen nur in der Pause **nach dem zweiten
Statustelegramm** raus und werden bis zur Quittung der Zentrale verfolgt — Kollision mit dem
Panel-Poll oder ausbleibendes ACK führt zu begrenzter Wiederholung (3 Versuche, 3-s-Timeout),
danach zu sichtbarem Scheitern. Bei Serial-Ausfall werden wartende Befehle **verworfen**
(fail-safe — kein „Nachfeuern" nach Recovery). `command_result`-Werte:

| Payload | Bedeutung |
|---|---|
| `OK <Cmd>` | Zentrale hat den Befehl quittiert (Zustand kommt trotzdem nur aus dem Snapshot) |
| `REJECTED <Cmd>: …` | Pre-Arm-Gate: nicht scharfschaltbereit, gar nicht erst gesendet |
| `FEHLER <Code>` | Zentrale lehnte ab (0x11-Fehlersatz im CONFIRM_ACK) |
| `NAK <Cmd>` | Zentrale antwortete CONFIRM_NAK |
| `TIMEOUT <Cmd>: …` | keine Quittung nach allen Versuchen bzw. Serial offline → verworfen |

**HA-Steuerung (optional, opt-in):** ist der Command-Subscribe aktiv, publiziert das HA-Discovery-
Modul (`telenot-app::hadisco`) statt des read-only Alarmzustand-Sensors ein
`alarm_control_panel` mit `command_topic` = `command` und einem `command_template`, das HA-Aktion
+ Code in dieses generische `{cmd, pin}` übersetzt (`value_template: {{ value|lower }}` mappt den
`state` auf HA-States). Der Command-Topic selbst bleibt **HA-agnostisch** — jeder MQTT-Sender
(Node-RED, openHAB, Skript) kann ihn nutzen; HA-Discovery ist nur Plug-&-Play-Komfort obendrauf.

## Scharfschaltbereitschaft (Pre-Arm-Gate)

Die Anlage lässt sich nur scharfschalten, wenn sie es meldet — Panel-eigene Bits
`0x0535` (intern bereit) / `0x0536` (extern bereit), publiziert als `ready/intern` ·
`ready/extern`. Die Firmware **prüft das vor dem Senden**: ein `ARM_HOME`/`ARM_NIGHT` bei
`ready/intern=no` (bzw. `ARM_AWAY` bei `ready/extern=no`) wird **nicht gesendet**, sondern
mit `command_result = REJECTED … (offen: <Kontakte>)` quittiert. HA sollte den Arm-Button
anhand von `ready/*` ausgrauen. (Verifiziert am realen Mitschnitt: extern war bei offener
Garage durchgehend `no`.)

## Diagnostics (`diagnostics`)

JSON, u.a.: `serial` (`ok`|`stale`, `last_frame_ms`), `free_heap`, `largest_free_block`,
`reconnects`, `schema_version`, `firmware_version`, `time_unsynced`, `reset_reason`,
`uptime_s`. Dient der Feld-Diagnose ohne Web-UI.

## Verfügbarkeit / LWT

LWT setzt `availability=offline`. Nach (Wieder-)Connect wird `online` + frische States erst
publiziert, **nachdem** die Serial-Liveness bestätigt ist (kein blinder Retained-Republish).

## Home-Assistant-Integration (HA-first)

Der **Kern bleibt HA-agnostisch**; die HA-Spezifik sitzt in einem **Adapter**, nicht im Kern. Zwei
gleichwertige Wege konsumieren denselben v1-Vertrag:

- **A) On-Device-Discovery** (optionales Firmware-Modul, Default AUS): das Gerät publisht
  `homeassistant/<component>/<device_id>_<id>/config` (retained) → HAs **eingebaute** MQTT-Integration
  legt die Entities selbst an. Null Custom-Code. Klein & isoliert; rührt den Kern nicht an.
- **B) Custom HA-Integration** (Python/HACS, später): konsumiert den generischen v1-Vertrag, legt
  Entities an, mit Config-Flow/Geräteseite/Services. Firmware bleibt 100 % HA-agnostisch.

Beide brauchen dieselben Vertragsbausteine — daher sind sie hier **first-class**:

### Inventory / Manifest (`inventory`, retained)
Maschinenlesbare Liste ALLER Entities, damit HA (egal welcher Weg) den Entity-Satz nach der
Discovery kennt:
```json
{ "schema_version": 1, "device_id": "telenot-ab12",
  "entities": [
    { "id": "im_essen_eg", "name": "IM Essen EG", "platform": "binary_sensor",
      "device_class": "motion", "state_topic": "eg/essen/bewegung", "area": "Essen" },
    { "id": "akku", "name": "Akku", "platform": "binary_sensor", "device_class": "battery",
      "state_topic": "diag/akku", "entity_category": "diagnostic" }
  ] }
```

**Chunking (große Anlagen).** Bis **130 bestätigte Entities** bleibt das Verhalten unverändert:
genau EIN v1-Payload wie oben auf `inventory` (retained). Darüber wird auf `inventory` nur noch
ein kompakter **Envelope** publiziert und die Entities auf retained Chunk-Topics verteilt
(64 bestätigte Entities pro Chunk):

```json
{ "schema_version": 1, "chunked": true, "chunk_count": 4, "entities_total": 203 }
```

- `inventory/chunk/0` … `inventory/chunk/<chunk_count-1>` (retained): gleiche Struktur wie der
  v1-Payload, plus die Felder `"chunk": <i>, "chunk_count": <N>` direkt nach `device_id`
  (vor `entities`). Die festen Entities (Alarmzustand + Scharf-bereit) stehen genau einmal —
  in Chunk 0. `entities_total` zählt alle Entities über alle Chunks (inkl. der festen).
- **Konsumenten-Regel:** Maßgeblich ist `chunk_count` aus dem Envelope. Verwaiste ältere
  `inventory/chunk/…`-Topics (z. B. retained Reste von vor einem Reboot mit größerem Inventar)
  sind zu **ignorieren**. Schrumpft das Inventar innerhalb einer Laufzeit, räumt die Bridge die
  weggefallenen Chunk-Topics aktiv mit Leer-Payload; bei Wechsel zurück unter die Schwelle
  (chunked → single) ebenso alle Chunk-Topics.

### Device-Info (HA-Geräteregistrierung)
Jeder Discovery-Payload (Weg A) bzw. die Integration (Weg B) trägt einen `device`-Block:
```json
{ "identifiers": ["telenot-ab12"], "manufacturer": "Telenot",
  "model": "complex 400", "name": "Telenot Bridge", "sw_version": "<fw>" }
```

### Diagnose-Entities (Batterie, Netz, ÜG, Störungen)
Die Telenot liefert Akku-/Netz-/ÜG-Störung und Funk-Batterien **als Meldepunkte** — der Decoder
kennt die Meldungsarten (`StoerungAkku`/`Netz`/`Uebertragungsweg`). Sie werden als Entities mit
passender `device_class` publiziert: `battery`, `power`, `connectivity`, `problem`, `tamper`,
`safety`, `smoke`, `moisture` — mit `entity_category: diagnostic`, wo sinnvoll. → „Batteriestatus
rauskitzeln" ist nur ein weiterer Inventory-Eintrag, **keine Kern-Änderung**.
*(Kleiner Folgeschritt in `telenot-config`: `SensorKind`/`device_class`-Map um `battery`/`power`/
`connectivity` ergänzen.)*

### Alarm-Panel-Mapping (`alarm_control_panel`)
Unser `state` deckt sich mit HAs `alarm_control_panel`:

| v1 `state` | HA-Zustand |
|---|---|
| `DISARMED` | `disarmed` |
| `ARMED_HOME` | `armed_home` |
| `ARMED_NIGHT` | `armed_night` |
| `ARMED_AWAY` | `armed_away` |
| `TRIGGERED` | `triggered` |
| `unknown`/Serial-stale | `unavailable` |

Command-Mapping (MQTT `alarm_control_panel`): `payload_arm_home`/`_away`/`_night` → unser
`{cmd:ARM_*}`; `payload_disarm` → `{cmd:DISARM, pin:<code>}`. `code_arm_required=false`,
`code_disarm_required=true` (PIN nur fürs Unscharf). `ready/*` + `command_result` werden als
Attribute/Binary-Sensoren gespiegelt; ein abgelehntes Schärfen bleibt am Panel-Zustand sichtbar.

> **Grenze:** HAs transiente Zustände `arming`/`pending`/`disarming` bilden wir bewusst NICHT ab —
> der `state` kommt bestätigt aus dem Snapshot (confirm-via-reality), kein optimistisches „arming".

### Wo lebt was (Trennung)
| Belang | Ort |
|---|---|
| Protokoll, State-Machine, Fail-Safe, Commands | ESP32 `telenot-core` (HA-agnostisch) |
| Generischer Zustands-/Command-Vertrag | MQTT `telenot/v1/<dev>/…` (stabile Naht) |
| HA-Entities, device_class, alarm_panel, code-Handling | HA-Adapter (Weg A *oder* B) |

Damit kommt **nichts HA-Spezifisches in den Kern** — höchstens das optionale, isolierte
Discovery-Modul (Weg A), falls Zero-Config gewünscht ist.

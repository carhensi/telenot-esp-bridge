# REST-Contract `/api/v1` — Setup-Web ↔ Bridge

Wire-Vertrag zwischen der Preact-Setup-UI (`web/`) und dem Gerät. Implementiert
**transport-agnostisch** in `crates/telenot-app` (`api::dispatch`); die HTTP-Schale ist
austauschbar: Host = `telenot-sim serve` (tiny_http), Firmware = `esp_http_server`. Companion
zu [`MQTT-CONTRACT.md`](MQTT-CONTRACT.md) — wo sinnvoll werden dieselben JSON-Formen/Enums
wiederverwendet.

## Grundregeln

- **Nur im Setup-Modus.** Im Normalbetrieb existiert kein HTTP-Listener.
- **Polling, kein SSE/WebSocket.** Scan: alle **2 s** `GET /scan`; Diagnose/State: alle **5 s**.
  Kein Endpunkt hält eine Verbindung offen (Serial-Owner-Vorrang, RAM-Budget).
- **Session-Cookie** nach Login: `HttpOnly; SameSite=Strict; Path=/api` (TLS-Transport ergänzt
  `Secure`). Alle Routen außer `POST /session` und `GET /device` brauchen eine gültige Session.
- **CSRF:** jede mutierende Methode (POST/PUT/PATCH/DELETE) braucht Header `X-CSRF-Token`
  (Double-Submit gegen das beim Login gelieferte Token) **und** validierte gleiche Herkunft
  (`Origin`/`Host`). Sonst `403`.
- **Secrets write-only.** PIN und MQTT-Passwort gehen nur rein; GETs liefern nur `*_set`-Booleans.
- **First-Boot-Gate.** Solange `password_change_required` gilt, sind alle Routen außer
  `POST /session/password` und `DELETE /session` mit `403 password_change_required` gesperrt —
  der Default-Login muss erst gegen ein eigenes Passwort getauscht werden.
- **Disarm fail-closed.** `POST /command {cmd:DISARM, pin}` wirkt nur bei aktivem Remote-Disarm
  UND korrekter PIN (konstantzeit, Lockout nach Fehlversuchen); Arm/Reset sind pre-arm-gated.
- **Adressen** sind u16-**Zahlen** (kein Hex-String). Enums snake_case (= serde der Rust-Typen).
- **Fehler-Envelope:** `{ "error": { "code": "<stabil>", "message": "<lokalisiert>" } }` mit
  HTTP-Status. `code` ist maschinenlesbar/i18n-tauglich.

## Endpunkte

| Methode | Pfad | Body → Antwort | Screen |
|---|---|---|---|
| POST | `/session` | `{password}` → `{csrf_token, password_change_required, device}` + Set-Cookie; `401 invalid_password`; nach 5 Fehlversuchen `429 rate_limited {retry_after_s}` (Brute-Force-Bremse) | S1 |
| POST | `/session/password` | `{new_password (≥8), current_password?}` → `204`; `400 password_too_short`; bei Rotation eines bereits gesetzten Passworts ist `current_password` Pflicht (sonst `403 current_password_invalid`) | S1 |
| DELETE | `/session` | → `204` (Logout) | — |
| GET | `/device` | → `{model, fw, serial, mac, ip, schema, fingerprint}` (vor Login lesbar) | S0 |
| GET | `/connection` | → `{type:internal\|tcp, ip, port}` | S2 |
| PUT | `/connection` | `{type, ip?, port?}` → `204` | S2 |
| POST | `/connection/check` | `internal` → synchron `{state:ok\|error, last_frame_ms?, detail}`; `tcp` → `202 {state:checking}` (echte TCP-Probe, asynchron) | S2 |
| GET | `/connection/check` | → `{state:idle\|checking\|ok\|error, last_frame_ms?, detail}` (Poll bei tcp) | S2 |
| POST | `/scan/start` | → `202 {phase:belegt}`; (Singleton) | S3 |
| POST | `/scan/cancel` | `{keep_partial?}` → `{phase:idle\|done}` | S3 |
| GET | `/scan` | → `{phase, total, named, elapsed, remaining, current?, feed:[Sensor]}` (2 s-Poll) | S3 |
| GET | `/sensors` | → `{sensors:[Sensor], counts:{confirmed,unconfirmed,excluded}}` | S4 |
| PATCH | `/sensors/{addr}` | `{name?,name_ha?,kind?,polarity?,topic?,confirmed?,include?}` → `Sensor` | S4 |
| POST | `/sensors/bulk` | `{addresses:[u16], op, kind?, polarity?}` → `{updated}` | S4 |
| POST | `/sensors/{addr}/observe-polarity` | → `202` (passives Fenster; Ergebnis via `GET /state`) | S4 |
| GET | `/mqtt` | → `{host,port,tls,verify_cert,username,password_set,topic_root,device_id,ha_discovery}` | S5 |
| PUT | `/mqtt` | gleiche Felder + `password?` (leer = unverändert) → `204` | S5 |
| POST | `/mqtt/test` | → `202 {state:running}` (asynchron) | S5 |
| GET | `/mqtt/test` | → `{state:idle\|running\|done, ok?, detail?}` (`detail`: ok\|connect\|tls\|auth) | S5 |
| GET | `/security` | → `{pin_set, remote_disarm}` | S6 |
| PUT | `/security/pin` | `{pin}` (numerisch ≥4) → `{pin_set, weak}`; `400 pin_invalid` | S6 |
| PUT | `/security/remote-disarm` | `{enabled, acknowledged?}` → `204`; `409 pin_required` / `409 ack_required` | S6 |
| GET | `/diagnostics` | → `{serial,mqtt,heap?,firmware_version,schema_version,uptime_s,reset_reason}` (5 s) | S7 |
| GET | `/diagnostics/log?since_seq=N` | → `{entries:[{seq,t_ms,level,msg}], dropped}` | S7 |
| GET | `/state` | → `{arm_state, availability, intern_ready?, extern_ready?, sensor_states:[{address,active}], polarity_observed?}` (5 s / Header-Puls) | Header/S4 |
| GET | `/review` | → `{counts, mqtt_target, mqtt_tested, ha_discovery, remote_disarm, schema_version, warnings:[{code,severity,count,message}]}` | S8 |
| POST | `/commit` | `{warnings_acknowledged?, expected_sensors?, expected_confirmed?}` → `202 {rebooting:false}`; `400 invalid_config`; `409 warnings_unacknowledged/commit_pending/inventory_changed` | S8 |
| GET | `/commit` | `{state:"idle"|"pending"|"saved"|"failed", sensors?, message?}` — Speicherbestätigung des Owners | S8/HomeKit |
| POST | `/homekit/apply` | `202 {rebooting:false}`; gleicher Speicherauftrag wie `/commit`, danach HAP-Abgleich ohne Neustart; Abschluss über `GET /commit` | HomeKit |
| POST | `/command` | `{cmd:arm_away\|arm_home\|arm_night\|disarm\|reset\|bypass_on\|bypass_off\|output_on\|output_off, pin?, mb?, addr?}` → `202` (Live-Test-Board; PIN-Gate + Ausführung im Gerät; Disarm/Bypass-Sperren fail-closed, `mb` 1–512 nur für Bypass (Profil-Limit prüft der Core), `area?` 1–16 für Arm auf Multi-Bereich-Anlagen) | Live-Test |

## Sensor-Objekt

Spiegelt `telenot-config::Sensor` + **reine Ansichts-Felder** (nie persistiert):

```json
{
  "address": 66, "name": "MK Haustür", "name_ha": "MK Haustür",
  "kind": "magnetkontakt", "topic": "mk_haustuer",
  "polarity": "active_low", "confirmed": false,
  "raw_name": "MK Haustür", "dup_of": 1024,
  "status": "unconfirmed", "include": true
}
```

- `status` (`unconfirmed`|`confirmed`|`excluded`) und `include` sind **clientseitig abgeleitet**
  aus `confirmed` + dem In-Session-`excluded`-Set; beim Commit fallen ausgeschlossene Sensoren raus.
- `raw_name` = Original-Name der Anlage (read-only); `dup_of` = wahrscheinliche Doublette
  (Sabotage-/Doppelkontakt), heuristisch + reversibel.

## Warnungs-Codes (S8) → i18n

`sensor_unconfirmed`→`s8.w.unconf`, `polarity_unconfirmed`→`s8.w.pol`,
`mqtt_never_tested`→`s8.w.mqtt`, `remote_disarm_active`→`s8.w.remote`. Config-Fehler
(`empty_topic`/`empty_name`/`duplicate_address`/`schema_version_mismatch`) stammen aus
`Config::validate()` und teilen denselben Code-Raum (`telenot_config::IssueCode`).

## Sicherheits-Parität Host ↔ Firmware

Session/CSRF-Gate, fail-closed Disarm (`enabled && pin_set`), write-only Secrets und das
Verwerfen retainter Command-Messages sind **geteilte Logik** in `telenot-app`. Der Host bindet
per Default auf Loopback (Klartext-HTTP für Dev); die Firmware ergänzt selbstsigniertes HTTPS +
NVS/Argon2 + physisches Setup-Gate. Es gibt **keinen** Host-Bypass, den die Firmware nicht hätte.

## Live-Daten (kein Streaming)

`POST /mqtt/test`, `POST /connection/check` (tcp) und `POST /sensors/{addr}/observe-polarity`
sind **asynchrone Jobs**: der Aufruf startet, das Ergebnis wird per Poll abgeholt
(`GET /mqtt/test`, `GET /connection/check` bzw. `GET /state` → `polarity_observed`). So bleibt
der <3 s-Serial-ACK-Loop unberührt.

## Live-Test-Board (`/command`)

Ein optionales, vom Wizard **getrenntes** Steuer-Board (`GET /state` 2 s-Poll für offene Melder +
Alarmzustand, `POST /command` für Arm/Disarm). Disarm ist **fail-closed** (PIN-Gate bzw. Opt-in);
Arm ist im Core pre-arm-gegated. Auf dem Host läuft die Ausführung im Daemon, nie im Serial-Owner.

Speicheraufträge konsumieren die aktuelle Melderauswahl. Erst nach erfolgreichem Schreiben
meldet `/commit` den Zustand `saved`; bei Fehlern bleibt die Auswahl zur Korrektur erhalten.
`/reboot` weist laufende/fehlgeschlagene Speicheraufträge sowie neue ungespeicherte
Melderänderungen mit `409` ab. HomeKit-Apply verwendet denselben Ablauf.

Die Firmware schreibt ausstehende HA-Entity-Löschungen vor dem Config-Wechsel in ein
begrenztes NVS-Journal (maximal 1200 Einträge, je Adresse und Entity-Typ). Nach einem
Neustart werden noch benötigte Löschungen vor der HA-Discovery fortgesetzt. Ein Auftrag
verschwindet erst nach MQTT-PUBACK oder wenn die aktive Config die Entity wieder enthält.
Schreibfehler am Journal verhindern die Bestätigung eines neuen Config-Wechsels.

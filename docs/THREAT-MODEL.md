# Threat-Model

Die Bridge ist eine Komfort-/Monitoring-Anbindung an die Telenot complex 400 — **keine
Sicherheits-Komponente der EMA**. Fällt sie aus, arbeitet die Zentrale autark weiter.

## Was geschützt wird (und was nicht)

- **MQTT-Command-Pfad** (Remote-Arm/Disarm) ist gehärtet gegen einen kompromittierten oder
  böswilligen MQTT-Sender: TLS + Broker-Cert-Validierung, dedizierter least-priv MQTT-User,
  ACL strikt nur aufs Command-Topic, **PIN im Payload + persistenter NVS-Lockout**,
  `retain=false`-Pflicht.
- **NICHT geschützt** gegen physischen Zugriff auf die Verkabelung (UART zwischen ADM3251E
  und Zentrale) oder eine kompromittierte ESP32-Firmware. Das GMS-Disarm-Telegramm (`0xE1`)
  trägt am Protokoll **keine Authentisierung** — die Zentrale schaltet beim Empfang
  bedingungslos unscharf. **Die Bridge-PIN ist also ein Bridge-Gate, kein EMA-Schutz.**
- **Echte Disarm-Sicherheit** sitzt physisch in der Zentrale (comlock/Bedienteil). →
  Klärungspunkt mit Errichter: Lässt sich Remote-Unscharf an der GMS-Schnittstelle der
  complex 400 einschränken/abschalten?

## Konsequenzen im Design

- **Remote-Disarm default-off, fail-closed** (opt-in, PIN nötig). Arm/Home/Night default an.
- **Schlüssel low-value halten:** gerätespezifisches Cert/Creds, broker-seitig auf die
  eigenen `telenot/v1/<device_id>/`-Topics gescopt, einzeln widerrufbar. Ein extrahierter
  Key ist außerhalb seiner Topics wertlos.
- **Gehäuse-Integrität = EMA-Sabotagekontakt** ist die primäre physische Verteidigung →
  prüfen, ob die Bridge in den Sabotagekreis der Telenot einbezogen werden kann.
- **Restrisiko klassischer ESP32:** Flash-Encryption/Secure-Boot sind gegen Fault-Injection
  nur ein Speed-Bump → siehe „Schlüssel low-value" + Sabotagekreis.

## Geräte-Härtung (B6) — bewusst NICHT umgesetzt (Entscheidung 2026-07-12)

Secure Boot v2 + Flash-Encryption (release) + eFuse-Härtung (JTAG/UART-Download aus) wurden
geprüft und **bewusst verworfen**. Begründung:

- Der einzige reale Gewinn wäre **NVS-Verschlüsselung** (Plaintext-MQTT-Passwort at rest).
  Das Passwort geht an den **eigenen Broker im eigenen LAN** → low value; die Disarm-PIN ist
  ohnehin PBKDF2-gehasht, der HomeKit-Code regenerierbar.
- Secure Boot / JTAG-aus richten sich gegen einen **physischen** Angreifer — der ist beim
  EMA-Gerät bereits durch den **Sabotagekreis** abgedeckt (Gehäuse auf = Alarm).
- Selbst dieser Gewinn wäre nur ein Speed-Bump: Classic-ESP32 inkl. Rev v3 ist per
  Voltage-Fault-Injection aushebelbar (Espressif-Advisory, CVE-2023-35818 / AR2023-005).
- Der Preis wäre **unwiderruflich** (eFuses einweg, Kabel-Weg zugebrannt, OTA einziger
  Update-Pfad) ohne billigen Mittelweg: App-Level-Crypto ohne Flash-Enc ist Theater, der
  Key läge selbst im Flash.

→ **Restrisiko akzeptiert.** Primäre Verteidigung bleibt „Schlüssel low-value + Sabotagekreis".

Umgesetzt IST dagegen: **SHA-verifizierte A/B-OTA mit Bootloader-Rollback** (2026-07) und
Secrets nie in Config/Export. Wer trotzdem härten will, klont das Repo und signiert mit
eigenem Key (config-getrieben, Chip-Rev v3.0+ Pflicht) — kein Maintainer-Support dafür.

## Akzeptierte Restrisiken (Security-Review 2026-07-09)

Der Review der Setup-/Diagnose-Runde bestätigte die Kern-Invarianten (Command-Autorisierung
`Auth → match → nur bei Allow ausführen` in `app_loop`, plus Defense-in-Depth `disarm_enabled`
im Core; switchable-Allowlist fail-closed; keine Secrets in `command_result`/`/state`/
`/diagnostics`). Bewusst akzeptiert:

- **`GET /device` (öffentlich) trägt `configured: bool`** — verrät ohne Login, ob eine Config
  vorliegt. Kein Secret; die Web-UI braucht es VOR dem Login (Betriebs- vs. Erst-Setup-Einstieg),
  und die Route ist ohnehin nur im **physisch getriggerten Setup-Fenster** erreichbar (im
  Normalbetrieb kein HTTP-Listener). Kein Remote-Exposure.
- **`save_remote_disarm`/`save_conn`/`save_mqtt` ignorieren NVS-Schreibfehler** — schlägt ein
  Persist fehl, meldet die UI dennoch Erfolg; nach Reboot gilt wieder der alte NVS-Stand.
  Für den Remote-Disarm-Master ist das **fail-safe**: der Reboot-Default ist AUS
  (`load_remote_disarm` → `false` bei fehlendem/ungültigem Key). Availability-, kein
  Security-Problem (Config-Save selbst ist über den Commit-Pfad bereits fail-closed).

## Review-Historie 2026-07-11/12 (Voll-Review App + Pipeline)

Vollreview aller Crates, Web-UI und CI/CD (manuelle Tiefenprüfung der kritischen Pfade,
~20 Mio. lokale Fuzz-Runs). Die Kern-Invarianten (fail-closed Disarm mit einem
Autorisierungspunkt, Session/CSRF, defensive Parser, `forbid(unsafe_code)` in den
portablen Crates) wurden bestätigt. Behobene Lücken:

- CI-Security-Jobs ergänzt: cargo-deny/-audit, gitleaks (volle Historie, sauber),
  cargo-fuzz (3 Targets) + Proptests, `npm audit` fürs eingebettete Web-Bundle
- RUSTSEC-Bumps (crossbeam-epoch, anyhow); Actions SHA-gepinnt, least-privilege
  `permissions`, `persist-credentials: false`
- CString-NUL-Panik in `homekit.rs` (korrupter NVS-Code ⇒ Boot-Loop-DoS) behoben
- `overflow-checks = true` im Release; `publish = false` workspace-weit; SECURITY.md
- Einziger `dangerouslySetInnerHTML`-Einsatz ist das gerätegenerierte QR-SVG — akzeptiert

Offene Punkte (Stand Review-Ende): gerätespezifisches **Provisioning** (Sticker statt
bekanntem Initialpasswort), **Web-Frontend-Testabdeckung**, **HTTPS fürs Setup-Fenster**.
B6 bleibt bewusst verworfen (s.o.). A/B-OTA-Befund: Rollback existiert **nur** mit dem
esp-idf-Bootloader — der espflash-Default wertet `PENDING_VERIFY` nicht aus
(Kabel-Flash-Regeln: [HARDWARE.md](HARDWARE.md)).

**Re-Review-Trigger:** neue Netzwerk-Listener/Protokolle, Auth-Änderungen, OTA-Quelle ≠
manueller Upload (GitHub-Pull scharf schalten), Wechsel der Krypto-Primitiven, falls B6
doch je gewünscht.

## Setup-Window (Re-Entry)

Das Setup ist kein Einmal-Ereignis: für spätere Re-Konfiguration muss die Web-UI wieder
erreichbar werden — **ohne** die Kerneigenschaft „Angriffsfläche nur bei physischer Präsenz"
aufzugeben. Darum **kein** dauerhafter Server, sondern ein **zeitlich begrenztes, ausschließlich
physisch getriggertes Fenster**:

```
NORMAL (kein Listener)
   │  BUT1 Long-Press (~5 s, zur Laufzeit — kein Reboot, kein Monitoring-Ausfall)
   ▼
SETUP_WINDOW   Listener up · Timer (z.B. 15 min) · LED-Muster (GPIO32 — „Fläche offen")
   │  Timeout (fail-safe zu) │ „Fertig" in der UI │ Commit+Reboot │ erneuter Long-Press
   ▼
NORMAL (Listener wieder zu)
```

- **Trigger ausschließlich physisch** (BUT1/GPIO34) — **kein** MQTT-/Netzwerk-Pfad, der die
  Fläche remote öffnen könnte. Bewusste Entscheidung gegen ein Remote-Setup-Command.
- **Auto-Close** per hartem Timeout + UI-„Fertig" + Commit-Reboot; der Listener kann nicht
  versehentlich „an" bleiben.
- **Auth bleibt Pflicht:** das beim First-Boot gesetzte NVS-Passwort gilt weiter — **kein**
  Rückfall auf das Sticker-Initialpasswort. Rate-Limit/429 + CSRF wie im Erst-Setup.
- **Sichtbarkeit:** LED-Muster (GPIO32) zeigt physisch, dass der Server läuft.
- **Auffindbarkeit** (Gerät hat kein Display): beim Öffnen `diag/setup_window →
  {url, fingerprint, expires_s}` auf MQTT publishen (reines Outbound, kein Inbound-Trigger).
  Plus dauerhaft mDNS/DHCP-Hostname — siehe `docs/HARDWARE.md` („Netzwerk & Auffindbarkeit").

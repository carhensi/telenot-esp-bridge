# AGENTS.md

Arbeitsleitfaden für KI-Agenten und Mitwirkende. Erst das hier lesen, dann [`README.md`](README.md).

## Orientierung

Rust-Firmware-Appliance (ESP32-POE-ISO), die eine **Telenot complex 400** (Einbruchmeldeanlage)
über die serielle GMS-V6-Schnittstelle ausliest/steuert und über **HomeKit** und **MQTT** anbindet.
Es ist eine **Alarmanlage**: Im Zweifel immer die konservative, fail-safe Variante wählen —
lieber `unavailable` melden als einen falschen Zustand.

## Invarianten (nicht verhandelbar)

1. **`telenot-core` ist sans-IO.** Kein I/O, keine Zeitquelle, keine Hardware —
   `step(now, event) → Vec<Action>`. Die gesamte sicherheitskritische Logik ist hier und
   deterministisch host-testbar. Kein HTTP/MQTT/TLS in den Core.
2. **Snapshot = Wahrheit.** Der Arm-Zustand wird AUSSCHLIESSLICH aus dem zyklischen Blockstatus
   (0x24) abgeleitet — nie optimistisch aus einem gesendeten Befehl.
3. **Fail-Safe.** Verstummt der Serial-Strom (~9 s) → `unavailable`, nie veralteter Zustand.
4. **Disarm ist fail-closed.** Remote-Unscharf (MQTT wie HomeKit) nur bei explizitem Opt-in
   **und** gesetzter PIN.
5. **Serial-Owner-Vorrang.** Der Transport-Eigentümer (Task) hat absolute Priorität; HTTP/MQTT/HAP
   rufen NIE synchron in den Core — sie lesen den publizierten Snapshot und reichen Intents ein.
   Lange Operationen (MQTT-Test) laufen im Worker, nie im Serial-Loop.
6. **Polling, kein SSE.** Scan-/Diagnose-Fortschritt sind GET-Snapshots (RAM-/Socket-Budget).
7. **MQTT ist die einzige Naht zu HA.** Der Core kennt HA nicht; HA-Discovery ist ein optionales
   Modul in `telenot-app` (`hadisco`), kein Core-Belang.
8. **Secrets write-only.** PIN/MQTT-Passwort gehen nur rein (über den `Services`-Port), kommen nie
   in einer Response zurück (nur `*_set`-Booleans). PIN nie in Config-Export/Log.
9. **Config-Schema bleibt sauber.** UI-Konzepte (`status`/`include`/`raw_name`/`dup_of`) sind
   abgeleitete Ansicht — **nicht** in `telenot-config::Sensor` persistieren.

## Repo-Karte

```
crates/telenot-protocol  no_std, alloc-frei. FT1.2-Frames + VdS-2465-Records + Encoder. KEINE Aggregation.
crates/telenot-config    serde-Schema, validate() → Issue{code: IssueCode}, Migration, Legacy-Import.
crates/telenot-core      Core (Arm/Disarm/Fail-Safe/Command-Reducer) + Discovery. std, sans-IO.
crates/telenot-app       std, framework-FREI (keine tokio/http/mqtt/esp-Deps!). DTOs + api::dispatch +
                         Runtime + security (verify_pin/Lockout) + command (authorize_command) +
                         hadisco + inventory. Wird von Host UND Firmware geteilt.
crates/telenot-sim       Host-Binary. CLI: replay/tcp/discover/serve. serve = Daemon (tiny_http +
                         Mock/Replay/Tcp-Transport + rumqttc). I/O-Schale, NICHT firmware-geteilt.
crates/telenot-esp-bridge  esp-idf I/O-Schale. EIGENER Workspace (xtensa-Target, kein Root-Member).
                         Module: main (Boot) · app_loop (EMA-Poll + Intent-Routing) · homekit (HAP-FFI)
                         · transport (Uart/Tcp/Switchable) · storage (NVS-Secrets + cfg-Partition)
                         · mqtt · httpd · net (Ethernet LAN8710A).
web/                     Setup-Web (Preact). QUELLE = web/src/main.jsx (eine Single-File mit allem:
                         i18n, API, Components, Screens) + web/src/*.css. MOCK = Offline-Fallback.
docs/                    REST-/MQTT-Verträge, Threat-Model, Hardware, GMS-Coverage, Web-UI-Brief.
reference/               GITIGNORED. Echtdaten der laufenden Anlage (Configs, pcap-Korpus). Ground Truth.
```

## Bauen / Testen / Laufen

Host (Root-Workspace — all das muss vor jedem Commit grün sein):

```bash
cargo test --workspace                                   # alle Suiten grün
cargo clippy --workspace --all-targets -- -D warnings    # sauber
cargo fmt --all -- --check                               # sauber
cargo build -p telenot-app                               # MUSS std-only ohne Host-Deps bauen (Firmware-Proxy)
npm --prefix web run lint                                # ESLint (react-hooks); 0 Errors Pflicht
npm --prefix web run build                               # Setup-Web → web/dist/index.html
cargo run -p telenot-sim -- serve --ema mock --web web/dist/index.html --bind 127.0.0.1:8848
```

Firmware (eigener Workspace, xtensa — Workspace-grün beweist die Firmware NICHT mit):

```bash
cd crates/telenot-esp-bridge
source ~/export-esp.sh                 # espup-Toolchain-Env
cargo build --release                  # muss WARNING-FREI bauen

# Flashen: IMMER mit Partition-Table (cfg- + factory_nvs-Partition!), Micro-USB (CH340):
espflash flash --partition-table partitions.csv --port <PORT> \
  target/xtensa-esp32-espidf/release/telenot-esp-bridge
```

Die Sensor-Config liegt in der eigenen 128-KB-Partition `cfg` (Default-NVS mit 24 KB ist zu
klein); ohne `cfg`-Partition fällt die Firmware mit Warnung im Diagnose-Log auf die Default-NVS
zurück. `factory_nvs` hält den HomeKit-Pairing-Keystore.

## Speicher-Grundsätze (ESP32, ~150 KB freier Heap — Stabilität > alles)

Merksatz: **nie „alle ~150 Sensoren × irgendwas" gleichzeitig materialisieren.** Keine
Voll-Listen-JSONs (→ Pagination/Feed-Tail), keine Vec-Sammlungen über alle Melder
(→ Streaming/Callbacks wie `discovery_for_each`), keine Config-Klone im Hot-Path
(Config existiert genau 2×: Core + Setup-Liste; app_loop borgt via `runtime.config()`).
Große Antworten mit `ok_json_cap` vorbemessen (Vec-Verdopplung braucht alt+neu gleichzeitig).
Langlebige Puffer gebunden halten (RingLog: 256 Einträge × ≤200 Zeichen).

Bewusste NICHT-Optimierungen (geprüft — nicht „fixen"):

- `CONFIG_BUF` ist eine Obergrenze, kein Puffer (Exact-Size-Alloc via blob_len).
- KEINE `MBEDTLS_DYNAMIC_BUFFER`: spart nur zwischen Handshakes, erzeugt dafür
  Alloc/Free-Zyklen im TLS-Pfad → Fragmentierung.
- KEINE Task-Stack-Kürzungen (conn-probe 12 KB, mqtt-probe 24 KB, httpd 10 KB,
  mqtt-ev 8 KB): Stack-Overflow = stiller Reboot, Ersparnis klein/intermittierend.
- `LWIP_MAX_SOCKETS=16` bleibt (HAP braucht sie im Direkt-HomeKit-Modus).
- Web-Bundle streamt aus Flash (`include_bytes!` + 4-KB-Chunks) — kein Heap.

Frühwarnung statt Autopsie: `/diagnostics` liefert `boot_reason` (esp_reset_reason),
`boot_count` (NVS) und `heap_low`-Latch (Low-Water < 40 KB / größter Block < 32 KB);
gleiche Felder im MQTT-`diagnostics` → in HA alarmieren.

## Frontend-Workflow

`web/src/main.jsx` (+ `web/src/*.css`) IST die Quelle — direkt editieren. (Der frühere
Design-Handoff `web/design/` + `assemble.sh` ist entfernt.) Nach Web-Änderungen:
`npm --prefix web run build`, dann die Firmware neu bauen — sie bettet `web/dist/index.html`
per `include_bytes!` ein.

**e2e + Demo-Video:** `web/tests/e2e/speedrun.spec.js` fährt den ganzen Setup-Flow (Playwright)
gegen `telenot-sim serve` und nimmt ihn auf — Smoke-Test **und** Demo-Rohmaterial (Landscape +
Portrait-Reel). Reproduzierbares Rezept + Anpass-Tabelle: `web/tests/e2e/README.md`.

## Konventionen

- **Code-Kommentare auf Englisch**, knapp, „warum" statt „was". User-facing Strings
  (Fehlermeldungen, UI-Labels) und `docs/`/README bleiben Deutsch (deutsches Produkt).
- `clippy -D warnings` ist Pflicht; `cargo fmt` anwenden.
- `telenot-protocol` bleibt `#![no_std]`. `telenot-app` bleibt **frei von tokio/http/mqtt/esp** —
  diese Deps nur in den Binaries (telenot-sim / telenot-esp-bridge), hinter Traits (`MqttSink`,
  `Services`, `Transport`).
- Neue Validierungs-Befunde brauchen einen stabilen `IssueCode` (mappt auf Web-i18n `s8.w.*`).
- Verhalten gegen Realität prüfen: pcap-Korpus + `reference/` (NICHT raten). `reference/` und die
  echte Hausconfig sind privat → nicht committen, nicht in Tests/Fixtures hardcoden (Fixtures
  synthetisch halten).

## Stand & Baustellen

Erledigt (Details in der README-Statustabelle):

- **Phase A + Host-Appliance**: Protokoll/Core/App/Sim/Setup-Web; Protokoll-Layer vollständig
  gegen reale Mitschnitte verifiziert (pcap + Befehls-Telegramme, s. `reference/`). Command-Pfad
  HA→Bridge (Arm/Disarm/Bypass/Schaltausgänge, fail-closed + Lockout), GMS-Befehls-Transaktion
  (ACK-Fenster 3 s, max. 3 Retries, fail-safe Verwerfen), Zeitstellen (0x50).
- **Firmware auf Hardware**: Ethernet, HTTPS-Setup-Web, NVS-Secrets (PBKDF2 + persistenter
  Lockout), MQTT mit TOFU-Pinning, interne RS232 (MOD-RS232 am UEXT, GPIO4/36) + umschaltbarer
  Transport (RS232 ↔ TCP, live ohne Reboot).
- **HomeKit-Direkt-Epic**: HAP nativ (SecuritySystem + Melder per `show_in_homekit`),
  No-Reboot-Koexistenz (Web :80 / HAP :8080), `homekit_disarm` als Opt-in.

Offen:

- **Externe Montage + Distribution** (externe Box mit Sabotagekontakt, UEXT-Stack als
  Standard-Variante, Web-Flasher via esp-web-tools, Release-Binaries).
- **Live-Test-Board** (Web, S7): noch keine Buttons für Bypass/Schaltausgänge — der REST-/MQTT-Pfad
  kann es bereits (`command(cmd, pin, extra)` nimmt `mb`/`addr`).
- **Sicherungsbereiche 2–8** (Multi-Bereich): bewusst verschoben — braucht Design-Entscheidungen
  (Bereichs-Erkennung via Belegt-Scan vs. Config) und Verifikation an echter Mehr-Bereich-Anlage
  (unbenutzte Bereiche melden 0x9E wie echte disarmed-ready). Siehe [GMS-Coverage](docs/GMS-COVERAGE.md).
- **Härtung**: A/B-OTA (SHA-verifiziert) + Bootloader-Rollback umgesetzt (2026-07).
  Flash-Encryption / Secure Boot v2 **bewusst verworfen** (2026-07-12) — Restrisiko
  akzeptiert, s. [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md). Nicht erneut vorschlagen.

## Don'ts

- Keine REST-/HTTP-/MQTT-Logik in `telenot-core`.
- Keine Host-Deps (tokio/rumqttc/tiny_http/axum) in `telenot-app`.
- UI-Felder nicht ins persistente Config-Schema.
- Disarm nie „optimistisch"; Arm-Zustand nur aus dem Blockstatus.
- Firmware nie ohne `--partition-table partitions.csv` flashen (`cfg`-Config und
  HomeKit-Pairings hängen daran).
- Reale Hausdaten (`reference/`, echte Sensornamen/Adressen) nicht committen.

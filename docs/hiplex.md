# hiplex 8400H — Support-Roadmap (zweite Anlagen-Variante)

> **Stand 2026-09: Lesebetrieb (GMS plus) ist als Beta in der Firmware** — Community-Beitrag
> eines Testers mit realer 8400H, verifiziert per Mitschnitten und echtem Alarmtest:
> Bereich 1 liest ab Basis **0x0500** (nicht 0x0530, wie aus der complex abgeleitet), das
> Alarmbit speichert bis zum Rücksetzen, Discovery sammelt Eingangs-Belegtblöcke bis zur
> Antwortpause (kein separater 0x72-Block). Offen: Befehlsadressen (Gate
> `hiplex_cmds_verified` bleibt zu), GMS lite, weitere Bereiche, Bereit-Bits, andere
> FW-Stände. Offline-Nachweis für Tester: `verify_hiplex_capture` (telenot-sim-Example).

Additiv, kein Rewrite: Protokoll/Core/Config sind sauber getrennt — die Arbeit steckt im
**Adress-/Topologie-Modell** und in der **Capture-Verifikation**. Engpass ist **H0** (echter
Mitschnitt); ohne den ist alles Weitere Spekulation.

## Befund: gleiche Protokoll-Familie

Die hiplex spricht **dieselbe GMS-Familie** wie die complex — VdS-2465-Sätze, FT1.2-Framing,
SEND_NORM→CONF_ACK-Handshake, derselbe Befehls-Baukasten (Scharf/Unscharf, Meldebereiche
sperren/freigeben, Schaltaktionen). Quellen: Telenot KNX-400-IP-Handbuch §7.2/7.3, SecMap
complex/hiplex-Vergleich, Loxone-Forum „Loxone und Telenot" (Praxis: Lesen + Schalten per
rohem RS232 funktioniert), unser capture-verifiziertes complex-Protokollwissen.

`telenot-protocol`, der sans-IO-`telenot-core`-Ansatz und `telenot-app`/MQTT/HA-Discovery
bleiben. hiplex-spezifisch sind: das **Adressmodell** (Meldebereich- statt
Meldepunkt-zentriert), **Mehr-Bereich** (15 Sicherungsbereiche + Zentralen-Schutzbereich
statt heute Bereich 1, s. [GMS-Coverage](GMS-COVERAGE.md)), die **Baudrate** (GMS plus
= 115200 statt 9600 8N1) und mögliche **Protokoll-Drift ab hiplex-FW 12.xx** (am Capture
verifizieren). Variable 0x24-Payload-Längen je FW/Modus sind kein Problem — der Decoder
slict die Blockstatus-Bits bereits längen-dynamisch aus dem Satz-Header
(`telenot-protocol/src/record.rs`, `as_block_status`), keine festen Offsets.

### GMS lite vs. GMS plus

- **GMS lite** (ab Featurepaket **F06** — Prospekt DE-6100034-05 S. 3: „Interface KNX 400
  IP / GMS lite"): sendet **nur Meldebereiche**, keine Meldepunkte — der Errichter muss
  jedem Meldepunkt einen Meldebereich zuweisen.
- **GMS plus** (**hiplex-only**, die complex 400 kann es nicht): Meldepunkte zurück +
  Klartexte + Alarmierungstypen (Störung/Brand/…) je Punkt; MB-Zuordnung für die
  Visualisierung dann nicht mehr zwingend. Baudrate fix 115200.
  - ⚠️ **Einführungszeitpunkt widersprüchlich**: Quellen nennen „ab FW 13", „FW v11.07 /
    Featurepaket F12 mit rein softwareseitiger lite/plus-Umschaltung in hipas" und
    „FW 12/15". Der Prospekt (Stand F07) kennt nur GMS lite → plus kam später. Beim
    Tester per hipas-Screenshot verifizieren, nicht hart behaupten.
  - Behaupteter plus-Befehl (unverifiziert): einzelne **Meldepunkte extern
    abschalten/bypassen**, gültig bis zum nächsten Unscharfschalten → Kandidat für H5.

### Steuern vs. Lesen (Community-Aussagen, unverifiziert)

- **GMS lite ist primär Datengeber** (Visualisierung/Monitoring): Zustände von
  Sicherungsbereichen, Meldebereichen **und Alarmierungstypen** (Einbruch/Brand/Störung)
  kommen raus; aktives Steuern (Scharf/Unscharf) ist je nach Partnersystem **restriktiv
  blockiert oder nicht implementiert**.
- **Scharf-/Unscharfschalten** über GMS kollidiert schnell mit VdS-Richtlinien und muss in
  **hipas explizit freigeschaltet/autorisiert** sein (Errichter!). Beim Tester mit abfragen.
- Konsequenz für uns: hiplex-Befehle sind ohnehin hinter dem `hiplex_cmds_verified`-Gate
  (Adressen unverifiziert, s. H5); zusätzlich damit rechnen, dass unter lite Befehle mit
  FEHLER/NAK quittiert werden — die Bridge meldet das sichtbar (`command_result`),
  kein Stillschweigen. Lesen/Spiegeln funktioniert in beiden Varianten.

### Größenordnung (lt. Prospekt DE-6100034-05 S. 7/9 + SecMap)

**15 Sicherungsbereiche + 1 Zentralen-Schutzbereich** („15 + Z" — Datenmodell auf **16
Slots** auslegen; löst den 15-vs-16-Widerspruch der Quellen auf), **512 Meldebereiche**
(Prospekt bestätigt), ~2300 Zustände, bis 1200 Befehle, 1008 konventionelle Meldergruppen
im Vollausbau (SecMap: bis 2048 bei plus — Differenz am Capture klären), 64 Schaltaktionen.

---

## TODO

### H0 — Voraussetzung: echter hiplex-GMS-Mitschnitt  *(Blocker für alles Weitere)*

Werkzeug: der **Debug-Capture-Modus** ([`DEBUG-CAPTURE.md`](DEBUG-CAPTURE.md)) —
ein Tester klemmt das Gerät an, zeichnet im Setup-Web auf und schickt die `capture.bin`
(roher RX-Bytestrom, direkt `telenot-sim replay`-bar). Keiner der drei Modi schaltet die
Anlage; die Live-Zähler zeigen sofort, dass GMS-Daten ankommen.

- [ ] Jemanden mit hiplex 8400H + GMS-Schnittstelle gewinnen und den Capture-Modus fahren
      lassen (Arm extern/intern + Unscharf, ein paar MB-Auslösungen; im Discover-Lauf Namen).
- [ ] Metadaten abfragen: **GMS lite oder plus**, **Firmware-Version/Featurepaket**
      (idealerweise hipas-Screenshot der GMS-Einstellung), **Baudrate**, Anzahl
      Sicherungs-/Meldebereiche, welche Schaltaktionen extern steuerbar sind
      (Errichter-Programmierung).
- [ ] Capture + Notizen wie bei der complex unter `reference/` ablegen (gitignored, privat).
- [ ] **Baudrate im Capture-Modus konfigurierbar machen** — heute hart 9600 in
      `crates/telenot-esp-bridge/src/transport.rs`; ein GMS-plus-Tester (115200) könnte
      sonst gar nicht aufzeichnen. Kleiner Vorab-Fix, entschärft H7 teilweise.

### H1 — Protokoll-Verifikation gegen den Mitschnitt
- [ ] FT1.2-Framing/Prüfsumme gegen hiplex-Frames bestätigen (sollte identisch sein).
- [ ] **Kommen MB-Zustände zyklisch im 0x24-Block oder (nur) als spontane 0x02-Meldungen?**
      Entscheidet, ob MB-Sensoren snapshot-getrieben sein können (unser „Snapshot = Truth")
      oder einen Event-Pfad brauchen. Ebenso prüfen: Poll-Intervall (complex: 3 s —
      Liveness-Deadline hängt daran).
- [ ] VdS-2465-Satztypen abgleichen: gibt es ab FW 12.xx neue/abweichende Satztypen oder
      Meldungsart-Codes? (analog zur Softwarestand-Drift bei der complex).
- [ ] Belegt-Abfrage (0x10) + Text/Bereich/Meldebereich (0x0C/0x54) gegen hiplex prüfen.
- [ ] Falls Abweichungen: `telenot-protocol`-Decoder additiv erweitern (keine complex-Regression).

### H2 — Panel-Profil + Adressmodell
- [x] **Profil-Abstraktion**: `telenot-core/src/profile.rs` — `PanelProfile` als const struct,
      `COMPLEX400` exakt die pcap-verifizierten Literale (Gleichheits-Regressionstest).
- [x] `telenot-config`: `panel { kind, gms_variant, hiplex_cmds_verified, areas }` additiv,
      Alt-JSON lädt als complex400 (Roundtrip-Test).
- [x] **Provisorisches hiplex-Profil**: `profile/hiplex_provisional.rs` — Werte + Quelle je
      Konstante, s. Tabelle unten. Korrektur nach Capture = Constants-only-Change.
- [ ] Adresstabelle gegen echtes Capture verifizieren (H1), dann die `[ASSUMED]`-Marker
      auflösen.

#### Provisorische hiplex-Adresstabelle (Stand: vor erstem Capture!)

| Konstante | Wert | Quelle |
|---|---|---|
| Bereichs-Basis | 0x0530 | [§7 KNX-400-IP] complex-identisch **[ASSUMED für hiplex]** |
| Bereichs-Stride | 8 Bit | [§7] complex: 0x0530–0x0570 = 8 Bereiche × 8 Bit **[ASSUMED]** |
| Bereiche | 16 (15 SB + ZSB=16) | [Prospekt S. 7/9] |
| Bit-Offsets (unscharf/int/ext/alarm/bereit) | 0/1/2/3/5/6 | [§7, pcap-verifiziert auf complex] |
| MB-Status-Basis | 0x05B0 | **[ASSUMED]**: §7-Muster „direkt nach Bereichsblöcken" (0x0530+16×8) |
| MB-gesperrt-Basis | 0x07B0 | **[ASSUMED]**: nach MB-Status (0x05B0+512) |
| MB max | 512 | [Prospekt] |
| Schaltaktions-Basis | **unbekannt → Pfad deaktiviert** | [SecMap: 64 existieren; Adresse fehlt] |

Die Loxone-Forum-Schnipsel (`68 46 46 68 73 02 3a 24 …` / `68 60 60 68 73 02 54 24 …`)
belegen Framing + variable 0x24-Längen, enthalten aber **keine absoluten Adressen** —
falls im Fachforum konkrete hiplex-Adressen kursieren, hier einpflegen (Constants-only).

### H3 — Core: Multi-Bereich + Meldebereich-Modus
- [x] `arm_state`/`bereit` als **Bereich→Zustand-Map** (1–15 + ZSB=16): per-Area-Ableitung,
      `area/{id}/state`-Topics (nur Multi-Bereich; complex bit-identisch), Schwere-Aggregat
      auf `state`. Nacht-Modus bleibt Bereich-1-gebunden (virtuelles Flag ist global).
- [x] **Meldebereich-Modus** (GMS lite): Discovery synthetisiert „MB {n}"-Sensoren
      (`into_config_for`), MB-Bypass-Readback bis 512.
- [ ] **GMS plus**: Meldepunkte + Alarmierungstyp je Punkt auswerten (nach Capture).
- [x] Host-Tests: `crates/telenot-core/tests/hiplex.rs` — Bitpositionen aus den
      Profil-Konstanten berechnet (Adress-Korrektur invalidiert keine Tests). Tests gegen
      den echten Mitschnitt folgen mit H1.
- **HomeKit bleibt Single-Area**: ein SecuritySystem, spiegelt `arm_state()` (Bereich 1 bzw.
  Aggregat); Befehle zielen auf Bereich 1. Per-Area-HomeKit ist bewusst out of scope.
  MB-Sensoren erscheinen in HomeKit erst, wenn der Nutzer sie in S4 klassifiziert
  (Kontakt/Bewegung/…) und `show_in_homekit` setzt — gleiche Kette wie bei der complex.
- [x] **Kapazität für 512 MBs (Kompakt-Modell Stufe 1+2, 2026-07-18)**: `MAX_SENSORS = 600`.
  EIN Datenmodell für complex + hiplex: `SensorTable` (32-B-Records + geblockte
  String-Arena, ~100 B/Sensor), `Arc<Config>` geteilt zwischen Core und App, Edit-Kopie
  lazy (EditSession, beim Commit konsumiert), Persistenz als chunked A/B-postcard mit
  CRC + Legacy-JSON-Dual-Write (≤ 48 KB) für OTA-Rollback-Kompatibilität, Inventory
  chunked ab >130 Entities (MQTT-CONTRACT). Host-E2E mit 500 Sensoren verifiziert.
  **Stufe 3 (Strings lazy aus dem Flash, 1000+) bewusst geparkt** — erst relevant für
  GMS plus mit Meldepunkten; das chunked Format ist dafür bereits die Grundlage, die
  `SensorRef`-API kapselt den Wechsel. Hardware-Alternative: ESP32-POE-ISO-**WROVER**
  (8 MB PSRAM) löst die RAM-Wand komplett.

### H4 — Discovery für hiplex
- [ ] Belegt-/Text-Scan im Meldebereich-Modus; lite vs. plus automatisch erkennen (oder als
      Config-Fallback, falls zur Laufzeit nicht erkennbar).
- [ ] `Discovery::into_config` erzeugt MB- bzw. Meldepunkt-Sensoren je Variante.

### H5 — Befehle (Steuern)

**Alle hiplex-Befehle sind hinter dem Gate `panel.hiplex_cmds_verified`** (Default aus):
solange die Adressen nicht am Capture verifiziert sind, wird jeder adresstragende Befehl
sichtbar abgelehnt (`DENIED … hiplex-Befehlsadressen unverifiziert`). Lesen ist nie gated.

- [x] Scharf/Unscharf **je Sicherungsbereich** (`on_command_area`, `area/{id}/command`,
      REST/MQTT-`area`-Feld). ArmNight nur Bereich 1.
- [x] Meldebereich sperren/freigeben (0x51/0xD1) bis MB 512 — Adressen aus dem Profil.
- [x] Schaltaktionen (`on_schaltaktion`, gleiche Wire-Form wie Ausgänge) — **deaktiviert**,
      solange die Basisadresse unbekannt ist (`schaltaktion_base: None`), zusätzlich hinter
      Remote-Master-Switch + Gate.
- [ ] GMS plus (unverifiziert): **Meldepunkt-Bypass** (abschalten bis zum nächsten
      Unscharf) — am Capture verifizieren, dann wie MB-Sperren opt-in.
- [ ] Nach Capture: Gate-Freigabe-Protokoll fahren (s. Verifikation unten), Sirenen bleiben
      default-OFF wie bei der complex.

### H6 — Config / Setup-Web / HA
- [x] Setup-Web S2: Anlagen-Auswahl (complex/hiplex) + GMS lite/plus + Baud + rote
      Freigabe-Checkbox für das Befehls-Gate.
- [x] HA-Discovery: `alarm_control_panel` je Sicherungsbereich (+ read-only Aggregat);
      complex-Entity-Set bleibt byte-identisch (Test). MB-Sensoren laufen als normale
      `binary_sensor` über die bestehende Sensor-Discovery.
- [ ] Bereichs-Namen im Setup-Web pflegen (`panel.areas`) — bis dahin default „Bereich 1".

### H7 — Transport / Baud
- [x] Baudrate konfigurierbar (NVS `conn_baud`, 9600/115200, No-Reboot via
      `change_baudrate`) — damit sind auch GMS-plus-Captures möglich. Bei TCP stellt der
      USR-TCP232 die Baud selbst (UI-Hinweis).
- [ ] `telenot-sim serve --ema tcp:<hiplex-usr>` als Host-Testpfad (wie bei der complex).

### H8 — Verifikation
- [ ] `cargo test --workspace` grün inkl. neuer hiplex-Decoder-/Core-Tests gegen das Capture.
- [ ] Live gegen die echte hiplex: Zustände spiegeln, Arm/Disarm je Bereich, MB sperren,
      (opt-in) Schaltaktion — über das Live-Test-Board.
- [ ] Keine complex-Regression (beide Profile grün).

---

## Hinweis

Eine offizielle Schnittstellenbeschreibung ist **nicht nötig** — wie bei
der complex genügt ein realer Mitschnitt zum Reverse-Engineering (im Loxone-Forum bestätigt).

## Verifikations-Protokoll (nach dem ersten Capture)

1. Capture read-only fahren (Debug-Capture-Modus, Baud passend wählen).
2. 0x24-Layout gegen `crates/telenot-core/src/profile/hiplex_provisional.rs` diffen —
   Korrekturen sind **Constants-only** (Tests rechnen Bitpositionen aus dem Profil).
3. Zustände live verifizieren (Arm/Unscharf an der Anlage bedienen, Bridge muss spiegeln).
4. Erst dann `hiplex_cmds_verified` im Setup-Web setzen und je Befehlsklasse einmal in
   sicherem Anlagenzustand testen (Unscharf → MB-Sperre → Arm je Bereich).

## Referenzen
- `GMS-COVERAGE.md` — Mehr-Bereich ist dort als Roadmap markiert.
- `../AGENTS.md` — Architektur-Invarianten (gelten auch für hiplex: Profil ≠ Fork).
- Telenot KNX 400 IP, Techn. Beschreibung §7.1–7.3; SecMap complex/hiplex-Vergleich; Loxone-Forum.
- **Prospekt DE-6100034-05** „hiplex 8400H" (F07-Stand): 15 SB + Zentralen-Schutzbereich
  (S. 7/9), 512 Meldebereiche, GMS lite ab F06 (S. 3) — Beleg für die Profil-Topologie.

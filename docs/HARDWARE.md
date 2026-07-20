# Hardware-Contract

**Board:** Olimex **ESP32-POE-ISO-16MB** (klassischer ESP32, internes Ethernet/LAN8710A,
802.3af PoE, galvanisch isoliert). 16 MB Flash wegen 2×OTA + LittleFS + Secure-Boot-Overhead.
**RS232-Tap:** isolierter Transceiver **ADM3251E** direkt an der Telenot-GMS-Schnittstelle
(USR-TCP232-Modul entfällt). *Aktueller Aufbau: Variante C (MOD-RS232 am UEXT, GPIO4/36) —
siehe „RS232-Anbindung" unten.* **Keine RTC → NTP/SNTP.**

## Pin-Belegung (verbindlich, gilt für WROOM & WROVER)

| Funktion | GPIO | Hinweis |
|---|---|---|
| UART RX ← ADM3251E TXD | **GPIO35** | input-only zulässig für RX; ⚠️ hängt laut Olimex-Pinout am LiPo-Mess-Spannungsteiler (s.u.) |
| UART TX → ADM3251E RXD | **GPIO33** | vollfunktional nötig |
| Setup-Taster | **GPIO34** | vorhandener Olimex-Button (BUT1) |
| Status-LED | **GPIO32** | kollisionsfrei zu RMII/Strapping |
| Ethernet-PHY Power | **GPIO12** | **reserviert** — nie als GPIO/UART verplanen |
| Recovery-Flash | GPIO1/3 (UART0) | frei halten (Recovery-/Erst-Flash per Kabel) |

RMII-Ethernet belegt GPIO0 (clk), 12 (phy_pwr), 16/17/18/19/21/22/23/25/26/27.
Strapping-Pins (0/2/5/12/15) meiden. **GPIO33/35 sind frei** (nicht von RMII belegt) →
darum als UART gewählt.

> ⚠️ **GPIO35-Vorbehalt:** Auf dem ESP32-POE-ISO ist GPIO35 laut Olimex-Pinout-Cheatsheet
> fest mit dem **LiPo-Batterie-Spannungsteiler** verbunden. Ohne angeschlossene Batterie ist
> das nur eine hochohmige Last (RX funktioniert), **mit Batterie** treibt der Teiler gegen das
> UART-Signal. Da wir ohne LiPo betreiben (PoE), bleibt GPIO35 die Wahl — falls je eine
> Pufferbatterie dazukommt, auf **GPIO36 (UEXT-RXD, ebenfalls input-only)** ausweichen.

## RS232-Anbindung (Bauvarianten)

Der ESP kann nur 3,3-V-TTL-UART — zur Telenot-GMS (RS232, ±12 V) braucht es einen
**isolierten** RS232-Transceiver dazwischen. Zwei gleichwertige Wege, **beide galvanisch
getrennt**:

- **A) Custom-PCB mit ADM3251E** (Referenz): ein SOIC-Chip integriert RS232-Transceiver +
  Isolation + isolierte DC-DC (isoPower), nur ein paar 0,1-µF-Cs außenrum. Kompakteste Lösung,
  braucht aber PCB-Fertigung + SMD-Löten.
- **B) No-Solder-Feldvariante** (empfohlen für Nachbau): fertiges, isoliertes Modul
  **Waveshare „TTL TO RS232 (B)"** (hutschienen-tauglich, Schraubklemmen beidseitig, Anti-Surge,
  ~12–18 €). Kein SMD; am ESP nur **3-4 Drähte** an GPIO33/35/3V3/GND anlöten (oder Stiftleiste).
  **Kein eigenes Netzteil:** Das Modul wird komplett vom 3V3-Pin des ESP versorgt; ein
  isolierter DC/DC auf dem Modul erzeugt die galvanisch getrennte Spannung für die RS232-Seite
  selbst („requires no extra power supply for the isolated terminal", Waveshare-Wiki).
  Anschlussschema also identisch zu einem nicht-isolierten Pegelwandler — der Mehrwert ist
  die Trennstelle: Überspannung/Potentialunterschied/Surge von der Telenot-Leitung endet an
  der Isolationsbarriere statt im ESP, und umgekehrt.
- **C) Labor-/Prototyp-Variante (NICHT isoliert), AKTUELL BESTÜCKT:** **Olimex MOD-RS232**
  (~6 €, MAX3232) steckt per Flachbandkabel direkt auf den **UEXT** des Boards → UART liegt
  dann auf **GPIO4 (TXD) / GPIO36 (RXD, input-only)** statt 33/35. **Die Firmware ist aktuell
  auf diese UEXT-Pins konfiguriert** (`main.rs` → `UartTransport::new(uart1, gpio4, gpio36)`);
  für Variante A/B dort auf GPIO33/35 zurückstellen. Plug-and-play ohne Löten, aber
  **gemeinsame Masse ESP ↔ Telenot** — verletzt die Isolationsanforderung für den Endausbau.
  Im reinen PoE-Betrieb ist das vertretbar: PoE-DC/DC und Ethernet-Magnetics sind isoliert,
  der MOD-RS232-GND ist damit die *einzige* Massebindung des ESP (keine Schleife).
  > ⚠️ **Nie per USB flashen, solange die RS232 an der Zentrale hängt:** Der CH340 bindet
  > PC-/Schutzleiter-Masse an den ESP → Masseschleife über die Telenot. Vor dem Flashen
  > UEXT- oder RS232-Kabel ziehen.

**Verdrahtung (Variante B, ESP ↔ Modul-TTL-Seite):**

| ESP32-POE-ISO | → | Waveshare TTL |
|---|---|---|
| 3V3 | → | VCC — **3,3 V, nicht 5 V** (sonst 5-V-Pegel am ESP-RX) |
| GND | → | GND |
| GPIO33 (TX) | → | RXD |
| GPIO35 (RX, input-only) | ← | TXD |

Modul-RS232-Seite (TXD/RXD/GND) → Telenot-GMS, dort wo der USR-TCP232 saß. **Straight-through /
1:1-Kabel — KEIN Null-Modem, TX/RX NICHT kreuzen** (empirisch am laufenden Aufbau bestätigt,
2026-07-12: die complex-400-GMS-Buchse und die Modulseite matchen 1:1). Baud **9600 8N1**;
im Zweifel der Web-Schritt „Verbindung prüfen" (`reachable` = Frames kommen, `no_frame` = Port
da, aber keine Daten). Strom kommt komplett über **PoE** (kein Netzteil): nur LAN(PoE) +
3-Draht-Serial gehen ins Gehäuse.

## Provisioning-Sequenz (einmalig am Tisch, NICHT per OTA)

> ⚠️ **GPIO12 ist zugleich PHY-Power UND Flash-Spannungs-Strapping (MTDI).** Falsche
> Reihenfolge bricked das Modul.

1. Flash-VDD verifizieren (`espefuse.py adc_info`; WROOM = 3.3 V).
2. **`espefuse.py set_flash_voltage 3.3V` zuerst** brennen → Kaltstart verifizieren.
3. Custom Partition-Table flashen (s.u.).
4. ~~Secure Boot v2 + Flash-Encryption (release) + eFuse-Härtung~~ — **bewusst verworfen**
   (2026-07-12, Restrisiko akzeptiert; s. [Threat-Model](THREAT-MODEL.md)). Der A/B-OTA-
   Rollback läuft ohne eFuse über den esp-idf-Bootloader (PENDING_VERIFY).
5. Vor Einbau kurz abnehmen: Boot-Log zeigt Partition `ota_0`; ein Test-OTA bootet `ota_1`
   und ist nach dem 5-min-Self-Test „valid"; ein absichtlich panicendes Test-Image rollt
   automatisch zurück (Boot-Loop statt Rollback = falscher Bootloader!).

**Kabel-Flash-Regeln (A/B-OTA):** Immer den **esp-idf-Bootloader mitflashen**
(`--bootloader target/…/bootloader.bin`) — der espflash-Default-Bootloader wertet
`PENDING_VERIFY` nicht aus, mit ihm gäbe es stillschweigend **kein** Rollback. Der
Cargo-Runner tut das automatisch und löscht zusätzlich `otadata` (`--erase-parts otadata`),
damit nach einem OTA aus `ota_1` der nächste Kabel-Flash wieder aus `ota_0` bootet.
Ein Voll-Erase setzt NVS zurück (Passwörter, PIN, HomeKit-Pairing) — vorher in der
Diagnose eine **Sicherung exportieren** (Melder-Config + MQTT-Settings).

## Partition-Layout (16 MB)

`nvs` · `phy_init` · `otadata` · **`ota_0` / `ota_1`** (gleich groß) · **`factory_nvs`**
(HAP-Pairing) · **`cfg`** (Config-JSON, getrennt von `nvs`) · `coredump`. NVS bleibt
**unverschlüsselt** (B6 verworfen). Maßgeblich ist `partitions.csv`; CI-Guard prüft
Image-Größe gegen Slot-Größe; `opt-level="s"` + LTO.

## Einbau — empfohlene Montage: externe Box

**Das Gerät kommt NICHT ins EMA-Gehäuse**, sondern in eine eigene kleine Box direkt
daneben/darauf. Gründe:

- **Updates ohne Sabotagealarm** — solange es kein OTA gibt, heißt jedes Firmware-Update
  sonst: Gehäuse öffnen. Extern bleibt der Micro-USB (und später der Setup-Taster) erreichbar.
- **Keine PoE-Durchführung** ins zugelassene EMA-Gehäuse; nur die kurze RS232-Leitung verlässt
  die Zentrale über eine vorhandene Kabeldurchführung.
- **Recovery** (Erst-Flash, espefuse) jederzeit am Gerät möglich.

**Die eine Bedingung** (siehe [Threat-Model](THREAT-MODEL.md)): Das GMS-Disarm-Telegramm ist
am Draht **unauthentisiert** — wer die RS232-Leitung anfassen kann, schaltet die Anlage
unscharf, unabhängig von jeder Bridge-Einstellung. Deshalb:

- **RS232-Leitung kurz halten** (Box unmittelbar an der Zentrale, nicht quer durch den Raum).
- **Deckelkontakt der externen Box in den Sabotagekreis** der Telenot einschleifen
  (freie Meldegruppe) — damit ist die Schutzwirkung des EMA-Gehäuses wiederhergestellt.
- Arbeiten an der Verkabelung: EMA vorher **unscharf** schalten (sonst Sabotage-/Gehäusealarm).

### Stückliste (Variante C, „kaufen & stecken")

| Teil | ~Preis | Zweck |
|---|---|---|
| Olimex **ESP32-POE-ISO-16MB** | ~25 € | Board (PoE, isoliertes Ethernet, 16 MB) |
| Olimex **MOD-RS232** | ~6 € | RS232-Pegelwandler, steckt per Flachband auf den UEXT |
| Aufputz-/Hutschienengehäuse mit Deckelkontakt | ~10 € | externe Box + Sabotageeinbindung |
| Patchkabel zu PoE-Switch/-Injektor | — | Strom + Netzwerk in einem |

Kein Löten, zusammenstecken genügt. **Isolations-Hinweis:** Variante C ist nicht galvanisch
getrennt — im reinen PoE-Betrieb vertretbar (einzige Massebindung, s.o.), für den Endausbau
bleibt **Variante B** (isoliertes Waveshare-Modul, ~15 €) die Empfehlung.

## Netzwerk & Auffindbarkeit

Das Gerät hat kein Display — die Setup-Adresse muss auffindbar sein. **Zwei sich ergänzende
Mechanismen** (beide aktiv, weil je nach Netz/Client der eine oder andere greift):

| Mechanismus | Name | unabhängig von | gut für |
|---|---|---|---|
| **DHCP-Hostname** (Option 12 = `telenot-bridge`) | `telenot-bridge.fritz.box` / oft `telenot-bridge` | mDNS-Support des Clients | Windows/Linux; im Router sichtbar |
| **mDNS** (advertise `telenot-bridge.local`) | `telenot-bridge.local` | Router/lokalem DNS | Apple-Geräte; Netze ohne brauchbaren DNS |

- **mDNS** ist Multicast peer-to-peer (Router nicht beteiligt). Native auf macOS/iOS, vorhanden
  auf Win10/11, braucht `avahi` unter Linux. Bricht in Gäste-WLAN/VLAN-Isolation, die Multicast
  filtern — daher der DHCP-Hostname als zweiter Weg.
- **HTTPS-SAN:** `telenot-bridge.local` **und** `telenot-bridge` ins self-signed Cert als SAN
  aufnehmen, damit beim Zugriff über den Namen kein zusätzlicher Name-Mismatch zur (erwarteten)
  Self-signed-Warnung dazukommt. Fingerprint-Abgleich gegen den Sticker bleibt der Vertrauensanker.
- **Erst-Setup:** MQTT ist noch nicht konfiguriert → DHCP-Hostname (über die Fritz!Box) bzw. IP
  vom Sticker ist der Weg. Beim späteren Setup-Window kommt zusätzlich das MQTT-Publish dazu
  (siehe `docs/THREAT-MODEL.md`, „Setup-Window (Re-Entry)").

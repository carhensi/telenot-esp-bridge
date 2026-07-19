# GMS-V6 — Feature-Abdeckung & Roadmap

Das GMS-Protokoll der complex 400 ist aus öffentlichen Bausteinen plus realen
Mitschnitten rekonstruiert: FT1.2-Framing (IEC 60870-5), VdS-2465-Satzformat
(publizierter Standard), öffentliche Telenot-Dokumentation (KNX-400-IP Technische
Beschreibung, Prospekte) und als Ground Truth ein pcap-Mitschnitt der laufenden
Anlage plus 198 reale Befehls-Telegramme der Vorgänger-Bridge. Für den unterstützten
Umfang (1 Sicherungsbereich, binäre Melder, Monitoring + scharf/unscharf — s. README
„Unterstützter Umfang") ist alles Sicherheitsrelevante abgedeckt; offen sind
Mehr-Bereich und einzelne Steuer-Features, die der Protokoll-Layer bereits trägt
(`encode_command_02` ist adress-/meldungsart-generisch).

## ✅ Abgedeckt / bestätigt korrekt

- Framing (FT1.2) + Prüfsumme, VdS-2465-Satz-Walker.
- Satztypen dekodiert: 0x02, 0x24, 0x10, 0x0C, 0x50, 0x53, 0x54, 0x56, 0x11 (alle
  8 Fehlercodes). Neustart (0x53) → `event=restart` + Re-Sync des Belegt-/Bereichsstatus;
  im CONFIRM_ACK abgelehnte Befehle (Fehler-Satz 0x11) → `command_result = FEHLER <code>`.
- Arm/Disarm/Reset (Bereich 1), Bereitschafts-Gate, Fail-Safe, virtueller armed_night.
- **Befehls-Transaktion:** Senden nur in der Pause nach dem zweiten
  Statustelegramm, ACK-Tracking mit 3-s-Timeout, begrenzter Retry, `OK`/`NAK`/`TIMEOUT`
  als `command_result`; bei Serial-Ausfall werden wartende Befehle fail-safe verworfen
  (Payload-Details: [MQTT-Contract](MQTT-CONTRACT.md)). Ein eigener SEND_NORM-Zyklus
  vor dem SEND_NDAT ist **bewusst weggelassen** — 198 reale Befehls-Telegramme der
  Alt-Bridge belegen, dass die complex 400 direkte SEND_NDAT akzeptiert.
- **Schaltausgänge** (Meldungsart 0x00/0x80): `OUTPUT_ON/OFF` (REST+MQTT) mit fail-closed
  `switchable`-Allowlist (default aus, Signalgeber-Freigabe ⇒ Validierungs-Warnung);
  HA-`switch` je Freigabe mit Readback über den Sensor-Topic. Telegramm byte-identisch
  zu realen Referenz-Telegrammen getestet.
- **Zone-Bypass** (0x51/0xD1 @ 0x05F0+): `BYPASS_ON/OFF` (`mb` 1–128), Sperren fail-closed
  wie Disarm, Entsperren frei; Readback `mb/<n>/bypassed` aus 0x05F0+ (am realen Snapshot
  verifiziert: MB8). HA-Discovery-Entities folgen mit dem Inventory-Ausbau.
- **Datum/Zeit stellen** (0x50): volle Befehls-Transaktion, weicht
  Arm-Befehlen aus; die Firmware stellt nach SNTP-Sync/Zentralen-Neustart die
  **lokale** Zeit (Wochentag Mo=0).
- Auto-Discovery (Belegt-Scan → Namen), Systemstatus-Range 0x0530–0x066F (inkl.
  „Meldebereich gesperrt" 0x05F0–0x066F), HD44780-Textdekodierung.
- **V5/V6-Drift korrekt:** Diagnose=0x40, Warnung=0x33 (nicht altes 0x00-Modell).
- **Keine analogen Werte in V6** — complex 400 ist durchgängig bitcodiert; „Messwerte"
  sind VdS-Rahmen-Erbe und werden bewusst nicht „erfunden".
- comslave/Funk/comlock410(8–15)/MBT: reiner Adressraum → über Discovery generisch erfasst.

## 🗺️ Roadmap (für allgemeines Produkt / andere Anlagen)

Protokollseitig bereits möglich — es fehlt nur Core-/HA-Exposition + ggf. Status-Readback.

| Feature | Mechanismus | Aufwand | Priorität |
|---|---|---|---|
| **Sicherungsbereiche 2–8** — `arm_state`/`bereit` als `Bereich→…`-Map, Entity je Bereich; die **einzige echte Topologie-Lücke** | Bereichsstatus 0x0530+(N-1)*8 | mittel | hoch (strukturell) |
| Überfall/Bedrohung: Betreiber-Text (0x54) mitpublishen (Mechanismus 0x21 vorhanden) | Satztyp 0x54 | klein | mittel (mit comlock) |
| Ident-Nr. (0x56) als Diagnose („spricht wirklich meine Zentrale") | Satztyp 0x56 | klein | niedrig |
| comslave/Funk-Störbits als Diagnose (Akku/Netz/Funkstörung gebündelt) | Systemstatus-Adressraum | klein | niedrig |

## ❌ Bewusst weggelassen

- Analoge Mess-/Stellwerte (existieren nicht in V6).
- Topologie-Engine für Funk/comslave (compasX ist Quelle der Wahrheit; Discovery genügt).
- Summer-Einzel-Reset (0x0537) separat vom Alarm-Reset.

Offen ist die Verifikation am Gerät: voller Scan-Adressraum (inkl. comlock410 8–15,
comslaves), Bypass- und Schaltausgang-Roundtrip live (inkl. 0x11-Verhalten bei
abgelehnten Befehlen); Alt-Softwarestände < V5 senden Diagnose/Warnung als 0x00
(bei Fremdanlagen ein stiller Fehlerfall).

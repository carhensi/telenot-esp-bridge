# Security Policy

Die Bridge ist eine Komfort-/Monitoring-Anbindung an die Telenot complex 400 —
**keine Sicherheits-Komponente der EMA**. Details und Design-Entscheidungen:
[THREAT-MODEL.md](THREAT-MODEL.md).

## Unterstützte Versionen

Nur der jeweils letzte Release und `main`. Ältere Releases erhalten keine Fixes —
Update via Web-Flasher bzw. Release-Image.

## Schwachstelle melden

**Bitte keine öffentlichen Issues für Sicherheitslücken.**

- Solange das Repository privat ist: per E-Mail an den Maintainer
  (carsten@hensiek.com).
- Sobald das Repository öffentlich ist: bevorzugt über GitHub →
  **Security → Report a vulnerability** (Private Vulnerability Reporting).

Das ist ein Hobby-Projekt: Antwort und Fix erfolgen best-effort, in der Regel
innerhalb von 1–2 Wochen. Koordinierte Offenlegung nach Fix-Release wird begrüßt.

## Scope

- ESP32-Firmware (`crates/telenot-esp-bridge`)
- Portable Crates (`telenot-protocol`, `-config`, `-core`, `-app`, `-sim`)
- Setup-Web-UI (`web/`) und Web-Flasher (`flasher/`)
- Release-Pipeline (`.github/workflows/`)

**Out of scope:** die Telenot-Zentrale selbst, deren GMS-Protokoll (trägt
protokollseitig keine Authentisierung — siehe Threat-Model) sowie die
Netzwerk-Infrastruktur des Betreibers.

## Bekannte, bewusst akzeptierte Einschränkungen

Dokumentiert, damit sie nicht als „neue" Findings gemeldet werden
(Stand: Security-Review 2026-07-11; Details im Threat-Model):

- **Nur für vertrauenswürdiges LAN** gedacht — die Bridge darf nicht direkt
  ins Internet exponiert werden. Setup-HTTP ist unverschlüsselt, aber nur in
  einem physisch getriggerten, zeitbegrenzten Setup-Fenster erreichbar.
- **Initialpasswort `telenot-init` ist per Design öffentlich bekannt** (steht in
  README und Quellcode). Es funktioniert nur im lokalen Setup-Fenster, und bis zum
  **erzwungenen Passwortwechsel** beim First-Boot lässt die API keine andere Aktion
  zu. Gerätespezifisches Provisioning (Sticker) ist geplant.
- **MQTT-Passwort liegt unverschlüsselt im NVS** (die Firmware muss es zum
  Verbinden lesen). NVS-Verschlüsselung (via Flash-Encryption/Secure-Boot, „B6")
  wurde geprüft und **bewusst verworfen** — Restrisiko akzeptiert: das Passwort
  geht an den eigenen LAN-Broker (low value), Auslesen setzt physischen Zugriff
  voraus, der bereits durch den Sabotagekreis abgedeckt ist, und auf Classic-ESP32
  wäre Flash-Encryption ohnehin nur ein Speed-Bump (CVE-2023-35818). Details:
  [THREAT-MODEL.md](THREAT-MODEL.md).
- **Kein physischer Schutz:** Zugriff auf UART/Verkabelung oder den ESP32
  selbst wird nicht abgewehrt — primäre Verteidigung ist der Sabotagekreis
  der EMA.

## Checkliste bei Public-Stellung des Repos

- [ ] GitHub Secret Scanning + Push Protection aktivieren
- [ ] Private Vulnerability Reporting aktivieren (und diesen Text anpassen)
- [ ] CodeQL-Workflow evaluieren (Rust-Support-Reifegrad prüfen)
- [ ] GitHub Pages für den Web-Flasher aktivieren (Release-Workflow re-runnen)

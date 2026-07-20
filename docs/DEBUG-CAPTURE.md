# GMS-Mitschnitt (Debug fremder Anlagen)

Diese Anleitung ist für **Tester mit einer noch nicht unterstützten Telenot-Anlage**
(z.B. hiplex 8400H). Ein kurzer Mitschnitt des GMS-Datenstroms genügt, damit wir die Anlage
analysieren und Support dafür bauen können — ganz ohne dass wir selbst Zugriff auf die Hardware
brauchen.

> **Kurz:** anklemmen → Setup-Web öffnen → „GMS-Mitschnitt" → Modus wählen → 2–3 Min laufen
> lassen → `capture.bin` herunterladen → an den Entwickler schicken.

## Liest das etwas an meiner Anlage — oder schreibt es?

**Kein Modus schaltet die Anlage.** Nie scharf/unscharf, nie Bypass, nie ein Ausgang. Die drei
Modi unterscheiden sich nur darin, **wie viel das Gerät an die Anlage sendet** — von „gar nichts"
bis „reine Lese-Abfragen":

| Modus | Sendet an die Anlage | Wozu |
|---|---|---|
| **Nur lauschen** (Standard) | **nichts** | Zeichnet nur auf, was die Zentrale von sich aus schickt. Maximal sicher — das Gerät ist komplett stumm. |
| **Lauschen + Quittung** | nur die FT1.2-Quittung (ACK) | Hält den Link „gesund", damit die Zentrale mehr sendet. Keine Befehle. |
| **Discover-Lauf** | Lese-Abfragen (Belegt/Text) | Fragt zusätzlich Adressen/Namen ab → kartiert die Anlage. Reine Lese-Abfragen, **kein Schalten**. |

Empfehlung: **erst „Nur lauschen"**. Wenn dabei kaum Frames ankommen, „Lauschen + Quittung"
oder „Discover-Lauf" probieren.

Zur Sicherheit ist zusätzlich verankert: **solange ein Mitschnitt läuft, weist das Gerät jeden
Steuerbefehl** (REST, MQTT, HomeKit) **hart ab.** Es kann also währenddessen gar nichts schalten.

## Schritt für Schritt

1. **Anklemmen** — Gerät wie eine reguläre Bridge an die GMS-/RS232-Schnittstelle der Anlage
   (RS232, 9600 8N1; siehe [HARDWARE.md](HARDWARE.md)). Netzwerk/PoE dran.
2. **Setup-Web öffnen** — im Setup-Fenster (30 min nach dem Boot, sonst per Board-Taster BUT1)
   die Weboberfläche aufrufen, einloggen.
3. **Diagnose → „GMS-Mitschnitt"** — Modus wählen (Empfehlung „Nur lauschen"),
   **Mitschnitt starten**.
4. **Beobachten** — die Zähler sollten steigen: *Bytes*, *Frames ok*, und unter *Satztypen* die
   gesehenen GMS-Record-Typen (z.B. `0x24`). Steigt „Frames ok", kommen gültige GMS-Daten an.
5. **2–3 Minuten laufen lassen** — bei einer scharf/unscharf-Aktion an der Anlage (falls möglich
   und gefahrlos) sind die Daten für uns besonders wertvoll.
6. **Stoppen → `capture.bin` herunterladen.**
7. **Datei schicken** — plus ein paar Infos (siehe unten).

## Was steht in der Datei (Datenschutz)

- `capture.bin` ist der **rohe GMS-Empfangs-Datenstrom** deiner Anlage — dasselbe Format, das
  unsere Tools direkt abspielen (`telenot-sim replay capture.bin`).
- Im **Discover-Lauf** enthält der Mitschnitt zusätzlich die von der Anlage gemeldeten
  **Melder-/Bereichsnamen** (z.B. „Fenster Wohnzimmer"). Wenn dir das zu privat ist: „Nur
  lauschen" verwenden — dann sind keine Klartextnamen enthalten.
- **Keine** Geräte-Geheimnisse: keine Bridge-PIN, keine Passwörter, keine MQTT-Zugangsdaten.
  Diese verlassen das Gerät grundsätzlich nie.

## Diese Infos helfen uns zusätzlich

- Anlagentyp + Firmware-Version (z.B. „hiplex 8400H, FW 13.x")
- **GMS lite oder GMS plus?** (bei der hiplex relevant — siehe [hiplex.md](hiplex.md))
- Baudrate (complex: 9600; GMS plus: oft 115200)
- Anzahl Sicherungsbereiche/Meldebereiche, falls bekannt
- welcher Modus verwendet wurde und was du an der Anlage getan hast (Arm/Disarm, Fenster geöffnet …)

## Für uns: den Mitschnitt auswerten

```bash
# Dekodiert & interpretiert den Mitschnitt (Frames, Satztypen, abgeleiteter Zustand):
cargo run -p telenot-sim -- replay capture.bin

# Oder als komplette Anlage servieren (Setup-Web + REST gegen den echten Core):
cargo run -p telenot-sim -- serve --ema replay:capture.bin --web web/dist/index.html
```

Der `FrameDecoder` resynchronisiert sich selbst, ein am Anfang angeschnittenes Frame stört also
nicht. Ablage privater Mitschnitte: `reference/` (gitignored).

# Setup-Web (B5)

Einmalige Einrichtungs-Oberfläche der Telenot-Bridge. Läuft **nur im Setup-Fenster**
(30 min nach Boot, per Taster BUT1 erneut) — im Normalbetrieb existiert kein Webserver,
der Live-Status lebt in HomeKit/Home Assistant. Das gebaute Bundle wird per
`include_bytes!` in die Firmware eingebettet.

## Stack

- **Preact + preact/compat** — React-API, aber ~10 KB statt ~45 KB.
- **Vite** + `vite-plugin-singlefile` → **ein** selbst-enthaltenes `dist/index.html`
  (JS + CSS inline, keine externen Requests). Offline-tauglich, flash-freundlich.
- Kein Webfont, keine Runtime-Abhängigkeiten. System-UI-Font, oklch-Design-Tokens.

## Build

```sh
npm install
npm run build      # → dist/index.html  (~147 KB, ~40 KB gzip)
npm run dev        # lokaler Dev-Server
```

Der Löwenanteil der ~40 KB gzip sind die 119 eingebetteten Mock-Sensoren + DE/EN-i18n;
im Echtbetrieb kommen die Sensoren per API vom Gerät. Für 16 MB Flash unkritisch.

## Struktur

```
web/
  src/               Quelle:
    main.jsx         EIN Preact-Modul (i18n, MOCK, api, Components, Screens, App)
    *.css            Design-Tokens + Styles
  dist/index.html    Build-Ausgabe (Single-Bundle, gitignored) — wird von der
                     Firmware per include_bytes! eingebettet
```

`src/main.jsx` ist die **Quelle** — direkt hier editieren. Der frühere Design-Handoff
(`design/` + `scripts/assemble.sh`) wurde 2026-07 entfernt (Git-History bis Commit
`ac8c8fd`). Ein Aufteilen in echte ES-Module ist eine bewusste Refactor-Aufgabe.

## Screens (Wizard B5)

`S0` Gerät · `S1` Anmeldung (Initial-PW → eigenes PW) · `S2` Verbindung (interne UART
**oder** USR-TCP232) · `S3` Auto-Discovery-Scan · `S4` Sensor-Bestätigung (gruppieren,
Doubletten, Polarität, Bulk) · `S5` MQTT/Broker · `S6` PIN & Remote-Disarm (opt-in,
fail-closed) · `S7` Diagnose · `S8` Übernehmen → Reboot.

Light/Dark, DE/EN, responsive (Desktop-Tabelle / mobile Karten), keine Hersteller-Marke.

## Live-Anbindung (REST) + Offline-Fallback

`window.API` (in `main.jsx`) spricht die REST-API unter `/api/v1`
(Vertrag: [`../docs/REST-CONTRACT.md`](../docs/REST-CONTRACT.md)). `API.detect()` prüft
beim Start, ob ein Backend erreichbar ist: wenn ja (`live = true`), gehen Login, Scan,
Sensor-Edits, MQTT-Test, Diagnose und Commit ans Gerät (Session-Cookie + `X-CSRF-Token`);
wenn nein (z.B. `dist/index.html` als statische Datei geöffnet), läuft die UI offline
gegen `window.MOCK`. Alle Live-Pfade sind mit `ctx.live` geguarded.

### Gegen den Host-Daemon laufen (ohne Hardware)

```sh
npm run build                                   # Bundle bauen
cargo run -p telenot-sim -- serve \             # Daemon (im Repo-Root)
  --ema mock --web web/dist/index.html --bind 127.0.0.1:8848
# → http://127.0.0.1:8848  · Login: Initial-Passwort „telenot-setup"
```

`--ema mock` synthetisiert eine Anlage (Status + Discovery), `--ema replay:<bin>` spielt
einen Mitschnitt, `--ema tcp:<host:port>` verbindet die **echte Anlage via USR-TCP232**.
Hot-Reload: `npm run dev` mit `/api`-Proxy auf den Daemon (siehe `vite.config.js`).

**Bis nach Home Assistant** (Klartext-Broker, optional):

```sh
cargo run -p telenot-sim -- serve --ema tcp:<panel-ip:port> \
  --config reference/merged-config.json \
  --mqtt <broker-host>:1883 [--mqtt-user U --mqtt-pass P]
```

Publiziert live auf `telenot/v1/<device_id>/…` und schickt **HA-Discovery**-Configs
(retained) → HA legt die Entities automatisch an; `--no-ha-discovery` schaltet das ab.
TLS ist auf dem Host nicht verdrahtet — Klartext-Port nutzen; echtes TLS macht die Firmware.

## Firmware-Schale

`telenot-app` (Service-Schicht/Handler) wird von Host-Daemon **und** Firmware geteilt;
die Firmware ergänzt nur die HTTP-Schale (`esp_http_server`), NVS/Argon2-Secrets,
UART-Transport und das physische Setup-Gate.

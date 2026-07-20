# Speedrun-e2e + Demo-Aufnahme

`speedrun.spec.js` fährt den kompletten Setup-Flow im Browser gegen ein
`telenot-sim serve` (Mock-Anlage, **keine Hardware**) und nimmt ihn als Video auf.
Doppelter Zweck:

- **e2e-Smoke** — Boot → Login → Verbindung → Scan (~240 Melder) → Sensoren bestätigen
  → HomeKit (QR) → Security (PIN + Remote-Disarm) → Commit → Reload → Dashboard →
  intern scharf → **Alarm** → unscharf. Grün = der Flow lebt (mit Regressions-Checks für
  „Alle bestätigen", Command-Kollision, Geräteklassen).
- **Demo-Rohmaterial** — das aufgenommene Video → MP4/GIF für LinkedIn/README.

Zwei Formate als Playwright-Projekte: **`landscape`** (1280×900, Desktop) und
**`portrait`** (402×874, Mobile-UI-Reel).

## Voraussetzungen (einmalig)

```sh
cd web && npm install
npx playwright install chromium
```

## Aufnehmen

Der Sim hält seinen State im RAM → **jedes Format einzeln** aufrufen (so gibt's je Lauf
einen frischen, unkonfigurierten Sim). Vorher Sim-Binary + Web-Bundle bauen:

```sh
# aus dem Repo-Root:
cargo build --release -p telenot-sim
npm --prefix web run build

cd web
npm run e2e -- --project=landscape     # Video → test-results/**/video.webm
npm run e2e -- --project=portrait

# Sprache: SPEEDRUN_LANG=de npm run e2e -- --project=landscape   (Default: en)
```

Aufgenommen wird in **2× (Retina)** — Landscape 2560×1800, Portrait 804×1748
(`deviceScaleFactor` + `video.size` in `playwright.config.js`), sonst wirkt das
Video auf Mac/iPhone unscharf.

Playwright startet den Sim selbst (`webServer` in `playwright.config.js`) und stoppt ihn
danach. Für interaktives Debugging: `npm run e2e:headed` oder `npx playwright test --ui`.

## Video → MP4 + GIF (ffmpeg)

```sh
V=$(find test-results -path '*landscape*' -name '*.webm' | head -1)
ffmpeg -i "$V" -c:v libx264 -pix_fmt yuv420p -crf 20 -movflags +faststart out.mp4
# GIF (1.6× schneller, 800 px breit, palette-optimiert):
ffmpeg -i "$V" -vf "setpts=PTS/1.6,fps=12,scale=800:-1:flags=lanczos,palettegen" pal.png
ffmpeg -i "$V" -i pal.png -filter_complex \
  "[0:v]setpts=PTS/1.6,fps=12,scale=800:-1:flags=lanczos[x];[x][1:v]paletteuse" out.gif
rm pal.png
```

## Anpassen

| Was | Wo |
|---|---|
| Beispiel-Sensoren (Namen/Typen/Anzahl) | `crates/telenot-sim/src/serve.rs` → `default_seed()` |
| Geräteklassen-Ableitung aus dem Namen | `crates/telenot-config/src/lib.rs` → `guess_from_name()` |
| Alarm im Video auslösen | Spec: `POST /sim/intrude` (Sim-only, s. u.) |
| Tempo / „Beats" | `beat(page, ms)` im Spec bzw. `setpts` beim GIF |
| Auflösung / Seitenverhältnis | `viewport` + `video.size` je Projekt in `playwright.config.js` |
| Sprache im Video | Env `SPEEDRUN_LANG=en\|de` (Label-Tabelle `T` im Spec); Theme via `addInitScript` |
| Scan-Tempo | Env `TELENOT_SIM_TICK_MS` (Default 30 ms; Demo-Sim via `playwright.config.js`: 7 ms) |

## Sim-only Debug-Routen (nur im Host-Sim, **nie** in der Firmware)

Im tiny_http-Wrapper von `serve.rs` abgefangen, bevor an `api.rs` delegiert wird:

- `POST /sim/intrude` — „Einbruch": öffnet den Einbruchs-Melder (Seed[0]) → bei scharf
  wird `0x0533` aktiv → Core leitet `ArmState::Triggered` ab (Alarm).
- `POST /sim/calm` — Alarm-Flag zurücknehmen.

## Nicht committen

`test-results/` (Videos) ist gitignored; fertige `demo/`-Artefakte liegen außerhalb.

import { test, expect } from "@playwright/test";

// Speedrun: fully sets up the bridge (against telenot-sim --ema mock), shows HomeKit,
// switches to MQTT/HA (XOR control channel), arms home, triggers an alarm, and disarms.
// Serves as a demo recording AND e2e smoke test.
//
// Language via SPEEDRUN_LANG=en|de (default en); theme Dark (localStorage, set pre-load).
// Pacing: deliberately calm "beats" so a viewer can read each screen — the money shots
// (QR, sensor table, alarm hero) hold longest. The scan itself is fast (sim tick, see
// TELENOT_SIM_TICK_MS in playwright.config.js).

const LOGIN_PW = "telenot-setup"; // Sim-Initialpasswort (serve.rs)
const PIN = "1234";
const LANG = process.env.SPEEDRUN_LANG === "de" ? "de" : "en";

// UI labels per language (sources: src/i18n.js + src/dashboard.jsx).
const T = {
  en: {
    start: "Start setup",
    signIn: "Sign in",
    pwChangeTitle: "Set a new password",
    setPw: "Set password", // non-exact ("Set password & continue")
    check: "Check connection",
    next: "Next",
    findSensors: "Find sensors",
    reviewResults: "Review results",
    confirmAll: "Confirm all",
    toMqtt: "Switch to Home Assistant / MQTT",
    mqttActive: "MQTT / Home Assistant mode is active",
    remoteToggle: "Allow remote disarm",
    remoteAck: "I understand the risk",
    remoteActivate: "Enable remote disarm",
    commit: "Save & reboot",
    liveView: "Live view",
    armHome: "Arm home", // non-exact (composite segment name)
    alarmTitle: "Burglar alarm",
    alarmDisarm: "Disarm & reset",
    disarmConfirm: "Disarm",
  },
  de: {
    start: "Einrichtung starten",
    signIn: "Anmelden",
    pwChangeTitle: "Neues Passwort vergeben",
    setPw: "Passwort setzen", // non-exact („Passwort setzen & fortfahren")
    check: "Verbindung prüfen",
    next: "Weiter",
    findSensors: "Sensoren suchen",
    reviewResults: "Ergebnisse prüfen",
    confirmAll: "Alle bestätigen",
    toMqtt: "Auf Home Assistant / MQTT umstellen",
    mqttActive: "MQTT- / Home-Assistant-Modus ist aktiv",
    remoteToggle: "Remote-Disarm erlauben",
    remoteAck: "Ich verstehe das Risiko",
    remoteActivate: "Remote-Disarm aktivieren",
    commit: "Speichern & Neustart",
    liveView: "Live-Ansicht",
    // „Intern scharf" also appears as status text → match the unique segment subtitle.
    armHome: "Teilschutz",
    alarmTitle: "Einbruchalarm",
    alarmDisarm: "Unscharf & Zurücksetzen",
    disarmConfirm: "Unscharf schalten",
  },
}[LANG];

// "beat" pause: calm, readable pace for the recording.
const beat = (page, ms = 1200) => page.waitForTimeout(ms);

test("Setup-Speedrun + Alarm-Finale", async ({ page, baseURL }) => {
  // Default exact (the stepper has step buttons with the same names as "1 Sign in"); arm
  // segments have composite names → exact=false there.
  const btn = (name, exact = true) => page.getByRole("button", { name, exact });

  await page.addInitScript((lang) => {
    try {
      localStorage.setItem("tn_lang", lang);
      localStorage.setItem("tn_theme", "dark");
    } catch (e) { /* ignore */ }
  }, LANG);

  await page.goto("/");
  await beat(page, 3000); // landing page: let the viewer read the intro

  // ── S0 Boot ────────────────────────────────────────────────────────────
  await btn(T.start).click();

  // ── S1 Login ───────────────────────────────────────────────────────────
  await page.locator('input[type="password"]').first().fill(LOGIN_PW);
  await beat(page, 600);
  await btn(T.signIn).click();

  // After login: either forced password change (fresh sim) OR straight to S2 — wait robustly.
  const changeHeading = page.getByRole("heading", { name: T.pwChangeTitle });
  const s2check = btn(T.check);
  await expect(changeHeading.or(s2check).first()).toBeVisible({ timeout: 20000 });
  if (await changeHeading.isVisible().catch(() => false)) {
    const NEWPW = "DemoBridge#2026";
    const pw = page.locator('input[type="password"]');
    await pw.nth(0).fill(NEWPW);
    await pw.nth(1).fill(NEWPW);
    const setPw = btn(T.setPw, false);
    await expect(setPw).toBeEnabled({ timeout: 5000 });
    await beat(page, 800);
    await setPw.click();
  }

  // ── S2 Verbindung ──────────────────────────────────────────────────────
  await expect(s2check).toBeVisible({ timeout: 15000 });
  await beat(page, 1500); // show the connection screen before probing
  await s2check.click();
  const next = btn(T.next, true);
  await expect(next).toBeEnabled({ timeout: 15000 });
  await beat(page, 3000); // hold the green "connection OK" result
  await next.click();

  // ── S3 Scan (fast — sim ticks quickly, see TELENOT_SIM_TICK_MS) ────────
  await beat(page, 1500);
  await btn(T.findSensors).click();
  const toResults = btn(T.reviewResults);
  await expect(toResults).toBeVisible({ timeout: 60000 });
  await beat(page, 2500); // hold the "scan done, 242 found" summary
  await toResults.click();

  // ── S4 Sensors ─────────────────────────────────────────────────────────
  // Regression: device classes are derived from the name (not all "Unknown").
  await expect
    .poll(async () => page.locator("select").evaluateAll((sels) => sels.filter((s) => s.value === "bewegungsmelder").length), { timeout: 10000 })
    .toBeGreaterThan(0);
  await beat(page, 2000); // top of the sensor table
  // Slow scroll through the list — the 242 curated sensors are worth showing.
  for (let i = 0; i < 5; i++) {
    await page.mouse.wheel(0, 600);
    await beat(page, 800);
  }
  await beat(page, 1200);
  await page.mouse.wheel(0, -6000); // back to top
  await beat(page, 1000);
  await btn(T.confirmAll).first().click();
  await beat(page, 1800);
  await btn(T.next, true).click();

  // ── S5 Integration: HomeKit (default mode) — QR appears (sim demo pairing) ────────────
  await expect(page.locator(".hk-qr")).toBeVisible({ timeout: 15000 });
  await beat(page, 8000); // Pairing-QR + Code: longest hold — viewers may actually scan it
  // XOR control channel: in HomeKit mode the MQTT/REST channel (and thus the web
  // dashboard's arm buttons) is hard-blocked. Switch to MQTT/HA mode so the finale
  // (arm → alarm → disarm via web) is authorized — and the switch shows the XOR callout.
  await btn(T.toMqtt).click();
  await expect(page.getByText(T.mqttActive, { exact: false })).toBeVisible({ timeout: 5000 });
  await beat(page, 4000); // MQTT settings + exclusivity callout: let the viewer read it
  await btn(T.next, true).click();

  // ── S6 Sicherheit: PIN + Remote-Disarm ─────────────────────────────────
  await beat(page, 1500);
  const numeric = page.locator('input[inputmode="numeric"]');
  await numeric.nth(0).fill(PIN);
  await numeric.nth(1).fill(PIN);
  await beat(page, 800);
  // Enable remote disarm (required so the later disarm is authorized).
  const remoteToggle = page.getByText(T.remoteToggle, { exact: false }).first();
  if (await remoteToggle.isVisible().catch(() => false)) {
    await remoteToggle.click();
    const ack = page.getByText(T.remoteAck, { exact: false }).first();
    if (await ack.isVisible({ timeout: 3000 }).catch(() => false)) {
      await beat(page, 1500); // risk warning: worth reading
      await ack.click();
    }
    const activate = btn(T.remoteActivate);
    if (await activate.isVisible({ timeout: 3000 }).catch(() => false)) {
      await beat(page, 500);
      await activate.click();
    }
    await beat(page, 1500); // show "Remote-Disarm AKTIV"
  }
  await beat(page, 800);
  await btn(T.next, true).click();

  // ── S7 Review → Commit ─────────────────────────────────────────────────
  await beat(page, 3000); // summary screen: let the viewer skim it
  const commit = btn(T.commit);
  // Acknowledge open warnings (checkbox is a role=checkbox button).
  if (!(await commit.isEnabled().catch(() => false))) {
    const ackBox = page.getByRole("checkbox").last();
    if (await ackBox.isVisible().catch(() => false)) await ackBox.click();
  }
  await expect(commit).toBeEnabled({ timeout: 10000 });
  await beat(page, 1200);
  await commit.click();

  // ── Reboot → Reload → Dashboard ────────────────────────────────────────
  await beat(page, 4500); // show "rebooting" screen
  await page.reload();
  // After reload the device is configured → header shows "Live view"; switch to live view.
  const liveView = btn(T.liveView);
  await expect(liveView).toBeVisible({ timeout: 20000 });
  await beat(page, 1200);
  await liveView.click();
  const armHome = btn(T.armHome, false);
  await expect(armHome).toBeVisible({ timeout: 20000 });
  await beat(page, 3500); // dashboard overview before arming

  // ── Arm home ───────────────────────────────────────────────────────────
  await armHome.click();
  await beat(page, 5000); // let "armed" state settle and sink in
  // Regression: the arm command must NOT fail with collision/timeout/XOR-deny (no error badge).
  await expect(page.getByText(/Kollision|TIMEOUT|abgelehnt|gesperrt/i)).toHaveCount(0);

  // ── Trigger alarm (sim-only) → inline alarm hero ───────────────────────
  await page.request.post(`${baseURL}/sim/intrude`);
  await expect(page.getByText(T.alarmTitle, { exact: false })).toBeVisible({ timeout: 15000 });
  await beat(page, 9000); // alarm hero: the money shot

  // ── Disarm & reset → PIN → ends the alarm ──────────────────────────────
  await btn(T.alarmDisarm).click();
  const pinModal = page.locator('input[type="password"]').last();
  await beat(page, 1000);
  await pinModal.fill(PIN);
  await beat(page, 800);
  await btn(T.disarmConfirm, true).click();
  await beat(page, 6000); // calm end frame: disarmed, all green
});

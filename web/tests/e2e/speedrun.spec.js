import { test, expect } from "@playwright/test";

// Speedrun: fully sets up the bridge (against telenot-sim --ema mock), configures HomeKit,
// arms home, triggers an alarm, and disarms. Serves as a demo recording AND e2e smoke test.
// Language EN, theme Dark (via localStorage, set before loading).

const LOGIN_PW = "telenot-setup"; // Sim-Initialpasswort (serve.rs)
const PIN = "1234";

// Small "beat" pause for a calm, clearly visible speedrun pace in the recording.
const beat = (page, ms = 450) => page.waitForTimeout(ms);

test("Setup-Speedrun + Alarm-Finale", async ({ page, baseURL }) => {
  // Default exact (the stepper has step buttons with the same names as "1 Sign in"); arm
  // segments have composite names → exact=false there.
  const btn = (name, exact = true) => page.getByRole("button", { name, exact });

  await page.addInitScript(() => {
    try {
      localStorage.setItem("tn_lang", "en");
      localStorage.setItem("tn_theme", "dark");
    } catch (e) { /* ignore */ }
  });

  await page.goto("/");
  await beat(page, 800);

  // ── S0 Boot ────────────────────────────────────────────────────────────
  await btn("Start setup").click();

  // ── S1 Login ───────────────────────────────────────────────────────────
  await page.locator('input[type="password"]').first().fill(LOGIN_PW);
  await beat(page, 250);
  await btn("Sign in").click();

  // After login: either forced password change (fresh sim) OR straight to S2 — wait robustly.
  const changeHeading = page.getByRole("heading", { name: "Set a new password" });
  const s2check = btn("Check connection");
  await expect(changeHeading.or(s2check).first()).toBeVisible({ timeout: 20000 });
  if (await changeHeading.isVisible().catch(() => false)) {
    const NEWPW = "DemoBridge#2026";
    const pw = page.locator('input[type="password"]');
    await pw.nth(0).fill(NEWPW);
    await pw.nth(1).fill(NEWPW);
    const setPw = btn("Set password", false);
    await expect(setPw).toBeEnabled({ timeout: 5000 });
    await beat(page, 300);
    await setPw.click();
  }

  // ── S2 Verbindung ──────────────────────────────────────────────────────
  await expect(s2check).toBeVisible({ timeout: 15000 });
  await s2check.click();
  const next = btn("Next", true);
  await expect(next).toBeEnabled({ timeout: 15000 });
  await beat(page);
  await next.click();

  // ── S3 Scan ────────────────────────────────────────────────────────────
  await btn("Find sensors").click();
  const toResults = btn("Review results");
  await expect(toResults).toBeVisible({ timeout: 60000 });
  await beat(page, 700);
  await toResults.click();

  // ── S4 Sensors ─────────────────────────────────────────────────────────
  // Regression: device classes are derived from the name (not all "Unknown").
  await expect
    .poll(async () => page.locator("select").evaluateAll((sels) => sels.filter((s) => s.value === "bewegungsmelder").length), { timeout: 10000 })
    .toBeGreaterThan(0);
  await btn("Confirm all").first().click();
  await beat(page);
  await btn("Next", true).click();

  // ── S5 Integration: HomeKit (default mode) — QR appears (sim demo pairing) ────────────
  await expect(page.locator(".hk-qr")).toBeVisible({ timeout: 15000 });
  await beat(page, 1800); // Pairing-QR + Code zeigen
  await btn("Next", true).click();

  // ── S6 Sicherheit: PIN + Remote-Disarm ─────────────────────────────────
  const numeric = page.locator('input[inputmode="numeric"]');
  await numeric.nth(0).fill(PIN);
  await numeric.nth(1).fill(PIN);
  await beat(page, 250);
  // Enable remote disarm (required so the later disarm is authorized).
  const remoteToggle = page.getByText("Allow remote disarm", { exact: false }).first();
  if (await remoteToggle.isVisible().catch(() => false)) {
    await remoteToggle.click();
    const ack = page.getByText("I understand the risk", { exact: false }).first();
    if (await ack.isVisible({ timeout: 3000 }).catch(() => false)) await ack.click();
    const activate = btn("Enable remote disarm");
    if (await activate.isVisible({ timeout: 3000 }).catch(() => false)) await activate.click();
  }
  await beat(page);
  await btn("Next", true).click();

  // ── S7 Review → Commit ─────────────────────────────────────────────────
  const commit = btn("Save & reboot");
  // Acknowledge open warnings (checkbox is a role=checkbox button).
  if (!(await commit.isEnabled().catch(() => false))) {
    const ackBox = page.getByRole("checkbox").last();
    if (await ackBox.isVisible().catch(() => false)) await ackBox.click();
  }
  await expect(commit).toBeEnabled({ timeout: 10000 });
  await beat(page);
  await commit.click();

  // ── Reboot → Reload → Dashboard ────────────────────────────────────────
  await beat(page, 3000); // show "rebooting" screen
  await page.reload();
  // After reload the device is configured → header shows "Live view"; switch to live view.
  const liveView = btn("Live view");
  await expect(liveView).toBeVisible({ timeout: 20000 });
  await beat(page, 600);
  await liveView.click();
  const armHome = btn("Arm home", false);
  await expect(armHome).toBeVisible({ timeout: 20000 });
  await beat(page, 900);

  // ── Arm home ───────────────────────────────────────────────────────────
  await armHome.click();
  await beat(page, 2500); // let "armed" state settle
  // Regression: the arm command must NOT fail with collision/timeout (no red error badge).
  await expect(page.getByText(/Kollision|TIMEOUT|ersch/i)).toHaveCount(0);

  // ── Trigger alarm (sim-only) → inline alarm hero ───────────────────────
  await page.request.post(`${baseURL}/sim/intrude`);
  await expect(page.getByText("Burglar alarm", { exact: false })).toBeVisible({ timeout: 15000 });
  await beat(page, 2500);

  // ── Disarm & reset → PIN → ends the alarm ──────────────────────────────
  await btn("Disarm & reset").click();
  const pinModal = page.locator('input[type="password"]').last();
  await pinModal.fill(PIN);
  await beat(page, 300);
  await btn("Disarm", true).click();
  await beat(page, 2500);
});

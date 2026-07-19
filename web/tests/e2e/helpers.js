// Shared helpers for the functional security suite (*.e2e.spec.js). Runs against
// `telenot-sim serve --ema mock`; each test isolates itself via POST /sim/reset.

export const LOGIN_PW = "telenot-setup"; // Sim-Initialpasswort (serve.rs)
export const NEW_PW = "e2e-secret-2026";
export const API = "/api/v1";

// Resets the sim to a fresh initial state (password, session, config, OTA).
// Uses Playwright's request context (independent of the browser cookie).
export async function resetSim(request, baseURL) {
  const r = await request.post(`${baseURL}/sim/reset`);
  if (r.status() !== 204) throw new Error(`/sim/reset → ${r.status()}`);
}

// Set EN + Dark before loading (stable, language-independent selectors where possible).
export async function gotoApp(page) {
  await page.addInitScript(() => {
    try {
      localStorage.setItem("tn_lang", "en");
      localStorage.setItem("tn_theme", "dark");
    } catch (e) { /* ignore */ }
  });
  await page.goto("/");
}

// Login including first-boot mandatory password change. On a fresh device "Start setup" leads there;
// on a configured device login starts directly — both cases are handled.
export async function loginAndSetPassword(page) {
  const btn = (name, exact = true) => page.getByRole("button", { name, exact });
  const start = btn("Start setup");
  if (await start.isVisible().catch(() => false)) await start.click();
  await page.locator('input[type="password"]').first().fill(LOGIN_PW);
  await btn("Sign in").click();

  const heading = page.getByRole("heading", { name: "Set a new password" });
  await expectVisible(heading, 15000);
  const pw = page.locator('input[type="password"]');
  await pw.nth(0).fill(NEW_PW);
  await pw.nth(1).fill(NEW_PW);
  await btn("Set password", false).click();
  // Wait until the change screen is gone (S2 on fresh device, wizard/live on configured).
  await expectHidden(heading, 15000);
}

// Opens the diagnostics modal (backup + update panels live there). Requires a configured
// device (fixture config) → switch to live view first, then the serial status pill in the
// header opens DiagModal.
export async function openDiagnostics(page) {
  const live = page.getByRole("button", { name: "Live view", exact: true });
  if (await live.isVisible().catch(() => false)) await live.click();
  // Serial status pill (opens DiagModal via ctx.openDiag) — its title is "Status & log".
  const pill = page.getByRole("button", { name: /Serial/i }).first();
  await expectVisible(pill, 15000);
  await pill.click();
  await expectVisible(page.getByRole("heading", { name: /Status & log|Status & Log/ }), 10000);
}

// Fetches a CSRF token valid for the existing session from the server (GET /session
// returns a fresh one). Replaces the former access to the internal window.API.csrf.
export async function freshCsrf(request, baseURL, jar) {
  const r = await request.get(`${baseURL}${API}/session`, { headers: { Cookie: jar } });
  return (await r.json()).csrf_token;
}

async function expectHidden(locator, timeout) {
  const { expect } = await import("@playwright/test");
  await expect(locator).toBeHidden({ timeout });
}

async function expectVisible(locator, timeout) {
  const { expect } = await import("@playwright/test");
  await expect(locator).toBeVisible({ timeout });
}

import { test, expect } from "@playwright/test";
import { resetSim, gotoApp, loginAndSetPassword, freshCsrf, LOGIN_PW, NEW_PW, API } from "./helpers.js";

// Security core: login gate, first-boot mandatory password change, session/CSRF gate on the server.
// Runs against telenot-sim; beforeEach resets the sim to a clean state.

test.beforeEach(async ({ request, baseURL }) => {
  await resetSim(request, baseURL);
});

test("falsches Passwort wird abgewiesen, kein Session-Zugang", async ({ page }) => {
  await gotoApp(page);
  const startBtn = page.getByRole("button", { name: "Start setup", exact: true });
  if (await startBtn.isVisible().catch(() => false)) await startBtn.click();
  await page.locator('input[type="password"]').first().fill("falsch");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  // Stays on login (no password-change screen, no progression).
  await expect(page.getByRole("heading", { name: "Set a new password" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Sign in", exact: true })).toBeVisible();
});

test("First-Boot erzwingt Passwortwechsel vor allem anderen", async ({ page }) => {
  await gotoApp(page);
  const startBtn = page.getByRole("button", { name: "Start setup", exact: true });
  if (await startBtn.isVisible().catch(() => false)) await startBtn.click();
  await page.locator('input[type="password"]').first().fill(LOGIN_PW);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  // Immediately after first login the change screen MUST appear (not S2).
  await expect(page.getByRole("heading", { name: "Set a new password" })).toBeVisible({ timeout: 15000 });
  await expect(page.getByRole("button", { name: "Check connection", exact: true })).toHaveCount(0);
});

test("nach Passwortwechsel ist die Session gültig", async ({ page }) => {
  await gotoApp(page);
  await loginAndSetPassword(page);
  await expect(page.getByRole("button", { name: "Check connection", exact: true })).toBeVisible();
});

test("mutierender Endpoint ohne Session → 401", async ({ request, baseURL }) => {
  // Playwright's request context has NO session cookie → the server gate must enforce 401.
  const r = await request.put(`${baseURL}${API}/security/remote-disarm`, {
    data: { enabled: true, acknowledged: true },
  });
  expect(r.status()).toBe(401);
});

test("mutierender Endpoint mit Session aber ohne CSRF-Token → 403", async ({ page, request, baseURL }) => {
  // Establish a session via the UI (sets the HttpOnly cookie in the browser context) …
  await gotoApp(page);
  await loginAndSetPassword(page);
  // … then use the same cookie jar but WITHOUT sending an X-CSRF-Token.
  const cookies = await page.context().cookies();
  const jar = cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  const r = await request.put(`${baseURL}${API}/security/remote-disarm`, {
    headers: { Cookie: jar, Origin: baseURL, Host: new URL(baseURL).host },
    data: { enabled: true, acknowledged: true },
  });
  expect(r.status()).toBe(403);
  const body = await r.json();
  expect(body.error.code).toBe("csrf");
  // Counter-check: WITH a valid CSRF token the same request succeeds.
  const csrf = await freshCsrf(request, baseURL, jar);
  const ok = await request.put(`${baseURL}${API}/security/remote-disarm`, {
    headers: { Cookie: jar, Origin: baseURL, Host: new URL(baseURL).host, "X-CSRF-Token": csrf },
    data: { enabled: false, acknowledged: true },
  });
  expect(ok.status()).toBeLessThan(300);
});

// Reference NEW_PW/LOGIN_PW so unused imports don't lint.
test("Konstanten sind gesetzt", async () => {
  expect(LOGIN_PW.length).toBeGreaterThan(0);
  expect(NEW_PW.length).toBeGreaterThan(0);
});

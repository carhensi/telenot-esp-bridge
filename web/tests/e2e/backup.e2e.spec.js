import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { resetSim, gotoApp, loginAndSetPassword, openDiagnostics } from "./helpers.js";

// Backup: export must contain NO secrets (security property); import is a valid roundtrip.
// Panels live in DiagModal (configured fixture device).

test.beforeEach(async ({ request, baseURL }) => {
  await resetSim(request, baseURL);
});

test("Export enthält Config, aber keine Secrets", async ({ page }) => {
  await gotoApp(page);
  await loginAndSetPassword(page);
  await openDiagnostics(page);

  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page.getByRole("button", { name: "Sicherung exportieren" }).click(),
  ]);
  const json = JSON.parse(readFileSync(await download.path(), "utf8"));

  expect(json.backup_version).toBe(1);
  expect(json.config.sensors.length).toBeGreaterThan(0); // Fixture-Config ist drin
  // Secret exclusion (the actual assertion): no password, HomeKit code cleared.
  expect(json.mqtt.password).toBeUndefined();
  expect(json.mqtt.homekit_code).toBe("");
  expect(JSON.stringify(json)).not.toContain(NEW_PW_MARKER);
});

// We set the password to NEW_PW; it must not appear anywhere in the backup.
const NEW_PW_MARKER = "e2e-secret-2026";

test("Import einer Sicherung wird übernommen", async ({ page }) => {
  await gotoApp(page);
  await loginAndSetPassword(page);
  await openDiagnostics(page);

  // First export to obtain a guaranteed-valid backup …
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page.getByRole("button", { name: "Sicherung exportieren" }).click(),
  ]);
  const buf = readFileSync(await download.path());

  // … then import those exact bytes again (roundtrip). setInputFiles targets the hidden
  // <input>, not the button.
  await page.locator('input[accept="application/json,.json"]').setInputFiles({
    name: "telenot-backup.json",
    mimeType: "application/json",
    buffer: buf,
  });
  // Success message in the panel (shows the imported sensor count + secrets hint).
  await expect(page.getByText(/Import ok/i)).toBeVisible({ timeout: 10000 });
});

test("kaputte Datei wird als Importfehler abgewiesen", async ({ page }) => {
  await gotoApp(page);
  await loginAndSetPassword(page);
  await openDiagnostics(page);
  await page.locator('input[accept="application/json,.json"]').setInputFiles({
    name: "kaputt.json",
    mimeType: "application/json",
    buffer: Buffer.from("das ist kein json"),
  });
  await expect(page.getByText(/fehlgeschlagen/i)).toBeVisible({ timeout: 10000 });
});

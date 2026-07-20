import { test, expect } from "@playwright/test";
import { resetSim, gotoApp, loginAndSetPassword, openDiagnostics, freshCsrf } from "./helpers.js";

// Firmware update panel: valid fake image → ready_to_reboot; bad image → error.
// The sim mocks the flash write (serve.rs handle_ota_upload_mock) with the same checks
// as the firmware (magic 0xE9, length, SHA-256).

test.beforeEach(async ({ request, baseURL }) => {
  await resetSim(request, baseURL);
});

// Builds a minimal "app image": 0xE9 magic + version string in app_desc (offset 48),
// padded beyond the 64 KB minimum size (OTA_MIN_LEN).
function fakeImage(version = "9.9.9", size = 96 * 1024, magic = 0xe9) {
  const buf = Buffer.alloc(size);
  buf[0] = magic;
  buf.write(version, 48, "ascii"); // esp_app_desc.version @ 24+8+16
  return buf;
}

test("valides Image → ready_to_reboot mit Version", async ({ page }) => {
  await gotoApp(page);
  await loginAndSetPassword(page);
  await openDiagnostics(page);

  await page.locator('input[accept=".bin,application/octet-stream"]').setInputFiles({
    name: "telenot-esp-bridge-app-9.9.9.bin",
    mimeType: "application/octet-stream",
    buffer: fakeImage("9.9.9"),
  });
  const start = page.getByRole("button", { name: "Update starten" });
  await expect(start).toBeEnabled({ timeout: 10000 }); // waits for client-side SHA computation
  await start.click();

  // Success: reboot button + version hint appear.
  await expect(page.getByRole("button", { name: "Jetzt neu starten" })).toBeVisible({ timeout: 15000 });
  await expect(page.getByText(/9\.9\.9/)).toBeVisible();
});

test("kaputtes Image (falsches Magic) → Fehlermeldung", async ({ page }) => {
  await gotoApp(page);
  await loginAndSetPassword(page);
  await openDiagnostics(page);

  await page.locator('input[accept=".bin,application/octet-stream"]').setInputFiles({
    name: "kaputt.bin",
    mimeType: "application/octet-stream",
    buffer: fakeImage("0.0.0", 96 * 1024, 0x00), // kein esp-idf-Magic
  });
  const start = page.getByRole("button", { name: "Update starten" });
  await expect(start).toBeEnabled({ timeout: 10000 });
  await start.click();
  await expect(page.getByText(/Kein Firmware-App-Image/i)).toBeVisible({ timeout: 15000 });
});

test("Update-Check-Toggle schaltet die Einstellung", async ({ page, request, baseURL }) => {
  await gotoApp(page);
  await loginAndSetPassword(page);
  await openDiagnostics(page);

  const toggle = page.getByText(/Täglich auf Updates prüfen/i).locator("xpath=ancestor::label").locator('input[type="checkbox"]');
  await expect(toggle).toBeChecked(); // Default AN
  await toggle.click();
  // Verify against the server via the session (cookie in browser).
  const cookies = await page.context().cookies();
  const jar = cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  const csrf = await freshCsrf(request, baseURL, jar);
  await expect
    .poll(async () => {
      const r = await request.get(`${baseURL}/api/v1/ota`, { headers: { Cookie: jar, "X-CSRF-Token": csrf } });
      return (await r.json()).update_check;
    }, { timeout: 5000 })
    .toBe(false);
});

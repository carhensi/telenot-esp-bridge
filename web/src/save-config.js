// Reboot only after the serial owner has acknowledged durable storage.
export async function saveAndReboot(api, total, confirmed, wait = ms => new Promise(resolve => setTimeout(resolve, ms))) {
  if (api.sensorEditsPending) throw new Error("Melderaenderungen werden noch gespeichert. Bitte warten.");
  await api.commit(true, total, confirmed);
  await waitForCommit(api, total, wait);
  await api.reboot();
}

export async function waitForCommit(api, total, wait = ms => new Promise(resolve => setTimeout(resolve, ms))) {
  for (let attempt = 0; attempt < 120; attempt++) {
    await wait(500);
    const status = await api.getCommit();
    if (status.state === "failed") throw new Error("Speichern fehlgeschlagen: " + status.message);
    if (status.state === "saved") {
      if (total !== undefined && status.sensors !== total) throw new Error("Gespeicherte Melderzahl stimmt nicht. Kein Neustart.");
      return;
    }
    if (status.state !== "pending") throw new Error("Speicherauftrag nicht mehr vorhanden. Kein Neustart.");
  }
  throw new Error("Speichern noch nicht bestätigt. Kein Neustart. Bitte Diagnose-Log prüfen.");
}

// Diagnostics modal: status/ring log + backup + firmware update + GMS capture + about.
import React from 'preact/compat';
const { useState, useEffect, useRef } = React;
import { API } from './api.js';
import { MOCK } from './mock.js';
import { Button, fmtDur, fmtUptime, Icon, Modal, StatusDot } from './ui.jsx';

// Project identity for the about panel (repo link doubles as the OTA release source).
const PROJECT = {
  name: "telenot-esp-bridge",
  repo: "https://github.com/carhensi/telenot-esp-bridge",
  license: "Apache-2.0",
  author: "Carsten Hensiek",
  email: "carsten@hensiek.com",
};


function DiagModal({ ctx, onClose, initialCapture }) {
  const { t, dev } = ctx;
  const [diag, setDiag] = useState(MOCK.diagnostics);
  const [log, setLog] = useState(MOCK.log);
  const [copied, setCopied] = useState(false);
  const [autoref, setAutoref] = useState(true);
  const lvlcls = (l) => "ringlog__lvl ringlog__lvl--" + l;

  // Live: poll diagnostics + ring log every 5s (while auto-refresh is on).
  useEffect(() => {
    if (!(ctx.live && API) || !autoref) return;
    let stop = false;
    const tick = () => {
      API.getDiagnostics().then((d) => { if (!stop) setDiag(d); }).catch(() => {});
      API.getLog(0).then((r) => { if (!stop) setLog(r.entries || []); }).catch(() => {});
    };
    tick();
    const id = setInterval(tick, 5000);
    return () => { stop = true; clearInterval(id); };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- poll keyed on the autoref toggle; ctx.live is session-constant
  }, [autoref]);

  const serialOk = diag.serial && diag.serial.status === "ok";
  const mqttStatus = diag.mqtt ? diag.mqtt.status : "connecting";
  const mqttTone = mqttStatus === "ok" ? "ok" : mqttStatus === "off" ? "unavail" : mqttStatus === "connecting" ? "warn" : "alarm";
  const mqttLabel = mqttStatus === "ok" ? t("st.ok") : mqttStatus === "off" ? t("st.off") : mqttStatus === "connecting" ? t("st.connecting") : t("st.error");

  const copyLog = () => {
    const txt = log.map((e) => `[${e.seq}] ${e.level.toUpperCase()} ${e.msg}`).join("\n");
    navigator.clipboard && navigator.clipboard.writeText(txt);
    setCopied(true); setTimeout(() => setCopied(false), 1600);
  };
  return (
    <Modal open onClose={onClose} wide title={t("s7.title")} icon="server"
      footer={<Button variant="primary" onClick={onClose}>{t("btn.close")}</Button>}>
      <div className="stat-grid" style={{ marginBottom: "var(--space-4)" }}>
        <div className="metric"><div className="metric__lbl">{t("s7.serial")}</div><div className="metric__val metric__val--flex"><StatusDot tone={serialOk ? "ok" : "alarm"} live={serialOk} /> {serialOk ? t("st.ok") : t("st.error")}</div></div>
        {ctx.mqtt && ctx.mqtt.homekit_mode ? (
          <div className="metric"><div className="metric__lbl">{t("s7.homekit")}</div><div className="metric__val metric__val--flex"><StatusDot tone="ok" /> {t("st.ok")}</div></div>
        ) : (
          <div className="metric"><div className="metric__lbl">{t("s7.mqtt")}</div><div className="metric__val metric__val--flex">
            <StatusDot tone={mqttTone} /> {mqttLabel}
            {ctx.mqtt && ctx.mqtt.tls && mqttStatus === "ok" && <Icon name="shield" size={13} style={{ color: "var(--state-ok-fg)" }} title="TLS" />}
          </div></div>
        )}
        <div className="metric"><div className="metric__lbl">{t("s7.heap")}</div><div className="metric__val mono">{diag.heap ? (diag.heap.free / 1024).toFixed(0) + " KB" : "—"}</div></div>
        <div className="metric"><div className="metric__lbl">{t("s7.heaplow")}</div><div className="metric__val mono" style={diag.heap_low ? { color: "var(--state-warn-fg)" } : undefined}>{diag.heap && diag.heap.low != null ? (diag.heap.low / 1024).toFixed(0) + " KB" : "—"}</div></div>
        <div className="metric"><div className="metric__lbl">{t("s7.bootreason")}</div><div className="metric__val mono">{diag.reset_reason || "—"}</div></div>
        <div className="metric"><div className="metric__lbl">{t("s7.boots")}</div><div className="metric__val mono">{diag.boot_count || "—"}</div></div>
        <div className="metric"><div className="metric__lbl">{t("s7.uptime")}</div><div className="metric__val mono">{diag.uptime_s != null ? fmtUptime(diag.uptime_s) : "—"}</div></div>
        <div className="metric"><div className="metric__lbl">{t("s7.fw")}</div><div className="metric__val mono">{dev.fw}{dev.fw_build ? ` · ${dev.fw_build}` : ""}</div></div>
      </div>
      <div className="card"><div className="card__head" style={{ justifyContent: "space-between" }}>
        <span className="card__title card__title--sm">{t("s7.ringlog")}</span>
        <div className="cluster">
          <label className="toggle" style={{ gap: "var(--space-2)" }}><span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("s7.autorefresh")}</span>
            <input type="checkbox" checked={autoref} onChange={(e) => setAutoref(e.target.checked)} /><span className="toggle__track"><span className="toggle__thumb" /></span></label>
          <Button variant="secondary" size="sm" iconLeft="copy" onClick={copyLog}>{copied ? t("btn.copied") : t("s7.copylog")}</Button>
        </div>
      </div>
        <div className="card__body card__body--compact">
          <div className="ringlog scroll">
            {log.map((e) => (
              <div className="ringlog__row" key={e.seq}>
                <span className="ringlog__t">{fmtDur(Math.floor((e.t_ms || 0) / 1000))}</span>
                <span className={lvlcls(e.level)}>{e.level}</span>
                <span className="ringlog__msg">{e.msg}</span>
              </div>
            ))}
            {log.length === 0 && <div className="ringlog__row" style={{ color: "var(--fg-subtle)" }}>…</div>}
          </div>
        </div>
      </div>
      <BackupPanel ctx={ctx} />
      <UpdatePanel ctx={ctx} setupWindowS={diag.setup_window_s_remaining} />
      <CapturePanel ctx={ctx} initialOpen={initialCapture} />
      <AboutPanel ctx={ctx} />
    </Modal>
  );
}

/* Settings backup: export/import configuration as a JSON file. Contains NO secrets
   (PIN, passwords, HomeKit pairing) — those must be re-entered after an import.
   Primarily a safety net for the one-time partition migration (OTA restructure). */
function BackupPanel({ ctx }) {
  const [msg, setMsg] = useState(null); // { tone: "ok"|"alarm", text }
  const [busy, setBusy] = useState(false);
  const fileRef = useRef(null);
  const doExport = async () => {
    if (!(ctx.live && API)) return;
    setBusy(true);
    try { await API.downloadBackup(); setMsg(null); }
    catch (e) { setMsg({ tone: "alarm", text: "Export fehlgeschlagen: " + e.message }); }
    setBusy(false);
  };
  const doImport = async (file) => {
    if (!file || !(ctx.live && API)) return;
    setBusy(true);
    try {
      const backup = JSON.parse(await file.text());
      const r = await API.importBackup(backup);
      setMsg({ tone: "ok", text: `Import ok — ${r.sensors} Melder übernommen. PIN, Passwörter und HomeKit-Kopplung müssen neu gesetzt werden.` });
    } catch (e) {
      setMsg({ tone: "alarm", text: "Import fehlgeschlagen: " + (e.message || "ungültige Datei") });
    }
    setBusy(false);
    if (fileRef.current) fileRef.current.value = "";
  };
  return (
    <div className="card" style={{ marginTop: "var(--space-4)" }}>
      <div className="card__head"><span className="card__title card__title--sm">Sicherung</span></div>
      <div className="card__body card__body--compact">
        <p style={{ fontSize: "var(--text-sm)", color: "var(--fg-subtle)", marginTop: 0 }}>
          Exportiert Melder-Konfiguration und Einstellungen als Datei — ohne PIN, Passwörter
          und HomeKit-Kopplung. Vor Firmware-Migrationen exportieren.
        </p>
        <div className="cluster">
          <Button variant="secondary" size="sm" iconLeft="download" disabled={busy || !ctx.live} onClick={doExport}>Sicherung exportieren</Button>
          <Button variant="secondary" size="sm" iconLeft="upload" disabled={busy || !ctx.live} onClick={() => fileRef.current && fileRef.current.click()}>Sicherung importieren…</Button>
          <input ref={fileRef} type="file" accept="application/json,.json" style={{ display: "none" }}
            onChange={(e) => doImport(e.target.files && e.target.files[0])} />
        </div>
        {msg && <p style={{ fontSize: "var(--text-sm)", color: msg.tone === "ok" ? "var(--state-ok-fg)" : "var(--state-alarm-fg)", marginBottom: 0 }}>{msg.text}</p>}
      </div>
    </div>
  );
}

/* Firmware update (OTA): upload the app image (.bin from the release, NOT the merged image) —
   the device streams it into the inactive slot, verifies SHA-256 + image magic, and reboots on
   confirmation. New firmware runs a 5-min self-test; a panic during that time → automatic
   rollback to the previous version. UI strings intentionally inline (maintenance tool). */
function UpdatePanel({ ctx, setupWindowS }) {
  const { dev } = ctx;
  const [ota, setOta] = useState(null);
  const [file, setFile] = useState(null);
  const [sha, setSha] = useState(null);
  const [busy, setBusy] = useState(false);
  const [prog, setProg] = useState(null); // { l, t } during XHR upload
  const [err, setErr] = useState(null);
  const fileRef = useRef(null);

  useEffect(() => {
    if (!(ctx.live && API)) return;
    let stop = false;
    const tick = () => API.getOta().then((o) => { if (!stop) setOta(o); }).catch(() => {});
    tick();
    const id = setInterval(tick, 2000);
    return () => { stop = true; clearInterval(id); };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- OTA status poll started once on mount; ctx.live is session-constant
  }, []);

  const ERRS = {
    sha_mismatch: "SHA-256 stimmt nicht — falsche oder beschädigte Datei",
    bad_image: "Kein Firmware-App-Image (das merged Flash-Image geht hier nicht)",
    too_small: "Übertragung unvollständig oder Datei zu klein",
    too_large: "Datei passt nicht in den 4-MB-Slot",
    busy: "Es läuft bereits ein Update",
    capture_active: "Erst den GMS-Mitschnitt stoppen",
    write_failed: "Flash-Schreibfehler — Log prüfen",
    read_failed: "Verbindung abgebrochen — erneut versuchen",
    network: "Netzwerkfehler — Setup-Fenster noch offen?",
  };

  const pick = async (f) => {
    if (!f) return;
    setErr(null); setSha(null); setFile(f);
    try {
      const buf = await f.arrayBuffer();
      const d = await window.crypto.subtle.digest("SHA-256", buf);
      setSha(Array.from(new Uint8Array(d)).map((b) => b.toString(16).padStart(2, "0")).join(""));
    } catch (e) {
      setErr("SHA-Berechnung fehlgeschlagen: " + e.message);
      setFile(null);
    }
  };
  const upload = async () => {
    if (!(file && sha && ctx.live && API)) return;
    setBusy(true); setErr(null); setProg({ l: 0, t: file.size });
    try {
      await API.uploadOta(file, sha, (l, t) => setProg({ l, t }));
      setFile(null); setSha(null);
      if (fileRef.current) fileRef.current.value = "";
    } catch (e) {
      setErr(ERRS[e.code] || "Fehlgeschlagen: " + e.message);
    }
    setProg(null); setBusy(false);
  };
  const reboot = () => { if (ctx.live && API) API.reboot().catch(() => {}); };

  const state = ota && ota.state;
  const pct = prog && prog.t ? Math.round((prog.l / prog.t) * 100)
    : state === "receiving" ? (ota.progress_pct || 0) : null;
  const slot = ota && ota.running_slot;
  const deviceErr = state === "failed" && ota.error ? (ERRS[ota.error] || ota.error) : null;
  // ESP-IDF slot states, humanized (raw "valid"/"unverified" is bootloader jargon).
  const SLOT = {
    valid: ["geprüft ✓", "Dieses Image hat Boot und 5-Minuten-Selbsttest bestanden."],
    unverified: ["Selbsttest läuft", "Erster Boot nach dem Update — schlägt der Selbsttest fehl, folgt der automatische Rollback."],
    invalid: ["Rollback erfolgt", "Dieses Image hat den Selbsttest nicht bestanden — die vorherige Firmware läuft."],
    factory: ["Werksimage", "Direkt geflasht (ohne OTA-Slot-Status)."],
  };
  const [slotLabel, slotTip] = (slot && SLOT[slot.state]) || [slot ? slot.state : "", ""];
  return (
    <div className="card" style={{ marginTop: "var(--space-4)" }}>
      <div className="card__head" style={{ justifyContent: "space-between" }}>
        <span className="card__title card__title--sm">Firmware-Update</span>
        <span className="mono" style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }} title={slotTip}>
          v{dev.fw}{dev.fw_build ? ` · ${dev.fw_build}` : ""}{slot ? ` · ${slot.label} · ${slotLabel}` : ""}
        </span>
      </div>
      <div className="card__body card__body--compact">
        {ota && ota.latest_version && (
          <p style={{ fontSize: "var(--text-sm)", color: "var(--state-warn-fg)", marginTop: 0 }}>
            Version {ota.latest_version} verfügbar — App-Image vom GitHub-Release laden und hier hochladen.
          </p>
        )}
        {ota && ota.pending_verify && (
          <p style={{ fontSize: "var(--text-sm)", color: "var(--state-warn-fg)", marginTop: 0 }}>
            Neue Firmware im Selbsttest{ota.self_test_s_remaining != null ? ` — noch ${ota.self_test_s_remaining} s` : ""}.
            Gerät nicht ausschalten (sonst Rollback auf die alte Version).
          </p>
        )}
        {state === "ready_to_reboot" && (
          <div className="cluster" style={{ marginBottom: "var(--space-3)" }}>
            <span style={{ fontSize: "var(--text-sm)", color: "var(--state-ok-fg)" }}>
              Image{ota.new_version ? ` v${ota.new_version}` : ""} geschrieben · Prüfsumme (SHA-256) verifiziert ✓ — Neustart übernimmt es.
            </span>
            <Button variant="primary" size="sm" iconLeft="power" onClick={reboot}>Jetzt neu starten</Button>
          </div>
        )}
        {state !== "ready_to_reboot" && (
          <div className="cluster">
            <Button variant="secondary" size="sm" iconLeft="upload" disabled={busy || !ctx.live}
              onClick={() => fileRef.current && fileRef.current.click()}>App-Image wählen…</Button>
            <input ref={fileRef} type="file" accept=".bin,application/octet-stream" style={{ display: "none" }}
              onChange={(e) => pick(e.target.files && e.target.files[0])} />
            {file && (
              <span className="mono" style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>
                {file.name} ({(file.size / 1048576).toFixed(2)} MB){sha ? ` · SHA ${sha.slice(0, 12)}…` : " · hashe…"}
              </span>
            )}
            <Button variant="primary" size="sm" disabled={!(file && sha) || busy || !ctx.live} onClick={upload}>
              {busy ? "Lädt hoch…" : "Update starten"}
            </Button>
          </div>
        )}
        {pct != null && (
          <div className="progress-track" style={{ marginTop: "var(--space-3)" }}>
            <div className="progress-fill" style={{ width: pct + "%" }} />
          </div>
        )}
        {(err || deviceErr) && (
          <p style={{ fontSize: "var(--text-sm)", color: "var(--state-alarm-fg)", marginBottom: 0 }}>{err || deviceErr}</p>
        )}
        {ota && (
          <label className="toggle" style={{ gap: "var(--space-2)", marginTop: "var(--space-3)" }}>
            <span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>Täglich auf Updates prüfen (Installation bleibt manuell)</span>
            <input type="checkbox" checked={!!ota.update_check}
              onChange={(e) => { const on = e.target.checked; setOta({ ...ota, update_check: on }); if (ctx.live && API) API.putOtaSettings(on).catch(() => {}); }} />
            <span className="toggle__track"><span className="toggle__thumb" /></span>
          </label>
        )}
        {typeof setupWindowS === "number" && (
          <p style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)", marginBottom: 0 }}>
            Setup-Fenster noch {Math.max(1, Math.floor(setupWindowS / 60))} min offen — Upload vorher starten.
          </p>
        )}
      </div>
    </div>
  );
}

/* Raw GMS capture: diagnose panels not yet supported (e.g. hiplex). Records the raw receive
   byte stream (directly replayable with `telenot-sim replay`); three modes as an escalation
   ladder, NONE of them switches the panel. UI strings intentionally inline (tester tool). */
function CapturePanel({ ctx, initialOpen }) {
  const [cap, setCap] = useState(MOCK.capture);
  const [mode, setMode] = useState("listen");
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(!!initialOpen); // collapsed by default; open when jumped to directly
  const panelRef = useRef(null);
  useEffect(() => { // scroll panel into view when opened via direct jump
    if (initialOpen && panelRef.current) panelRef.current.scrollIntoView({ block: "start", behavior: "smooth" });
  }, [initialOpen]);
  useEffect(() => {
    if (!(ctx.live && API)) return;
    let stop = false;
    const tick = () => API.getCapture().then((c) => { if (!stop) setCap(c); }).catch(() => {});
    tick();
    const id = setInterval(tick, 2000);
    return () => { stop = true; clearInterval(id); };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- capture status poll started once on mount; ctx.live is session-constant
  }, []);
  const start = async () => { setBusy(true); try { if (ctx.live && API) await API.startCapture(mode); } catch (_) { /* offline fallback */ } setBusy(false); };
  const stop = async () => { setBusy(true); try { if (ctx.live && API) await API.stopCapture(); } catch (_) { /* offline fallback */ } setBusy(false); };
  const dl = () => { if (ctx.live && API) API.downloadCapture().catch(() => {}); };
  const modes = [
    ["listen", "Nur lauschen", "sendet nichts an die Anlage"],
    ["listen_ack", "Lauschen + Quittung", "sendet nur FT1.2-Quittung, kein Schalten"],
    ["discover", "Discover-Lauf", "sendet Lese-Abfragen (Belegt/Text), kein Schalten"],
  ];
  return (
    <div className="card" ref={panelRef} style={{ marginTop: "var(--space-4)" }}>
      <button type="button" onClick={() => setOpen((o) => !o)} style={{
        width: "100%", display: "flex", alignItems: "center", justifyContent: "space-between",
        gap: "var(--space-2)", background: "transparent", border: 0, cursor: "pointer",
        padding: "var(--space-3) var(--space-4)", color: "var(--fg-default)", textAlign: "left",
      }}>
        <span className="card__title card__title--sm">
          GMS-Mitschnitt <span style={{ color: "var(--fg-subtle)", fontWeight: "var(--fw-regular)" }}>· Diagnose</span>
        </span>
        <span style={{ display: "flex", alignItems: "center", gap: "var(--space-2)" }}>
          {cap.active && <StatusDot tone="ok" live />}
          <span style={{ color: "var(--fg-subtle)", fontSize: "var(--text-sm)" }}>{open ? "▾" : "▸"}</span>
        </span>
      </button>
      {open && (
      <div className="card__body stack stack-3">
        <div style={{ fontSize: "var(--text-sm)", color: "var(--fg-subtle)" }}>
          Zeichnet den rohen GMS-Datenstrom auf, damit eine noch nicht unterstützte Anlage
          (z.B. hiplex) analysiert werden kann. <strong>Kein Modus schaltet die Anlage</strong> —
          sie unterscheiden sich nur darin, wie viel das Gerät sendet.
        </div>
        <div className="stack stack-2">
          {modes.map(([id, label, sub]) => (
            <label key={id} style={{
              display: "flex", alignItems: "flex-start", gap: "var(--space-2)",
              padding: "var(--space-2) var(--space-3)", borderRadius: "8px",
              border: "1px solid " + (mode === id ? "var(--accent)" : "var(--border-default)"),
              background: mode === id ? "var(--accent-soft)" : "transparent",
              cursor: cap.active ? "default" : "pointer", opacity: cap.active ? 0.55 : 1,
            }}>
              <input type="radio" name="capmode" checked={mode === id} disabled={cap.active}
                onChange={() => setMode(id)} style={{ marginTop: "3px", accentColor: "var(--accent)" }} />
              <span style={{ fontSize: "var(--text-sm)" }}><strong>{label}</strong> — {sub}</span>
            </label>
          ))}
        </div>
        {cap.active && <div style={{ fontSize: "var(--text-sm)", color: "var(--fg-default)" }}><strong>Aktiv:</strong> {cap.sends}</div>}
        <div className="stat-grid">
          <div className="metric"><div className="metric__lbl">Bytes</div><div className="metric__val mono">{cap.bytes_total}</div></div>
          <div className="metric"><div className="metric__lbl">Frames ok</div><div className="metric__val mono">{cap.frames_ok}</div></div>
          <div className="metric"><div className="metric__lbl">Frames Fehler</div><div className="metric__val mono">{cap.frames_err}</div></div>
          <div className="metric"><div className="metric__lbl">Laufzeit</div><div className="metric__val mono">{cap.elapsed_s}s</div></div>
        </div>
        {cap.rec_types && cap.rec_types.length > 0 &&
          <div style={{ fontSize: "var(--text-sm)" }}>Satztypen: <span className="mono">{cap.rec_types.map((r) => `${r.hex}×${r.count}`).join("  ")}</span></div>}
        <div className="cluster">
          {!cap.active
            ? <Button variant="primary" size="sm" iconLeft="spark" disabled={busy} onClick={start}>Mitschnitt starten</Button>
            : <Button variant="danger" size="sm" iconLeft="power" disabled={busy} onClick={stop}>Stoppen</Button>}
          <Button variant="secondary" size="sm" iconLeft="copy" disabled={!cap.buf_used} onClick={dl}>capture.bin herunterladen ({(cap.buf_used / 1024).toFixed(1)} KB)</Button>
        </div>
        <div style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>
          Nach 2–3 Minuten stoppen, Datei herunterladen und an den Entwickler schicken (Details: docs/DEBUG-CAPTURE.md).
        </div>
      </div>
      )}
    </div>
  );
}

/* Collapsed about card: project identity, source, license, author, thanks. */
function AboutPanel({ ctx }) {
  const { t, dev } = ctx;
  const [open, setOpen] = useState(false);
  return (
    <div className="card" style={{ marginTop: "var(--space-4)" }}>
      <button type="button" onClick={() => setOpen((o) => !o)} style={{
        width: "100%", display: "flex", alignItems: "center", justifyContent: "space-between",
        gap: "var(--space-2)", background: "transparent", border: 0, cursor: "pointer",
        padding: "var(--space-3) var(--space-4)", color: "var(--fg-default)", textAlign: "left",
      }}>
        <span className="card__title card__title--sm">
          {t("about.title")} <span style={{ color: "var(--fg-subtle)", fontWeight: "var(--fw-regular)" }}>· {PROJECT.name}</span>
        </span>
        <span style={{ color: "var(--fg-subtle)", fontSize: "var(--text-sm)" }}>{open ? "▾" : "▸"}</span>
      </button>
      {open && (
        <div className="card__body stack stack-3">
          <dl className="deflist" style={{ margin: 0 }}>
            <dt>{t("about.version")}</dt><dd className="mono">v{dev.fw}{dev.fw_build ? ` · ${dev.fw_build}` : ""}</dd>
            <dt>{t("about.source")}</dt><dd><a href={PROJECT.repo} target="_blank" rel="noreferrer">{PROJECT.repo.replace("https://", "")}</a></dd>
            <dt>{t("about.license")}</dt><dd>{PROJECT.license}</dd>
            <dt>{t("about.author")}</dt><dd>{PROJECT.author} · <a href={`mailto:${PROJECT.email}`}>{PROJECT.email}</a></dd>
          </dl>
          <div style={{ fontSize: "var(--text-sm)", color: "var(--fg-subtle)" }}>{t("about.thanks")}</div>
        </div>
      )}
    </div>
  );
}

/* ===================== S8 — REVIEW & COMMIT ===================== */
export { DiagModal };

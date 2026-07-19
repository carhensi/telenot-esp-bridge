// App root: state, routing, header, stepper, overlays, scan status.
import React from 'preact/compat';
const { useState, useEffect, useRef, useCallback, useMemo } = React;
import { I18N } from './i18n.js';
import { MOCK } from './mock.js';
import { API } from './api.js';
import { Icon, Brandmark, Button, StatusDot, Modal, hex, fmtClock } from './ui.jsx';
import { ScreenBoot, ScreenLogin, ScreenSerial, ScreenScan, ScreenSensors } from './wizard.jsx';
import { ScreenMqtt, ScreenSecurity } from './integration.jsx';
import { DiagModal } from './diagnostics.jsx';
import { ScreenReview, ScreenReboot } from './review.jsx';
import { DASH, AlarmOverlay, DisarmModal, ScreenDashboard } from './dashboard.jsx';

// Diagnostics (s7) is no longer a wizard step — opened on demand via the Serial/MQTT pill
// in the header (DiagModal). The s7.* strings and the component are kept.
const STEPS = ["s0", "s1", "s2", "s3", "s4", "s5", "s6", "s8"];
// Fixed appearance: light + "clean" + default accent + default density (style switcher removed).
const ACCENT = { h: 264, c: 0.118 };

// Scan status as a ring pill in the header: ring = progress, click opens a detail popover
// (progress bar, found/named/remaining, current address, "Back to scan").
// Phases: idle (hidden) · busy (indeterminate spinner) · naming (progress) · done (✓ green).
function ScanStatus({ ctx }) {
  const { t, scan } = ctx;
  const [open, setOpen] = useState(false);
  const ref = useRef(null);
  useEffect(() => {
    if (!open) return;
    const h = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    const k = (e) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", h);
    document.addEventListener("keydown", k);
    return () => { document.removeEventListener("mousedown", h); document.removeEventListener("keydown", k); };
  }, [open]);

  if (scan.phase === "idle") return null;

  const counting = scan.phase === "belegt";
  const done = scan.phase === "done";
  const pct = scan.total > 0 ? Math.round((scan.named / scan.total) * 100) : 0;
  const C = 2 * Math.PI * 12;
  const off = counting ? C * 0.72 : C * (1 - pct / 100);

  const fmtEta = (s) => {
    if (!s || s <= 0) return "–";
    if (s >= 60) return Math.ceil(s / 60) + " " + t("s3.min");
    return Math.round(s) + " " + t("scan.sec");
  };
  const etaTxt = fmtEta(scan.remaining);
  const state = counting ? "counting" : done ? "done" : "running";

  return (
    <div className="scanwrap" ref={ref}>
      <button className={`scanpill scanpill--${state}`} onClick={() => setOpen((v) => !v)} aria-expanded={open} aria-label={t("scan.details")}>
        <span className="scanring">
          <svg width="30" height="30" viewBox="0 0 30 30" className={counting ? "scanring--spin" : ""}>
            <circle className="scanring__bg" cx="15" cy="15" r="12" fill="none" strokeWidth="3" />
            <circle className="scanring__fg" cx="15" cy="15" r="12" fill="none" strokeWidth="3"
              strokeLinecap="round" strokeDasharray={C.toFixed(1)} strokeDashoffset={off.toFixed(1)} />
          </svg>
          <span className="scanring__pct">{done ? <Icon name="check" size={13} sw={3} /> : counting ? "" : pct + "%"}</span>
        </span>
        <span className="scanpill__txt">
          <span className="scanpill__t">
            {!done && <span className="scanpill__pulse" />}
            {done ? t("scan.done.pill") : t("s3.scanning")}
          </span>
          <span className="scanpill__s">
            {counting ? t("scan.counting")
              : done ? `${scan.total} ${t("s3.found").toLowerCase()}`
              : `${scan.named}/${scan.total} · ${t("scan.left", { v: etaTxt })}`}
          </span>
        </span>
      </button>

      {open && (
        <div className="scanpop">
          <div className="scanpop__head">
            {!done && <span className="scanpill__pulse" />}
            {done && <StatusDot tone="ok" />}
            <span className="scanpop__title">{t("scan.title")}</span>
            <span className={`badge badge--${done ? "ok" : "unconfirmed"}`}>{done ? t("st.ok") : t("s3.scanning")}</span>
          </div>
          <div className="scanpop__body">
            <div className="progress-track">
              <div className={`progress-fill${counting ? " progress-fill--indet" : ""}`} style={counting ? undefined : { width: pct + "%" }} />
            </div>
            <div className="scanpop__stats">
              <div className="scan-stat"><span className="scan-stat__val">{scan.named}</span><span className="scan-stat__lbl">{t("s3.found")}</span></div>
              <div className="scan-stat"><span className="scan-stat__val">{done ? scan.total : scan.named}</span><span className="scan-stat__lbl">{t("scan.named")}</span></div>
              <div className="scan-stat"><span className="scan-stat__val" style={{ color: "var(--accent)" }}>{done ? "–" : etaTxt}</span><span className="scan-stat__lbl">{t("scan.rest")}</span></div>
            </div>
            {!done && (
              <div className="scanpop__cur">
                {t("scan.checking")} <span className="mono">{scan.current ? hex(scan.current) : "…"}</span>
              </div>
            )}
            <button className="scanpop__link" onClick={() => { setOpen(false); ctx.goto(3); }}>{t("s3.chipreturn")} →</button>
          </div>
        </div>
      )}
    </div>
  );
}

// Status pill in the header (Serial/MQTT/HomeKit/web interface) — icon with live dot (color = state);
// click opens diagnostics/integrations or extends the session.
function StatPill({ tone = "ok", icon, label, value, onClick, title, className = "" }) {
  return (
    <button className={`statpill statpill--${tone}${className ? " " + className : ""}`} onClick={onClick} title={title}>
      <span className="statpill__icowrap">
        {icon && <Icon name={icon} size={15} className="statpill__ico" />}
        <span className="statpill__live" />
      </span>
      {label && <span className="statpill__label">{label}</span>}
      {value != null && <span className="statpill__val">{value}</span>}
    </button>
  );
}

function Header({ ctx, theme, setTheme }) {
  const { t } = ctx;
  return (
    <header className="appbar">
      <button type="button" className="appbar__brand" onClick={ctx.goHome} aria-label={t("brand.home")} title={t("brand.home")}>
        <Brandmark />
        <div><div className="brandmark__name">{t("app.name")}</div><div className="brandmark__sub">{t("app.sub")}</div></div>
      </button>
      <span className="appbar__spacer" />
      {!ctx.booting && ctx.view !== "dashboard" && !(ctx.dev && ctx.dev.configured) && (
        <div className="statpill setup-flag" title={t("hdr.setupmode")}>
          <span className="statpill__icowrap"><Icon name="sliders" size={15} className="statpill__ico" /><span className="statpill__live" /></span>
          <span className="statpill__label">{t("hdr.setupmode")}</span>
        </div>
      )}
      <div className="appbar__status">
        <ScanStatus ctx={ctx} />
        {ctx.webSeconds != null && (() => {
          const s = ctx.webSeconds;
          const tone = s <= 60 ? "alarm" : s <= 120 ? "warn" : "ok";
          // Countdown collapsed by default (statpill--web), expands on hover/focus.
          return <StatPill tone={tone} icon="webface" label={t("hdr.webiface")} value={fmtClock(s)} onClick={ctx.extendWeb} title={t("web.extend")} className="statpill--web" />;
        })()}
        {ctx.view === "dashboard" && (() => {
          // Combined health pill: worst state of serial + integration (MQTT/HomeKit) wins;
          // the per-subsystem breakdown lives in the diagnostics modal it opens.
          const sTone = ctx.serialOk == null ? "unavail" : ctx.serialOk ? "ok" : "alarm";
          const m = ctx.mqttStat;
          const mTone = ctx.mqtt && ctx.mqtt.homekit_mode ? "ok"
            : m === "ok" ? "ok" : m === "connecting" ? "warn" : m === "off" || m == null ? "unavail" : "alarm";
          const rank = { ok: 0, unavail: 1, warn: 2, alarm: 3 };
          const tone = rank[sTone] >= rank[mTone] ? sTone : mTone;
          return <StatPill tone={tone} icon="server" label={t("hdr.status")} onClick={ctx.openDiag} title={t("s7.title")} />;
        })()}
      </div>
      {/* Mode-aware toggle — icon only, consistent with language/theme controls. Only shown once configured.
         Dashboard → configuration (sliders), Wizard → live view (shield). */}
      {ctx.dev && ctx.dev.configured && (() => {
        const toConfig = ctx.view === "dashboard";
        const label = toConfig ? t("menu.config") : t("nav.live");
        return (
          <button className="icon-btn" onClick={() => ctx.toggleDashboard()} aria-label={label} title={label}>
            <Icon name={toConfig ? "sliders" : "shield"} size={18} />
          </button>
        );
      })()}
      {/* Settings inline, space-efficient — desktop and mobile alike (no menu needed).
         Language: a button showing what it switches to (DE→"EN"). Theme: sun/moon. */}
      <button className="lang-btn" onClick={() => ctx.setLang(ctx.lang === "de" ? "en" : "de")} aria-label={t("menu.language")} title={t("menu.language")}>
        {ctx.lang === "de" ? "EN" : "DE"}
      </button>
      <button className="icon-btn" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} aria-label={t("hdr.theme")} title={t("hdr.theme")}><Icon name={theme === "dark" ? "sun" : "moon"} size={18} /></button>
    </header>
  );
}

function Stepper({ ctx }) {
  const { t, step, maxStep, goto } = ctx;
  return (
    <>
      <div className="stepper-wrap"><nav className="stepper scroll" aria-label="Steps">
        {STEPS.map((id, i) => {
          const done = i < step && i <= maxStep;
          const disabled = i > maxStep;
          return (
            <button key={id} className={`step${done ? " step--done" : ""}`} aria-current={i === step} disabled={disabled} onClick={() => goto(i)}>
              <span className="step__num">{done ? <Icon name="check" size={13} sw={3} /> : i}</span>{t("step." + id)}
            </button>
          );
        })}
      </nav></div>
      <div className="stepbar-mobile">
        <span className="stepbar-mobile__label">{t("step." + STEPS[step])}</span>
        <div className="stepbar-mobile__track"><div className="stepbar-mobile__fill" style={{ width: ((step + 1) / STEPS.length * 100) + "%" }} /></div>
        <span className="stepbar-mobile__label mono">{step + 1}/{STEPS.length}</span>
      </div>
    </>
  );
}

function ToastRegion({ toasts }) {
  return (
    <div className="toast-region" aria-live="polite">
      {toasts.map((x) => (
        <div key={x.id} className={`toast toast--${x.tone}`}>
          <Icon name={x.tone === "ok" ? "check" : "alert"} size={18} style={{ color: x.tone === "ok" ? "var(--state-ok-solid)" : "var(--state-alarm-solid)" }} />
          <span className="toast__msg">{x.msg}</span>
        </div>
      ))}
    </div>
  );
}

/* Web interface lock: reminder popup + lock screen. The firmware does the actual shutdown
   (setup window expiry); here we only track the countdown and offer to extend. */
function WebWarnModal({ t, seconds, onExtend, onDismiss }) {
  return (
    <Modal open onClose={onDismiss} icon="clock" iconTone="warn"
      title={t("web.warntitle", { clock: fmtClock(seconds) })}
      footer={<>
        <Button variant="ghost" onClick={onDismiss}>{t("web.keepediting")}</Button>
        <Button variant="primary" iconLeft="clock" onClick={onExtend}>{t("web.extend")}</Button>
      </>}>
      <p style={{ color: "var(--fg-muted)" }}>{t("web.warnbody")}</p>
    </Modal>
  );
}

function WebLockedOverlay({ t }) {
  return (
    <div className="scrim" style={{ zIndex: 100 }}>
      <div className="modal" style={{ maxWidth: 420 }}>
        <div className="modal__head">
          <span className="modal__icon modal__icon--warn"><Icon name="power" size={20} /></span>
          <h2 className="modal__title">{t("web.lockedtitle")}</h2>
        </div>
        <div className="modal__body"><p style={{ color: "var(--fg-muted)", margin: 0 }}>{t("web.lockedbody")}</p></div>
      </div>
    </div>
  );
}

// Neutral transition during initial routing (boot fetch). Prevents a brief S0 welcome
// flash before a configured device jumps straight to the dashboard.
function ScreenSplash() {
  return (
    <div className="page page--narrow" style={{ display: "grid", placeItems: "center", minHeight: "40vh" }}>
      <div className="stack stack-3" style={{ alignItems: "center", opacity: 0.6 }} aria-busy="true">
        <Brandmark size={64} />
        <div className="progress-track" style={{ width: 120 }}><div className="progress-fill progress-fill--indet" /></div>
      </div>
    </div>
  );
}

function App() {
  // Persist language + theme across reloads (localStorage). Defaults: DE/Light.
  const [lang, setLang] = useState(() => { try { return window.localStorage.getItem("tn_lang") || "de"; } catch { return "de"; } });
  const [theme, setTheme] = useState(() => { try { return window.localStorage.getItem("tn_theme") || "light"; } catch { return "light"; } });

  // Last known setup state from the previous visit. If the device was configured ("1"),
  // render the first frame directly as the dashboard — no wizard flash, no splash.
  // The boot fetch only confirms or corrects this afterwards.
  const bootHint = (() => { try { return window.localStorage.getItem("tn_configured"); } catch { return null; } })();
  const configuredHint = bootHint === "1";

  const [view, setView] = useState(configuredHint ? "dashboard" : "wizard"); // wizard | dashboard | reboot
  const [diagOpen, setDiagOpen] = useState(false); // diagnostics panel (on demand from header)
  const [diagDirect, setDiagDirect] = useState(false); // open capture panel directly when jumping to diag
  const [liveState, setLiveState] = useState(null); // app-wide panel state (global alarm watcher)
  const [disarmOpen, setDisarmOpen] = useState(false); // app-wide disarm PIN modal (overlay + dashboard)
  const [alarmDismissed, setAlarmDismissed] = useState(false); // alarm overlay temporarily dismissed
  const [step, setStep] = useState(0);
  const [maxStep, setMaxStep] = useState(0);

  const [sensors, setSensors] = useState(() => MOCK.sensors.map((s) => ({ ...s })));
  const [mqtt, setMqtt] = useState(() => ({ ...MOCK.mqtt }));
  const [sec, setSec] = useState({ pinSet: false, pin: "", remoteDisarm: false });
  const [conn, setConn] = useState({ type: "internal", ip: "192.168.1.50", port: "8234", panel: "complex400", gms: "lite", baud: "9600", cmdsVerified: false });
  const [scan, setScan] = useState({ phase: "idle", total: 0, scanned: 0, named: 0, elapsed: 0, remaining: 540, current: 0, feed: [] });
  const [toasts, setToasts] = useState([]);
  const scanTimer = useRef(null);

  // Live backend (telenot-sim serve / firmware) detected? Otherwise fall back to MOCK (offline prototype).
  const [live, setLive] = useState(false);
  // Show splash only while routing isn't already known from the last visit:
  // With configuredHint we land optimistically in the dashboard (no splash). Without hint
  // (fresh browser / last state unconfigured) the splash bridges the boot fetch so the
  // wrong S0 screen never flashes.
  const [booting, setBooting] = useState(() => !!API && !configuredHint);
  // Optimistically set configured from the hint so the dashboard renders immediately in
  // operation mode ("Status & Control") rather than briefly in test mode. getDevice() overwrites.
  const [dev, setDev] = useState(configuredHint ? { ...MOCK.device, configured: true } : MOCK.device);
  const [rooms, setRooms] = useState(MOCK.rooms);

  const t = useMemo(() => I18N.make(lang), [lang]);

  // dev/verify hook only — exposes __jump(i) for manual step navigation
  useEffect(() => { if (import.meta.env.DEV) window.__jump = (i) => { setMaxStep(8); setStep(i); }; }, []);

  // Reusable response hydrators (DRY — each used in 2–7 call sites across boot-resume,
  // scan poll, and enterWizard):
  const scanFromDto = (s) => ({ phase: s.phase, total: s.total, scanned: s.scanned || 0, named: s.named, elapsed: s.elapsed, remaining: s.remaining, current: s.current || 0, feed: (s.feed || []).map(API.normSensor) });
  const hydrateSensors = useCallback((r) => {
    if (!r || !r.sensors) return;
    setSensors(r.sensors.map(API.normSensor));
    if (r.rooms) setRooms(r.rooms);
  }, []);
  // Hydrate persisted connection settings into the UI — otherwise S2 shows default
  // values after a reload and looks wrongly "unconfigured".
  const hydrateConnection = useCallback(() => {
    API.getConnection()
      .then((c) => setConn((p) => ({ type: c.type || p.type, ip: c.ip || p.ip, port: c.port ? String(c.port) : p.port, panel: c.panel_kind || p.panel, gms: c.gms_variant || p.gms, baud: c.baud ? String(c.baud) : p.baud, cmdsVerified: !!c.hiplex_cmds_verified })))
      .catch(() => {});
  }, []);
  // Keep MQTT/HomeKit mode current app-wide — otherwise the header integration pill shows
  // a stale mock default after a reload that lands directly in the dashboard.
  const hydrateMqtt = useCallback(() => {
    API.getMqtt().then((m) => setMqtt({ ...m, _pw: "" })).catch(() => {});
  }, []);

  // Backend detection + session resume: /device is public. If a valid session exists (cookie
  // survives reload/new tab) we skip boot + login and land directly in the wizard —
  // mid-scan if a scan is running.
  useEffect(() => {
    if (!API) return; // Pure-mock build: booting was never true, show wizard immediately.
    (async () => {
     try {
      const ok = await API.detect();
      if (!ok) return;
      setLive(true);
      setSensors([]); // real device → stop showing mock/demo sensors (loaded live)
      let d = null;
      try {
        d = await API.getDevice(); setDev(d);
        // Routing hint for the next reload: determines the first frame (dashboard vs. wizard).
        try { window.localStorage.setItem("tn_configured", d && d.configured ? "1" : "0"); } catch (_) {}
      } catch (_) {}
      let sess;
      try { sess = await API.resumeSession(); }
      catch (_) {
        // No session (e.g. fresh after reboot). Configured device → go straight to login
        // instead of the first-setup boot screen ("setup again?!"). setView("wizard")
        // pulls us out of the optimistic dashboard (session expired / device reset).
        setView("wizard");
        if (d && d.configured) { setMaxStep((m) => Math.max(m, 1)); setStep(1); }
        return;
      }
      if (!sess) { setView("wizard"); return; }
      if (sess.password_change_required) { setView("wizard"); setMaxStep((m) => Math.max(m, 1)); setStep(1); return; }
      setMaxStep((m) => Math.max(m, 2));
      hydrateConnection();
      hydrateMqtt();
      try {
        const s = await API.getScan();
        if (s && s.phase && s.phase !== "idle") {
          setScan(scanFromDto(s));
          setView("wizard");
          setMaxStep((m) => Math.max(m, 3));
          setStep(3);
          if (s.phase === "done") {
            hydrateSensors(await API.getSensors());
          } else {
            attachScanPoll();
          }
          return;
        }
      } catch (_) {}
      if (d && d.configured) {
        // Session valid, no scan, device configured → live view instead of wizard.
        try { hydrateSensors(await API.getSensors()); } catch (_) {}
        setMaxStep(7);
        setView("dashboard");
        return;
      }
      // Scan done but not yet committed (sensors seeded)? → sensor table,
      // NOT S2 — otherwise the user re-configures and accidentally starts a new scan.
      try {
        const r = await API.getSensors();
        if (r.sensors && r.sensors.length) {
          hydrateSensors(r);
          setView("wizard");
          setMaxStep((m) => Math.max(m, 6));
          setStep(4);
          return;
        }
      } catch (_) {}
      setView("wizard");
      setStep(2); // Session valid, no scan → go straight to the connection page.
     } finally {
      setBooting(false); // Routing decided (every path) → wizard/dashboard may render.
     }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- boot/routing runs once on mount; the hydrate/scan-poll callbacks are intentionally not deps
  }, []);

  // Apply fixed appearance; only light/dark remains toggleable.
  useEffect(() => {
    const r = document.documentElement;
    r.setAttribute("data-theme", theme);
    r.setAttribute("data-style", "clean");
    r.setAttribute("data-density", "default");
    r.style.setProperty("--accent-h", ACCENT.h);
    r.style.setProperty("--accent-c", ACCENT.c);
    try { window.localStorage.setItem("tn_theme", theme); } catch { /* ignore */ }
  }, [theme]);
  useEffect(() => { try { window.localStorage.setItem("tn_lang", lang); } catch { /* ignore */ } }, [lang]);

  const toast = useCallback((tone, msg) => {
    // Successes are NOT shown as toasts (less noise) — only errors/warnings.
    if (tone === "ok") return;
    const id = Date.now() + Math.random();
    setToasts((p) => [...p, { id, tone, msg }]);
    setTimeout(() => setToasts((p) => p.filter((x) => x.id !== id)), 4200);
  }, []);

  const goto = useCallback((i) => { if (i <= maxStep) { setStep(i); window.scrollTo(0, 0); } }, [maxStep]);
  const next = useCallback(() => setStep((s) => { const n = Math.min(STEPS.length - 1, s + 1); setMaxStep((m) => Math.max(m, n)); window.scrollTo(0, 0); return n; }), []);
  const back = useCallback(() => setStep((s) => { window.scrollTo(0, 0); return Math.max(0, s - 1); }), []);

  // Mirror scan status via a 2s poll. The scan runs server-side on the device — this poll
  // attaches to it regardless of whether it started via startScan or after a reload (enterWizard).
  const attachScanPoll = useCallback(() => {
    clearInterval(scanTimer.current);
    scanTimer.current = setInterval(async () => {
      try {
        const s = await API.getScan();
        setScan(scanFromDto(s));
        // Use the wait time: unlock non-scan steps (MQTT, security, diagnostics) while the
        // scan runs. The sensor step (4) and commit remain content-gated.
        if (s.phase === "belegt" || s.phase === "naming") setMaxStep((m) => Math.max(m, 6));
        if (s.phase === "done") {
          clearInterval(scanTimer.current);
          const r = await API.getSensors();
          hydrateSensors(r);
          toast("ok", t("s3.donetoast", { n: r.sensors.length }));
        }
      } catch (_) {}
    }, 2000);
  }, [toast, t, hydrateSensors]);

  // scan: live → real discovery via 2s poll; offline → simulation
  const startScan = useCallback(() => {
    if (live && API) {
      setScan({ phase: "belegt", total: 0, scanned: 0, named: 0, elapsed: 0, remaining: 540, current: 0, feed: [] });
      setMaxStep((m) => Math.max(m, 6)); // unlock config steps immediately (use the wait time)
      API.startScan().catch(() => {});
      attachScanPoll();
      return;
    }
    setScan({ phase: "belegt", total: 0, scanned: 0, named: 0, elapsed: 0, remaining: 540, current: 0, feed: [] });
    const list = sensors;
    setTimeout(() => {
      const total = list.length;
      setScan((s) => ({ ...s, phase: "naming", total, current: list[0].address }));
      let i = 0;
      scanTimer.current = setInterval(() => {
        i++;
        if (i >= total) {
          clearInterval(scanTimer.current);
          setScan((s) => ({ ...s, phase: "done", scanned: total, named: total, feed: list.slice(0, total), current: 0, elapsed: Math.round(total * 4.4), remaining: 0 }));
          return;
        }
        setScan((s) => ({ ...s, scanned: i, named: i, feed: list.slice(0, i), current: list[Math.min(i, total - 1)].address, elapsed: Math.round(i * 4.4), remaining: Math.round((total - i) * 4.4) }));
      }, 50);
    }, 1700);
  }, [sensors, live, attachScanPoll]);

  // After login: is a scan already running on the device (reload/tab switch mid-scan)?
  // If so, jump straight to the scan step and keep polling — otherwise proceed normally.
  const enterWizard = useCallback(async () => {
    if (live && API) {
      hydrateConnection();
      try {
        const s = await API.getScan();
        if (s && s.phase && s.phase !== "idle") {
          setScan(scanFromDto(s));
          setMaxStep((m) => Math.max(m, 3));
          setStep(3);
          window.scrollTo(0, 0);
          if (s.phase === "done") {
            hydrateSensors(await API.getSensors());
          } else {
            attachScanPoll();
          }
          return;
        }
      } catch (_) {}
      // Configured device, no scan running → go straight to live view instead of wizard step 2.
      // Setup steps remain reachable via the stepper.
      if (dev && dev.configured) {
        try { hydrateSensors(await API.getSensors()); } catch (_) {}
        setMaxStep(7);
        setView("dashboard");
        window.scrollTo(0, 0);
        return;
      }
      // Completed (uncommitted) scan present on the device → sensor table, not a new scan.
      try {
        const r = await API.getSensors();
        if (r.sensors && r.sensors.length) {
          hydrateSensors(r);
          setMaxStep((m) => Math.max(m, 6));
          setStep(4);
          window.scrollTo(0, 0);
          return;
        }
      } catch (_) {}
    }
    next();
  }, [live, dev, next, attachScanPoll, hydrateConnection, hydrateSensors]);

  const keepPartial = useCallback(() => { clearInterval(scanTimer.current); if (live && API) API.cancelScan(true).catch(() => {}); setScan((s) => ({ ...s, phase: "done", remaining: 0 })); }, [live]);
  const discardScan = useCallback(() => { clearInterval(scanTimer.current); if (live && API) API.cancelScan(false).catch(() => {}); setScan({ phase: "idle", total: 0, scanned: 0, named: 0, elapsed: 0, remaining: 540, current: 0, feed: [] }); }, [live]);
  useEffect(() => () => clearInterval(scanTimer.current), []);

  const commit = useCallback(async () => {
    if (live && API) {
      try { await API.commit(true); } catch (e) { toast("err", (e && e.message) || t("s8.commit")); return; }
      // Config saved → real reboot. Applies the config and starts HomeKit (HAP only runs
      // with a config present). Fire-and-forget — the device reboots regardless.
      API.reboot().catch(() => {});
    }
    setView("reboot"); window.scrollTo(0, 0);
  }, [live, toast, t]);

  // Live test board: arm/disarm. Live → real /command (disarm fail-closed); offline → demo toast.
  // lastCmdAt triggers a fast state poll in the dashboard to reduce perceived latency.
  const [lastCmdAt, setLastCmdAt] = useState(0);
  const command = useCallback(async (cmd, pin) => {
    if (live && API) {
      try {
        await API.command(cmd, pin);
        setLastCmdAt(Date.now());
        toast("ok", "Command sent: " + cmd);
      } catch (e) { toast("err", (e && e.message) || "Command failed"); }
    } else {
      toast("ok", "Command (demo): " + cmd);
    }
  }, [live, toast]);
  const restart = useCallback(() => {
    setSensors(MOCK.sensors.map((s) => ({ ...s }))); setMqtt({ ...MOCK.mqtt }); setSec({ pinSet: false, pin: "", remoteDisarm: false });
    setScan({ phase: "idle", total: 0, named: 0, elapsed: 0, remaining: 540, current: 0, feed: [] });
    setConn({ type: "internal", ip: "192.168.1.50", port: "8234", panel: "complex400", gms: "lite", baud: "9600", cmdsVerified: false });
    setStep(0); setMaxStep(0); setView("wizard"); window.scrollTo(0, 0);
  }, []);

  // Feed header pills with real states (previously hard-coded "OK"/"Connecting…"):
  // lightweight 10s poll on /diagnostics. Before login the API returns 401 → pills
  // stay neutral ("—").
  const [diagStat, setDiagStat] = useState(null);
  useEffect(() => {
    if (!(live && API)) return;
    let stop = false;
    const tick = () => API.getDiagnostics()
      .then((d) => { if (!stop) setDiagStat(d); })
      .catch(() => { if (!stop) setDiagStat(null); });
    tick();
    const id = setInterval(tick, 10000);
    return () => { stop = true; clearInterval(id); };
  }, [live]);

  // Global alarm watcher: lightweight /state poll (~3s) once the device is configured —
  // so a triggered alarm pops up on EVERY screen (not only the dashboard, which keeps its
  // own finer-grained poll).
  useEffect(() => {
    if (!(live && API && dev && dev.configured)) return;
    let stop = false;
    const tick = () => API.getState().then((s) => { if (!stop) setLiveState(s); }).catch(() => {});
    tick();
    const id = setInterval(tick, 3000);
    return () => { stop = true; clearInterval(id); };
  }, [live, dev]);
  const armState = liveState ? liveState.arm_state : null;
  // Reset overlay dismiss once the alarm clears → the next real alarm pops up again.
  useEffect(() => { if (armState !== "TRIGGERED") setAlarmDismissed(false); }, [armState]);

  // ── Web interface lock (setup window) ────────────────────────────────────
  // Remaining time comes from the diagnostics poll (setup_window_s_remaining); between polls
  // we count down locally each second. MOCK simulates 30 min so the reminder/lock is testable
  // without hardware — window.__web(seconds) overrides the countdown in dev mode.
  const [webSeconds, setWebSeconds] = useState(live ? null : 1800);
  const [warnDismissed, setWarnDismissed] = useState(false);
  useEffect(() => {
    if (live && diagStat && typeof diagStat.setup_window_s_remaining === "number") {
      setWebSeconds(diagStat.setup_window_s_remaining);
      setWarnDismissed(false);
    }
  }, [diagStat, live]);
  const webActive = webSeconds != null;
  useEffect(() => {
    if (!webActive) return undefined;
    const id = setInterval(() => setWebSeconds((s) => (s == null ? s : Math.max(0, s - 1))), 1000);
    return () => clearInterval(id);
  }, [webActive]);
  useEffect(() => { if (import.meta.env.DEV) window.__web = (s) => setWebSeconds(s); }, []);
  const webWarnOpen = webSeconds != null && webSeconds > 0 && webSeconds <= 120 && !warnDismissed;
  const webLocked = webSeconds != null && webSeconds <= 0;
  const extendWeb = useCallback(() => {
    setWarnDismissed(true);
    if (live && API) {
      API.extendSession().then(() => setWebSeconds((s) => Math.max(s || 0, 1800))).catch(() => toast("err", t("web.extendfail")));
    } else {
      setWebSeconds(1800);
    }
  }, [live, toast, t]);

  // HomeKit "stage → apply" (live-add without reboot): collect HomeKit-toggled sensors
  // (glow), then apply them live to the HAP set via /homekit/apply (no commit/reboot).
  const [hkPending, setHkPending] = useState(() => new Set());
  const hkMark = useCallback((addr) => setHkPending((p) => { const n = new Set(p); n.add(addr); return n; }), []);
  const hkApply = useCallback(async () => {
    if (live && API) {
      try { await API.applyHomekit(); } catch (e) { toast("err", (e && e.message) || t("hk.applyfail")); return; }
    }
    setHkPending(new Set());
    toast("ok", t("hk.applied"));
  }, [live, toast, t]);

  // Toggle dashboard ↔ configuration: on a configured device never land on the
  // first-setup intro screen (S0/S1) — go straight to the sensor table.
  const toggleDashboard = useCallback(() => {
    if (view === "dashboard") {
      if (dev && dev.configured && step < 2) { setMaxStep((m) => Math.max(m, 7)); setStep(4); }
      setView("wizard");
    } else {
      setView("dashboard");
    }
    window.scrollTo(0, 0);
  }, [view, dev, step]);

  // Logo/wordmark = home: configured device → live dashboard (the real "home page"),
  // otherwise back to the setup start (S0).
  const goHome = useCallback(() => {
    if (dev && dev.configured) { setView("dashboard"); }
    else { setView("wizard"); goto(0); }
    window.scrollTo(0, 0);
  }, [dev, goto]);

  const ctx = {
    t, lang, setLang, dev, rooms, live, booting,
    sensors, setSensors, setRooms, mqtt, setMqtt, sec, setSec,
    scan, startScan, keepPartial, discardScan, conn, setConn,
    step, maxStep, goto, next, back, enterWizard, commit, restart, toast,
    view, setView, toggleDashboard, goHome, command, lastCmdAt, openDiag: (direct) => { setDiagDirect(direct === true); setDiagOpen(true); },
    armState, openDisarm: () => setDisarmOpen(true),
    webSeconds, extendWeb,
    hkPending, hkMark, hkApply,
    // null = unknown (no backend/no session) → pills show neutral "—".
    serialOk: diagStat && diagStat.serial ? diagStat.serial.status === "ok" : null,
    mqttStat: diagStat && diagStat.mqtt ? diagStat.mqtt.status : null,
  };

  const SCREENS = [ScreenBoot, ScreenLogin, ScreenSerial, ScreenScan, ScreenSensors, ScreenMqtt, ScreenSecurity, ScreenReview];
  const Current = SCREENS[step];

  return (
    <div id="app">
      <Header ctx={ctx} theme={theme} setTheme={setTheme} />
      {!booting && view === "wizard" && <Stepper ctx={ctx} />}
      <main className="main">
        {booting ? <ScreenSplash />
          : view === "reboot" ? <ScreenReboot ctx={ctx} />
          : view === "dashboard" ? <ScreenDashboard ctx={ctx} />
          : <Current ctx={ctx} />}
      </main>
      {diagOpen && <DiagModal ctx={ctx} initialCapture={diagDirect} onClose={() => { setDiagOpen(false); setDiagDirect(false); }} />}
      {webWarnOpen && <WebWarnModal t={t} seconds={webSeconds} onExtend={extendWeb} onDismiss={() => setWarnDismissed(true)} />}
      {webLocked && <WebLockedOverlay t={t} />}
      {armState === "TRIGGERED" && view !== "dashboard" && !alarmDismissed && (
        <AlarmOverlay L={DASH[lang] || DASH.de}
          meta={(() => {
            if (!liveState || !liveState.sensor_states) return "";
            const fo = liveState.sensor_states.find((x) => x.active);
            if (!fo) return "";
            const s = sensors.find((z) => z.address === fo.address);
            return (s ? s.name : hex(fo.address)) + " · " + hex(fo.address);
          })()}
          onDisarm={() => setDisarmOpen(true)}
          onAck={() => command("reset")}
          onDismiss={() => setAlarmDismissed(true)} />
      )}
      <DisarmModal L={DASH[lang] || DASH.de} open={disarmOpen} onClose={() => setDisarmOpen(false)} onConfirm={(pin) => command("disarm", pin)} />
      <ToastRegion toasts={toasts} />
    </div>
  );
}

export { App };

// Wizard screens S0–S4: boot, login, connection, scan, sensors + wizard chrome.
import React from 'preact/compat';
const { useState, useEffect } = React;
import { API } from './api.js';
import { Icon, Brandmark, Button, Badge, Field, PasswordInput, Modal, Callout, hex, fmtDur } from './ui.jsx';
import { SensorTable } from './sensors.jsx';

// Wizard step list (owned here; app.jsx imports it for routing).
export const STEPS = ["s0", "s1", "s2", "s3", "s4", "s5", "s6", "s8"];

/* ============================================================
   Bridge Setup — Wizard-Screens S0..S8 + Reboot
   ============================================================ */

function WizFooter({ ctx, back = true, nextLabel, onNext, nextDisabled, nextVariant = "primary", hint, children }) {
  return (
    <div className="wiznav">
      <div className="wiznav__inner">
        {back && <Button variant="ghost" iconLeft="arrowLeft" onClick={ctx.back}>{ctx.t("btn.back")}</Button>}
        {children}
        <span className="wiznav__spacer" />
        {hint && <span className="wiznav__hint">{hint}</span>}
        {nextLabel && <Button variant={nextVariant} iconRight={nextVariant === "primary" ? "arrowRight" : null} onClick={onNext || ctx.next} disabled={nextDisabled}>{nextLabel}</Button>}
      </div>
    </div>
  );
}

function PageHead({ ctx, eyebrow, title, sub }) {
  return (
    <div className="page__head">
      {eyebrow && <div className="page__eyebrow">{eyebrow}</div>}
      <h1 className="page__title">{title}</h1>
      {sub && <p className="page__sub">{sub}</p>}
    </div>
  );
}

/* ===================== S0 — BOOT ===================== */
function ScreenBoot({ ctx }) {
  const { t, dev } = ctx;
  return (
    <div className="page page--narrow">
      <div className="card">
        <div className="device-card">
          <Brandmark size={84} />
          <div>
            <div className="page__eyebrow" style={{ textAlign: "center" }}>{t("s0.eyebrow")}</div>
            <h1 className="page__title" style={{ textAlign: "center" }}>{t("s0.title")}</h1>
            <p className="page__sub" style={{ textAlign: "center", margin: "var(--space-3) auto 0" }}>{t("s0.sub")}</p>
          </div>
          <div className="cluster" style={{ justifyContent: "center" }}>
            <Badge tone="ghost" icon="layers">{STEPS.length - 1} {t("s0.steps")}</Badge>
            <Badge tone="ghost" icon="clock">{t("s0.time")}</Badge>
          </div>
          <dl style={{ width: "100%", margin: 0 }}>
            <div className="kv-mono"><dt>{t("s0.model")}</dt><dd>{dev.model}</dd></div>
            <div className="kv-mono"><dt>{t("s0.fw")}</dt><dd>{dev.fw}</dd></div>
            <div className="kv-mono"><dt>{t("s0.mac")}</dt><dd>{dev.mac}</dd></div>
          </dl>
          <Button variant="primary" size="lg" block iconRight="arrowRight" onClick={ctx.next}>{t("s0.start")}</Button>
        </div>
      </div>
      <div style={{ marginTop: "var(--space-4)" }}>
        <Callout tone="info" title={t("s0.vdstitle")}>{t("s0.vdsbody")}</Callout>
      </div>
    </div>
  );
}

/* ===================== S1 — LOGIN ===================== */
function ScreenLogin({ ctx }) {
  const { t } = ctx;
  const [phase, setPhase] = useState("login"); // login | change
  const [pw, setPw] = useState("");
  const [np, setNp] = useState(""); const [np2, setNp2] = useState("");
  const [help, setHelp] = useState(false);
  const strength = np.length === 0 ? 0 : np.length < 8 ? 1 : np.length < 12 ? 2 : 3;
  const matchErr = np2.length > 0 && np !== np2 ? t("s1.pwnomatch") : null;
  const canSet = np.length >= 8 && np === np2;

  return (
    <div className="page page--narrow">
      {phase === "login" ? (
        <div className="card"><div className="card__body stack stack-5">
          <PageHead ctx={ctx} eyebrow={t("s1.eyebrow")} title={t("s1.title")} sub={t("s1.sub")} />
          {ctx.dev && ctx.dev.configured && <Callout tone="accent" icon="check">{t("s1.configured")}</Callout>}
          <Field label={t("s1.pw")}>
            <PasswordInput value={pw} onChange={setPw} placeholder={t("s1.pwph")} autoFocus />
          </Field>
          <Button variant="ghost" size="sm" iconLeft="help" style={{ alignSelf: "flex-start" }} onClick={() => setHelp(true)}>{t("s1.where")}</Button>
          <Callout tone="accent" icon="shield">{t("s1.threat")}</Callout>
          <Button variant="primary" size="lg" block disabled={!pw} onClick={async () => {
            if (ctx.live && API) {
              try { const d = await API.login(pw); if (d.password_change_required) setPhase("change"); else ctx.enterWizard(); }
              catch (e) { ctx.toast("err", t("st.authfail")); }
            } else setPhase("change");
          }}>{t("s1.login")}</Button>
        </div></div>
      ) : (
        <div className="card"><div className="card__body stack stack-5">
          <PageHead ctx={ctx} eyebrow={t("s1.eyebrow")} title={t("s1.changetitle")} sub={t("s1.changesub")} />
          <Field label={t("s1.newpw")} hint={t("s1.pwmin")}>
            <PasswordInput value={np} onChange={setNp} autoFocus />
            <div className="pw-meter">
              {[1, 2, 3].map((i) => <span key={i} className={`pw-meter__seg${strength >= i ? " pw-meter__seg--on" + strength : ""}`} />)}
            </div>
            {np.length > 0 && <span className="field__hint">{["", t("s1.pwweak"), t("s1.pwmedium"), t("s1.pwstrong")][strength]}</span>}
          </Field>
          <Field label={t("s1.newpw2")} error={matchErr}>
            <PasswordInput value={np2} onChange={setNp2} error={!!matchErr} />
          </Field>
          <Button variant="primary" size="lg" block disabled={!canSet} iconRight="arrowRight" onClick={async () => {
            if (ctx.live && API) { try { await API.setPassword(np); } catch (e) { ctx.toast("err", (e && e.message) || "Fehler"); return; } }
            ctx.toast("ok", t("s1.changetitle")); ctx.next();
          }}>{t("s1.setpw")}</Button>
        </div></div>
      )}
      <Modal open={help} onClose={() => setHelp(false)} title={t("s1.where")} icon="help"
        footer={<Button variant="primary" onClick={() => setHelp(false)}>{t("btn.close")}</Button>}>
        <p style={{ color: "var(--fg-muted)" }}>{t("s1.wherebody")}</p>
      </Modal>
    </div>
  );
}

/* ===================== S2 — CONNECTION ===================== */
function ScreenSerial({ ctx }) {
  const { t, conn, setConn } = ctx;
  const [state, setState] = useState(ctx.live ? "idle" : "checking"); // idle | checking | ok | fail
  const [res, setRes] = useState(null); // last check result: { detail, last_frame_ms }
  const [diagMode, setDiagMode] = useState(false); // shortcut to GMS capture
  const isTcp = conn.type === "tcp";
  const setType = (type) => { setConn({ ...conn, type }); setState(type === "tcp" || ctx.live ? "idle" : "checking"); if (type !== "tcp" && !ctx.live) setTimeout(() => setState("ok"), 1500); };
  const runCheck = () => {
    setState("checking");
    if (ctx.live && API) {
      const done = (r) => { setRes(r); setState(r.state === "ok" ? "ok" : "fail"); };
      API.putConnection(conn.type, conn.ip, conn.port, conn)
        .then(() => API.checkConnection())
        .then((r) => {
          if (r.state !== "checking") { done(r); return; }
          // TCP probe runs asynchronously (connect 2s + listen window 4s) → poll
          let n = 0;
          const id = setInterval(async () => {
            n++;
            try {
              const r2 = await API.getConnectionCheck();
              if (r2.state === "ok" || r2.state === "error") { clearInterval(id); done(r2); }
              else if (n > 24) { clearInterval(id); done({ state: "error", detail: "connect" }); }
            } catch (_) { clearInterval(id); done({ state: "error", detail: "connect" }); }
          }, 500);
        })
        .catch(() => done({ state: "error", detail: "connect" }));
      return;
    }
    setTimeout(() => setState("ok"), 1500);
  };
  // eslint-disable-next-line react-hooks/exhaustive-deps -- offline mock: one-shot "connected" simulation on mount only
  useEffect(() => { if (!isTcp && !ctx.live) { const id = setTimeout(() => setState("ok"), 1600); return () => clearTimeout(id); } }, []);

  return (
    <div className="page page--mid">
      <PageHead ctx={ctx} eyebrow={t("s2.eyebrow")} title={t("s2.title")} sub={t("s2.sub")} />

      <div className="field__label" style={{ marginBottom: "var(--space-2)" }}>{t("s2.panel")}</div>
      <div role="radiogroup" aria-label={t("s2.panel")} style={{ display: "grid", gridTemplateColumns: "1fr 1fr", alignItems: "stretch", gap: "var(--space-3)", marginBottom: "var(--space-4)" }}>
        {[
          { kind: "complex400", title: t("s2.panelcomplex"), desc: t("s2.panelcomplexdesc") },
          { kind: "hiplex8400", title: t("s2.panelhiplex"), desc: t("s2.panelhiplexdesc") },
        ].map((o) => {
          const sel = conn.panel === o.kind;
          return (
            <div key={o.kind} className="card" role="radio" aria-checked={sel} tabIndex={0}
              onClick={() => setConn({ ...conn, panel: o.kind, baud: o.kind === "hiplex8400" && conn.gms === "plus" ? "115200" : "9600" })}
              onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); setConn({ ...conn, panel: o.kind }); } }}
              style={{ margin: 0, cursor: "pointer", borderColor: sel ? "var(--accent)" : undefined, background: sel ? "var(--accent-soft)" : undefined }}>
              <div className="card__body" style={{ display: "flex", alignItems: "center", gap: "var(--space-3)" }}>
                <span className="modal__icon modal__icon--accent" style={{ width: 40, height: 40, flex: "none" }}><Icon name="shield" size={20} /></span>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontWeight: "var(--fw-semibold)", fontSize: "var(--text-sm)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{o.title}</div>
                  <div style={{ color: "var(--fg-muted)", fontSize: "var(--text-sm)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{o.desc}</div>
                </div>
                {sel && <Badge tone="ok" icon="check" />}
              </div>
            </div>
          );
        })}
      </div>
      {conn.panel === "hiplex8400" && (
        <div className="card" style={{ marginBottom: "var(--space-4)" }}><div className="card__body">
          <div className="field__row">
            <Field label={t("s2.gms")}>
              <select className="input" value={conn.gms}
                onChange={(e) => { const gms = e.target.value; setConn({ ...conn, gms, baud: gms === "plus" ? "115200" : "9600" }); }}>
                <option value="lite">{t("s2.gmslite")}</option>
                <option value="plus">{t("s2.gmsplus")}</option>
              </select>
            </Field>
            <Field label={t("s2.baudsel")}>
              <select className="input input--mono" value={conn.baud}
                onChange={(e) => setConn({ ...conn, baud: e.target.value })}>
                <option value="9600">9600</option>
                <option value="115200">115200</option>
              </select>
            </Field>
          </div>
          {conn.type === "tcp" && <p style={{ color: "var(--fg-subtle)", fontSize: "var(--text-xs)", margin: "var(--space-2) 0 0" }}>{t("s2.baudtcpnote")}</p>}
          <label style={{ display: "flex", alignItems: "flex-start", gap: "var(--space-2)", marginTop: "var(--space-3)", cursor: "pointer", fontSize: "var(--text-xs)" }}>
            <input type="checkbox" checked={!!conn.cmdsVerified} onChange={(e) => setConn({ ...conn, cmdsVerified: e.target.checked })}
              style={{ marginTop: "1px", accentColor: "var(--danger, #d33)" }} />
            <span style={{ color: "var(--danger, #d33)" }}>{t("s2.cmdsverified")}</span>
          </label>
          <p style={{ color: "var(--fg-subtle)", fontSize: "var(--text-xs)", margin: "var(--space-2) 0 0" }}>{t("s2.cmdswarn")}</p>
        </div></div>
      )}

      <div className="field__label" style={{ marginBottom: "var(--space-2)" }}>{t("s2.conntype")}</div>
      <div role="radiogroup" aria-label={t("s2.conntype")} style={{ display: "grid", gridTemplateColumns: "1fr 1fr", alignItems: "stretch", gap: "var(--space-3)", marginBottom: "var(--space-4)" }}>
        {[
          { type: "internal", sel: !isTcp, icon: "server", title: t("s2.internalcard"), desc: t("s2.internaldesc") },
          { type: "tcp", sel: isTcp, icon: "link", title: t("s2.tcpcard"), desc: t("s2.tcpdesc") },
        ].map((o) => (
          <div key={o.type} className="card" role="radio" aria-checked={o.sel} tabIndex={0}
            onClick={() => setType(o.type)}
            onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); setType(o.type); } }}
            style={{ margin: 0, cursor: "pointer", borderColor: o.sel ? "var(--accent)" : undefined, background: o.sel ? "var(--accent-soft)" : undefined }}>
            <div className="card__body" style={{ display: "flex", alignItems: "center", gap: "var(--space-3)" }}>
              <span className="modal__icon modal__icon--accent" style={{ width: 40, height: 40, flex: "none" }}><Icon name={o.icon} size={20} /></span>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: "var(--fw-semibold)", fontSize: "var(--text-sm)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{o.title}</div>
                <div style={{ color: "var(--fg-muted)", fontSize: "var(--text-sm)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{o.desc}</div>
              </div>
              {o.sel && <Badge tone="ok" icon="check" />}
            </div>
          </div>
        ))}
      </div>
      {isTcp && (
        <div className="card" style={{ marginBottom: "var(--space-4)" }}><div className="card__body">
          <div className="field__row">
            <Field label={t("s2.ip")}><input className="input input--mono" value={conn.ip} onChange={(e) => { setConn({ ...conn, ip: e.target.value }); setState("idle"); }} placeholder="192.168.1.50" /></Field>
            <div className="field--port"><Field label={t("s5.port")}><input className="input input--mono" value={conn.port} onChange={(e) => { setConn({ ...conn, port: e.target.value }); setState("idle"); }} /></Field></div>
          </div>
        </div></div>
      )}

      <div className="card"><div className="card__body">
        {state === "idle" ? (
          <Button variant="primary" iconLeft="plug" onClick={runCheck}>{t("s2.check")}</Button>
        ) : state === "checking" ? (
          <div className="stack stack-4" style={{ alignItems: "center", padding: "var(--space-6) 0" }}>
            <div className="progress-track" style={{ width: "60%" }}><div className="progress-fill progress-fill--indet" /></div>
            <span style={{ color: "var(--fg-muted)" }}>{t("s2.checking")}</span>
          </div>
        ) : state === "fail" ? (
          <div className="stack stack-5">
            <div className="cluster"><Badge tone="alarm" icon="alert">{t("s2.failtitle")}</Badge></div>
            <p style={{ color: "var(--fg-muted)", margin: 0 }}>
              {t(res?.detail === "no_frame" ? "s2.fail.noframe" : "s2.fail.connect")}
            </p>
            <div>
              <div className="field__label">{t("s2.recovery")}</div>
              <ul style={{ color: "var(--fg-muted)", margin: "var(--space-2) 0 0", paddingLeft: "1.2em" }}>
                {(isTcp && res?.detail !== "no_frame" ? ["s2.rec0"] : ["s2.rec1", "s2.rec2", "s2.rec3"])
                  .map((k) => <li key={k}>{t(k)}</li>)}
              </ul>
            </div>
            <Button variant="secondary" size="sm" iconLeft="refresh" style={{ alignSelf: "flex-start" }} onClick={runCheck}>{t("s2.recheck")}</Button>
          </div>
        ) : (
          <div className="stack stack-5">
            <div className="cluster"><Badge tone="ok" icon="check">{t("s2.ok")}</Badge></div>
            <p style={{ color: "var(--fg-muted)", margin: 0 }}>{t("s2.okbody")}</p>
            <dl className="deflist" style={{ margin: 0 }}>
              {isTcp ? (
                <>
                  <dt>{t("s2.tcp")}</dt><dd className="mono">{conn.ip}:{conn.port}</dd>
                  <dt>{t("s2.lastframe")}</dt><dd className="mono">{res?.last_frame_ms != null ? `${res.last_frame_ms} ms` : "—"}</dd>
                </>
              ) : (
                <>
                  <dt>{t("s2.lastframe")}</dt><dd className="mono">{res?.last_frame_ms != null ? `${res.last_frame_ms} ms` : "—"}</dd>
                  <dt>{t("s2.baud")}</dt><dd className="mono">{conn.baud || "9600"} 8N1</dd>
                </>
              )}
            </dl>
            <Button variant="secondary" size="sm" iconLeft="refresh" style={{ alignSelf: "flex-start" }} onClick={runCheck}>{t("s2.recheck")}</Button>
          </div>
        )}
      </div></div>
      <label style={{
        display: "flex", alignItems: "flex-start", gap: "var(--space-2)",
        marginTop: "var(--space-3)", cursor: "pointer",
        fontSize: "var(--text-xs)", color: "var(--fg-subtle)",
      }}>
        <input type="checkbox" checked={diagMode} onChange={(e) => setDiagMode(e.target.checked)}
          style={{ marginTop: "1px", accentColor: "var(--accent)" }} />
        <span>{t("s2.diagmode")}</span>
      </label>
      <WizFooter ctx={ctx}
        nextLabel={diagMode ? t("s2.diagbtn") : t("btn.next")}
        nextVariant={diagMode ? "secondary" : "primary"}
        onNext={diagMode ? () => ctx.openDiag(true) : undefined}
        nextDisabled={diagMode ? false : state !== "ok"} />
    </div>
  );
}

/* ===================== S3 — SCAN ===================== */
function ScreenScan({ ctx }) {
  const { t, scan } = ctx;
  const [confirmCancel, setConfirmCancel] = useState(false);
  // Bar = scanned addresses (not named) → advances even when no name is found.
  const pct = scan.total > 0 ? Math.round((scan.scanned / scan.total) * 100) : 0;

  if (scan.phase === "idle") {
    return (
      <div className="page page--mid">
        <PageHead ctx={ctx} eyebrow={t("s3.eyebrow")} title={t("s3.title")} sub={t("s3.sub")} />
        <div className="card"><div className="card__body stack stack-5">
          <Callout tone="info" icon="info">{t("s3.beforenote")}</Callout>
        </div></div>
        <WizFooter ctx={ctx} back nextLabel={t("s3.start")} onNext={ctx.startScan} />
      </div>
    );
  }

  const done = scan.phase === "done";
  return (
    <div className="page page--mid">
      <PageHead ctx={ctx} eyebrow={t("s3.eyebrow")} title={done ? t("s3.done") : t("s3.scanning")} />
      <div className="card"><div className="card__body stack stack-5">
        <div className="cluster" style={{ gap: "var(--space-2)" }}>
          <span className={`phase-pill ${scan.phase === "belegt" ? "phase-pill--active" : "phase-pill--done"}`}>
            {scan.phase !== "belegt" && <Icon name="check" size={14} />}{t("s3.phase1")}</span>
          <Icon name="chevronRight" size={16} style={{ color: "var(--fg-subtle)" }} />
          <span className={`phase-pill ${scan.phase === "naming" ? "phase-pill--active" : done ? "phase-pill--done" : ""}`}>
            {done && <Icon name="check" size={14} />}{t("s3.phase2")}</span>
        </div>

        {scan.phase === "belegt" ? (
          <div className="stack stack-3">
            <div className="progress-track"><div className="progress-fill progress-fill--indet" /></div>
            <span style={{ color: "var(--fg-muted)" }}>{t("s3.phase1")}…</span>
          </div>
        ) : (
          <div className="stack stack-5">
            <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: "var(--space-4)" }}>
              <div style={{ position: "relative", width: 132, height: 132 }}>
                <svg width="132" height="132" viewBox="0 0 132 132" style={{ transform: "rotate(-90deg)" }} aria-hidden="true">
                  <circle className="scanring__bg" cx="66" cy="66" r="54" fill="none" strokeWidth="8" />
                  <circle className="scanring__fg" cx="66" cy="66" r="54" fill="none" strokeWidth="8" strokeLinecap="round"
                    strokeDasharray={2 * Math.PI * 54} strokeDashoffset={2 * Math.PI * 54 * (1 - pct / 100)} />
                </svg>
                <div style={{ position: "absolute", inset: 0, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 1 }}>
                  <span className="mono" style={{ fontSize: 30, fontWeight: "var(--fw-semibold)", lineHeight: 1, letterSpacing: "-0.02em" }}>{pct}%</span>
                  <span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("s3.checked")}</span>
                </div>
              </div>
              <div className="mono" style={{ color: "var(--fg-muted)", fontSize: "var(--text-sm)" }}>{scan.scanned}/{scan.total} {t("s3.addresses")}</div>
            </div>
            <div className="stat-grid">
              <div className="scan-stat" style={{ alignItems: "center", textAlign: "center" }}><span className="scan-stat__val">{scan.named}</span><span className="scan-stat__lbl">{t("s3.named")}</span></div>
              <div className="scan-stat" style={{ alignItems: "center", textAlign: "center" }}><span className="scan-stat__val">{fmtDur(scan.elapsed)}</span><span className="scan-stat__lbl">{t("s3.elapsed")}</span></div>
              <div className="scan-stat" style={{ alignItems: "center", textAlign: "center" }}><span className="scan-stat__val">{done ? "0:00" : "~" + Math.max(1, Math.ceil(scan.remaining / 60)) + " " + t("s3.min")}</span><span className="scan-stat__lbl">{t("s3.remaining")}</span></div>
            </div>
            {!done && <p style={{ color: "var(--fg-subtle)", fontSize: "var(--text-sm)", margin: 0 }}>{t("s3.namesnote")}</p>}
          </div>
        )}

        {!done && <Callout tone="info" icon="info">{t("s3.safenote")}</Callout>}
        {done && <Callout tone="ok" icon="check">{t("s3.donebody", { n: scan.total })}</Callout>}

        <div>
          <div className="field__label" style={{ marginBottom: "var(--space-2)" }}>{t("s3.feedtitle")}</div>
          <div className="scan-feed scroll" aria-live="polite">
            {scan.feed.slice().reverse().map((s) => (
              <div className="scan-feed__row" key={s.address}>
                <span className="cell-mono">{hex(s.address)}</span>
                <span style={{ flex: 1 }}>{s.name || <em style={{ color: "var(--fg-subtle)" }}>{s.raw_name}</em>}</span>
                <Badge tone="unconfirmed">{t("stat.unconfirmed")}</Badge>
              </div>
            ))}
            {scan.feed.length === 0 && <div className="scan-feed__row" style={{ color: "var(--fg-subtle)" }}>…</div>}
          </div>
        </div>
      </div></div>
      <WizFooter ctx={ctx} back={!done}
        nextLabel={done ? t("s3.toresults") : null}
        onNext={ctx.next}>
        {!done && <Button variant="primary" size="sm" iconRight="arrowRight" onClick={() => ctx.goto(5)}>{t("s3.usewait")}</Button>}
        {!done && <Button variant="danger-soft" size="sm" iconLeft="x" onClick={() => setConfirmCancel(true)}>{t("s3.cancel")}</Button>}
      </WizFooter>

      <Modal open={confirmCancel} onClose={() => setConfirmCancel(false)} title={t("s3.canceltitle")} icon="alert" iconTone="warn"
        footer={<><Button variant="ghost" onClick={() => { ctx.discardScan(); setConfirmCancel(false); }}>{t("s3.discard")}</Button>
          <Button variant="primary" onClick={() => { ctx.keepPartial(); setConfirmCancel(false); }}>{t("s3.keeppartial")}</Button></>}>
        <p style={{ color: "var(--fg-muted)" }}>{t("s3.cancelbody", { done: scan.named, total: scan.total })}</p>
      </Modal>
    </div>
  );
}

/* ===================== S4 — SENSORS ===================== */
function ScreenSensors({ ctx }) {
  const { t } = ctx;
  // Scan still running (live) → results only available at the end. Show a wait state with
  // progress + jump back to scan or forward to config instead of a partial list.
  const scanning = ctx.live && (ctx.scan.phase === "belegt" || ctx.scan.phase === "naming");
  // Live: load current sensor list from the device (covers --config-seed + re-entry).
  useEffect(() => {
    if (ctx.live && API && !scanning) {
      API.getSensors().then((r) => {
        ctx.setSensors(r.sensors.map(API.normSensor));
      }).catch(() => ctx.toast("err", t("s4.loadfail")));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reload sensors when the scan finishes only; ctx/t are app-stable
  }, [scanning]);
  if (scanning) {
    return (
      <div className="page page--mid">
        <PageHead ctx={ctx} eyebrow={t("s4.eyebrow")} title={t("s4.title")} sub={t("s4.sub")} />
        <div className="card"><div className="card__body stack stack-4" style={{ alignItems: "center", padding: "var(--space-6)" }}>
          <div className="progress-track" style={{ width: "60%" }}><div className="progress-fill" style={{ width: (ctx.scan.total > 0 ? Math.round(ctx.scan.scanned / ctx.scan.total * 100) : 0) + "%" }} /></div>
          <p style={{ color: "var(--fg-muted)", textAlign: "center", margin: 0 }}>{t("s4.scanwait", { scanned: ctx.scan.scanned, total: ctx.scan.total, named: ctx.scan.named })}</p>
          <div className="cluster" style={{ gap: "var(--space-3)" }}>
            <Button variant="secondary" iconLeft="search" onClick={() => ctx.goto(3)}>{t("s4.toscan")}</Button>
            <Button variant="primary" iconRight="arrowRight" onClick={() => ctx.goto(5)}>{t("s3.usewait")}</Button>
          </div>
        </div></div>
      </div>
    );
  }
  return (
    <div className="page">
      <PageHead ctx={ctx} eyebrow={t("s4.eyebrow")} title={t("s4.title")} sub={t("s4.sub")} />
      <SensorTable ctx={ctx} sensors={ctx.sensors} setSensors={ctx.setSensors} t={t} lang={ctx.lang} toast={ctx.toast} />
      {ctx.mqtt && ctx.mqtt.homekit_mode && ctx.hkPending.size > 0 && (
        <div className="hk-applybar">
          <span className="hk-applybar__txt"><Icon name="spark" size={16} /> {t("hk.pending", { n: ctx.hkPending.size })}</span>
          <Button variant="primary" iconLeft="check" onClick={ctx.hkApply}>{t("hk.applybtn")}</Button>
        </div>
      )}
      <WizFooter ctx={ctx} nextLabel={t("btn.next")} />
    </div>
  );
}
export { WizFooter, PageHead, ScreenBoot, ScreenLogin, ScreenSerial, ScreenScan, ScreenSensors };

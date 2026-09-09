// Final step S8: review/commit + reboot screen.
import React from 'preact/compat';
const { useState, useEffect } = React;
import { Badge, Button, Callout, Checkbox, Icon } from './ui.jsx';
import { PageHead, WizFooter } from './wizard.jsx';


function ScreenReview({ ctx }) {
  const { t, sensors, mqtt, sec } = ctx;
  const [ack, setAck] = useState(false);
  const [saving, setSaving] = useState(false);
  const savingRef = React.useRef(false);
  const save = async () => {
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    try { await ctx.commit(); }
    finally { savingRef.current = false; setSaving(false); }
  };
  const conf = sensors.filter((s) => s.status === "confirmed").length;
  const excl = sensors.filter((s) => s.status === "excluded").length;
  const unconf = sensors.filter((s) => s.status === "unconfirmed").length;
  const polUnclear = sensors.filter((s) => s.polarity === "unconfirmed" && s.status !== "excluded").length;
  const warnings = [];
  if (unconf > 0) warnings.push({ tone: "warn", msg: t("s8.w.unconf", { n: unconf }) });
  if (polUnclear > 0) warnings.push({ tone: "warn", msg: t("s8.w.pol", { n: polUnclear }) });
  const swSiren = sensors.filter((s) => s.switchable && s.kind === "signalgeber" && s.status !== "excluded").length;
  if (swSiren > 0) warnings.push({ tone: "alarm", msg: t("s8.w.swsiren", { n: swSiren }) });
  // MQTT test warning only in MQTT mode (in HomeKit mode MQTT is off → irrelevant).
  if (!mqtt.homekit_mode) warnings.push({ tone: "warn", msg: t("s8.w.mqtt") });
  // Active disarm paths named explicitly (both can be open simultaneously → different channels).
  if (mqtt.homekit_mode && mqtt.homekit_disarm) warnings.push({ tone: "alarm", msg: t("s8.w.hkdisarm") });
  if (sec.remoteDisarm) warnings.push({ tone: "alarm", msg: t("s8.w.remote") });
  const needAck = warnings.length > 0;
  // Commit only once the scan is done (the user can jump ahead while it runs).
  const scanning = ctx.live && (ctx.scan.phase === "belegt" || ctx.scan.phase === "naming");

  return (
    <div className="page page--mid">
      <PageHead ctx={ctx} eyebrow={t("s8.eyebrow")} title={t("s8.title")} sub={t("s8.sub")} />
      <div className="card"><div className="card__body">
        <dl className="deflist">
          <dt>{t("s8.sensors")}</dt><dd><Badge tone="ok" icon="check">{conf} {t("s8.sconf")}</Badge> <Badge tone="unconfirmed">{unconf} {t("s8.sunconf")}</Badge> <Badge tone="unavail">{excl} {t("s8.sexcl")}</Badge></dd>
          <dt>{t("s2.conntype")}</dt><dd>{ctx.conn.type === "tcp" ? <span><span className="mono">{ctx.conn.ip}:{ctx.conn.port}</span> · {t("s2.tcp")}</span> : t("s2.internal")}</dd>
          {mqtt.homekit_mode ? (
            <><dt>{t("s8.integration")}</dt><dd>Apple HomeKit</dd></>
          ) : (<>
            <dt>{t("s8.integration")}</dt><dd>Home Assistant / MQTT</dd>
            <dt>{t("s8.mqtt")}</dt><dd className="mono">{mqtt.tls ? "mqtts" : "mqtt"}://{mqtt.host}:{mqtt.port}</dd>
            <dt>{t("s8.hadisc")}</dt><dd>{mqtt.ha_discovery ? t("s8.on") : t("s8.off")}</dd>
          </>)}
          <dt>{t("s8.remote")}</dt><dd>{sec.remoteDisarm ? t("s8.on") : t("s8.off")}</dd>
          {mqtt.homekit_mode && <><dt>{t("s8.hkdisarm")}</dt><dd>{mqtt.homekit_disarm ? t("s8.on") : t("s8.off")}</dd></>}
        </dl>
      </div></div>

      {warnings.length > 0 && (
        <div className="card"><div className="card__body stack stack-3">
          <div className="field__label">{t("s8.warnings")}</div>
          {warnings.map((w, i) => <Callout key={i} tone={w.tone} icon="alert">{w.msg}</Callout>)}
          <label className="toggle" style={{ alignItems: "flex-start", gap: "var(--space-3)", marginTop: "var(--space-2)" }}>
            <Checkbox checked={ack} onChange={() => setAck(!ack)} />
            <span style={{ fontSize: "var(--text-sm)" }}>{t("s8.ackwarn")}</span>
          </label>
        </div></div>
      )}

      {scanning && <div style={{ marginTop: "var(--space-3)" }}><Callout tone="info" icon="info">{t("s8.scanrunning")}</Callout></div>}
      <div style={{ marginTop: "var(--space-3)" }}><Callout tone="info" title={t("s0.vdstitle")}>{t("s8.vds")}</Callout></div>
      <WizFooter ctx={ctx} nextLabel={saving ? "Speichern wird geprüft ..." : t("s8.commit")} nextVariant="primary" nextDisabled={saving || (needAck && !ack) || scanning} hint={scanning ? t("s8.scanwait") : null} onNext={save} />
    </div>
  );
}

/* ===================== REBOOT ===================== */
function ScreenReboot({ ctx }) {
  const { t } = ctx;
  const [phase, setPhase] = useState("saving"); // saving | done (brief animation before success)
  useEffect(() => { const id = setTimeout(() => setPhase("done"), 2200); return () => clearTimeout(id); }, []);
  return (
    <div className="page page--narrow">
      <div className="card"><div className="device-card">
        {phase === "saving" ? (
          <>
            <div className="progress-track" style={{ width: "70%" }}><div className="progress-fill progress-fill--indet" /></div>
            <h1 className="page__title">{t("s8.committing")}</h1>
          </>
        ) : (
          <>
            <span className="modal__icon modal__icon--accent" style={{ width: 64, height: 64 }}><Icon name="check" size={32} /></span>
            <h1 className="page__title" style={{ textAlign: "center" }}>{t("rb.title")}</h1>
            <p className="page__sub" style={{ textAlign: "center" }}>{t("rb.body")}</p>
            <div className="setup-flag" style={{ background: "var(--bg-inset)", color: "var(--fg-muted)", border: "1px solid var(--border-default)" }}>
              <Icon name="power" size={14} />{t("rb.rebooting")}</div>
            {!ctx.live && <Button variant="ghost" size="sm" iconLeft="refresh" onClick={() => ctx.restart()}>Setup neu starten (Demo)</Button>}
          </>
        )}
      </div></div>
    </div>
  );
}

/* ===================== LIVE-TEST & STEUERUNG (isoliert) ===================== */
export { ScreenReview, ScreenReboot };

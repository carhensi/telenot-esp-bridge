// Live view: arm segments, alarm hero/overlay, disarm modal, dashboard.
import React from 'preact/compat';
const { useState, useEffect } = React;
import { API } from './api.js';
import { MOCK } from './mock.js';
import { Badge, Button, Callout, Field, hex, Icon, Modal } from './ui.jsx';
import { PageHead } from './wizard.jsx';
import { KIND_LABELS } from './sensors.jsx';


const DASH = {
  de: {
    title: "Live-Test & Steuerung", eyebrow: "LIVE-TEST",
    title_op: "Status & Steuerung", eyebrow_op: "BETRIEB",
    sub: "Isolierter Test gegen die Anlage: Melder-Status live + Scharf/Unscharf. Wirkt auf die ECHTE Anlage.",
    sub_op: "Melder-Status live, Scharf/Unscharf und Diagnose. Zur Konfiguration über das Schild-Symbol oben bzw. den Button unten.",
    arm: "Alarmzustand", open: "Offene Melder",
    allclosed: "Alle Melder geschlossen", active: "{n} von {total} aktiv",
    control: "Steuerung", away: "Extern scharf", home: "Intern scharf", night: "Nacht scharf", disarm: "Unscharf", reset: "Zurücksetzen",
    alarm_eyebrow: "Alarm ausgelöst", alarm_title: "Einbruchalarm", alarm_disarm: "Unscharf & Zurücksetzen", alarm_ack: "Quittieren",
    seg_ext_d: "Vollschutz · alle Bereiche", seg_int_d: "Teilschutz · Bewegungsmelder aus", seg_off_d: "Kein Schutz · erfordert PIN",
    open_expand: "{n} offen", more: "+{n} weitere", less: "Weniger anzeigen", notready: "Nicht bereit · Melder offen",
    notconn: "Anlage nicht verbunden", offline: "Keine Verbindung zur Anlage — Steuerung deaktiviert.", nodata: "Keine Daten",
    pinph: "Disarm-PIN", ready_i: "intern bereit", ready_e: "extern bereit", back: "Zum Setup", back_op: "Konfiguration",
    pintitle: "Anlage unscharf schalten", pinlabel: "Benutzer-PIN", cancel: "Abbrechen", disarmconfirm: "Unscharf schalten",
    pinbody: "Zum Bestätigen deine Benutzer-PIN eingeben. Der Befehl wirkt auf die echte Anlage.",
    unconf: "unbestätigt", lastcmd: "Letztes Befehls-Ergebnis", ago: "vor {s} s",
    warn: "Befehle wirken auf die ECHTE Anlage. Scharfschalten ist ohne PIN möglich (sofern die Anlage bereit ist); Unscharfschalten nur mit aktiviertem Remote-Disarm (Sicherheits-Schritt) + PIN — fail-closed.",
    warn_op: "Scharfschalten ist ohne PIN möglich (sofern die Anlage bereit ist). Unscharfschalten nur mit aktiviertem Remote-Disarm (Sicherheits-Schritt) + PIN — fail-closed.",
    st: { DISARMED: "Unscharf", ARMED_HOME: "Intern scharf", ARMED_NIGHT: "Nacht scharf", ARMED_AWAY: "Extern scharf", TRIGGERED: "ALARM", unknown: "—" },
  },
  en: {
    title: "Live test & control", eyebrow: "LIVE TEST",
    title_op: "Status & control", eyebrow_op: "OPERATION",
    sub: "Isolated test against the panel: live sensor states + arm/disarm. Acts on the REAL panel.",
    sub_op: "Live sensor states, arm/disarm and diagnostics. Configuration via the shield icon or \"To configuration\".",
    arm: "Alarm state", open: "Open sensors",
    allclosed: "All sensors closed", active: "{n} of {total} active",
    control: "Control", away: "Arm away", home: "Arm home", night: "Night", disarm: "Disarm", reset: "Reset",
    alarm_eyebrow: "Alarm triggered", alarm_title: "Burglar alarm", alarm_disarm: "Disarm & reset", alarm_ack: "Acknowledge",
    seg_ext_d: "Full protection · all areas", seg_int_d: "Partial · motion sensors off", seg_off_d: "No protection · requires PIN",
    open_expand: "{n} open", more: "+{n} more", less: "Show less", notready: "Not ready · sensor open",
    notconn: "Panel not connected", offline: "No connection to the panel — controls disabled.", nodata: "No data",
    pinph: "Disarm PIN", ready_i: "internal ready", ready_e: "external ready", back: "To setup", back_op: "Configuration",
    pintitle: "Disarm the system", pinlabel: "User PIN", cancel: "Cancel", disarmconfirm: "Disarm",
    pinbody: "Enter your user PIN to confirm. The command acts on the real panel.",
    unconf: "unconfirmed", lastcmd: "Last command result", ago: "{s} s ago",
    warn: "Commands act on the REAL panel. Arming needs no PIN (as long as the panel is ready); disarming requires remote-disarm enabled (security step) + PIN — fail-closed.",
    warn_op: "Arming needs no PIN (as long as the panel is ready). Disarming requires remote-disarm enabled (security step) + PIN — fail-closed.",
    st: { DISARMED: "Disarmed", ARMED_HOME: "Armed home", ARMED_NIGHT: "Armed night", ARMED_AWAY: "Armed away", TRIGGERED: "ALARM", unknown: "—" },
  },
};

// One segment of the vertical arm switcher. The active segment shows its tone
// (armed=blue / warn=amber / disarmed=neutral) via inline styles + the --on class.
// Not ready to arm (panel reports ready=false) → grey + non-clickable (disabled).
// The active segment is non-clickable too: re-sending the current state is a no-op
// (and for "disarmed" would pointlessly open the PIN modal).
function ArmSeg({ on, ready = true, tone, icon, title, desc, notReadyLabel, onClick }) {
  const blocked = !on && !ready;
  const segStyle = on ? { borderColor: `var(--state-${tone}-border)`, background: `var(--state-${tone}-bg)` } : undefined;
  // active → filled tone icon; ready-but-inactive → tone-colored icon (visibly clickable);
  // not ready → CSS (.armseg--notready) mutes everything to grey.
  const icoStyle = on ? { background: `var(--state-${tone}-solid)`, color: "#fff" }
    : blocked ? undefined
    : { color: `var(--state-${tone}-solid)` };
  return (
    <button className={`armseg${on ? " armseg--on" : ""}${blocked ? " armseg--notready" : ""}`} style={segStyle} onClick={onClick} disabled={blocked || on}>
      <span className="armseg__ico" style={icoStyle}><Icon name={icon} size={22} /></span>
      <span style={{ flex: 1, minWidth: 0 }}>
        <span className="armseg__t" style={{ display: "block" }}>{title}</span>
        <span className="armseg__d" style={{ display: "block" }}>{blocked ? notReadyLabel : desc}</span>
      </span>
      <Icon name="checkcircle" size={22} className="armseg__check" style={{ color: `var(--state-${tone}-solid)` }} />
    </button>
  );
}

// Pulsing alarm hero — shared between the dashboard inline and the global overlay.
function AlarmHero({ L, meta, onDisarm, onAck }) {
  return (
    <div className="alarmhero">
      <div className="alarmhero__top">
        <span className="alarmhero__ico"><Icon name="bell" size={28} /></span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className="alarmhero__eyebrow"><span className="alarmhero__dot" />{L.alarm_eyebrow}</div>
          <div className="alarmhero__state">{L.alarm_title}</div>
          {meta && <div className="alarmhero__meta">{meta}</div>}
        </div>
      </div>
      <div className="alarmhero__acts">
        <Button variant="danger" iconLeft="unlock" onClick={onDisarm}>{L.alarm_disarm}</Button>
        <Button variant="secondary" iconLeft="refresh" onClick={onAck}>{L.alarm_ack}</Button>
      </div>
    </div>
  );
}

// Global alarm overlay: appears on every screen except the dashboard (where the inline hero suffices).
function AlarmOverlay({ L, meta, onDisarm, onAck, onDismiss }) {
  return (
    <div className="alarm-scrim">
      <div className="alarm-overlay">
        <AlarmHero L={L} meta={meta} onDisarm={onDisarm} onAck={onAck} />
        <Button variant="ghost" className="alarm-overlay__dismiss" onClick={onDismiss}>{L.cancel}</Button>
      </div>
    </div>
  );
}

// Disarm PIN modal at app level: usable from both the dashboard and the global alarm overlay.
function DisarmModal({ L, open, onClose, onConfirm }) {
  const [pin, setPin] = useState("");
  useEffect(() => { if (open) setPin(""); }, [open]);
  return (
    <Modal open={open} onClose={onClose} icon="unlock" iconTone="alarm" title={L.pintitle}
      footer={<>
        <Button variant="ghost" onClick={onClose}>{L.cancel}</Button>
        <Button variant="danger" iconLeft="unlock" onClick={() => { onConfirm(pin); onClose(); }}>{L.disarmconfirm}</Button>
      </>}>
      <div className="stack stack-4">
        <p style={{ color: "var(--fg-muted)", fontSize: "var(--text-sm)", margin: 0 }}>{L.pinbody}</p>
        <Field label={L.pinlabel}>
          <input className="input input--mono" type="password" inputMode="numeric" autoComplete="off" data-1p-ignore data-lpignore="true" placeholder="••••" value={pin}
            onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))}
            style={{ letterSpacing: "0.3em", textAlign: "center", fontSize: "var(--text-lg)" }} />
        </Field>
      </div>
    </Modal>
  );
}

function ScreenDashboard({ ctx }) {
  const L = DASH[ctx.lang] || DASH.de;
  const kl = KIND_LABELS[ctx.lang] || KIND_LABELS.de;
  // Configured device: operation home ("Status & Control"), no test mode language.
  const op = !!(ctx.dev && ctx.dev.configured);
  const [state, setState] = useState(null);
  const [smap, setSmap] = useState({});
  const [expanded, setExpanded] = useState(false); // open-sensor list expanded?

  useEffect(() => {
    // No backend client (pure demo build) → Mock. With API present we load for real,
    // even before ctx.live (detect) is confirmed — otherwise demo sensors flash briefly
    // on the optimistic dashboard first render. ctx.live in deps triggers one refetch.
    if (!API) {
      const m = {}; (MOCK ? MOCK.sensors : []).forEach((s) => { m[s.address] = s; });
      setSmap(m);
      setState({ arm_state: "DISARMED", availability: "online", intern_ready: true, extern_ready: true, sensor_states: [] });
      return;
    }
    let stop = false;
    API.getSensors().then((r) => { const m = {}; r.sensors.forEach((s) => { m[s.address] = s; }); if (!stop) setSmap(m); }).catch(() => {});
    return () => { stop = true; };
  }, [ctx.live]);

  // State poll: normally every 2s; for 6s after a command every 300ms —
  // halves perceived latency until arm/disarm and the command result are visible
  // (the remaining wait is protocol: commands only go out in the panel's poll window,
  // and the panel reports the new state only in its next status cycle).
  useEffect(() => {
    if (!API) return; // demo build: state comes from the mock effect above.
    let stop = false;
    const tick = () => API.getState().then((s) => { if (!stop) setState(s); }).catch(() => {});
    tick();
    const fast = Date.now() - (ctx.lastCmdAt || 0) < 6000;
    let id = setInterval(tick, fast ? 300 : 2000);
    let downshift = null;
    if (fast) {
      downshift = setTimeout(() => { clearInterval(id); id = setInterval(tick, 2000); }, 6000);
    }
    return () => { stop = true; clearInterval(id); if (downshift) clearTimeout(downshift); };
  }, [ctx.lastCmdAt, ctx.live]);

  const arm = state ? state.arm_state : "unknown";
  const sstates = state ? state.sensor_states : [];
  const open = sstates.filter((x) => x.active);
  // Controls only when the panel is connected (availability=online) — otherwise commands are
  // discarded anyway ("serial offline"). Readiness comes from the panel bits; if a bit is
  // still unknown on a connected panel, arm-away falls back to "no sensor open", arm-home stays allowed.
  const connected = !!(state && state.availability === "online");
  const externReady = connected && (state.extern_ready != null ? state.extern_ready : open.length === 0);
  const internReady = connected && (state.intern_ready != null ? state.intern_ready : true);
  // No connection → no active segment highlighted, all grey.
  const armUi = connected ? arm : "unknown";
  // Best-effort alarm meta: first open sensor (name + address) — no timestamp in the DTO.
  const meta0 = open.length ? (() => { const s = smap[open[0].address]; return (s ? s.name : hex(open[0].address)) + " · " + hex(open[0].address); })() : "";

  return (
    <div className="page page--mid">
      <PageHead ctx={ctx} eyebrow={op ? L.eyebrow_op : L.eyebrow} title={op ? L.title_op : L.title} sub={op ? L.sub_op : L.sub} />

      {arm === "TRIGGERED" && (
        <AlarmHero L={L} meta={meta0} onDisarm={() => ctx.openDisarm()} onAck={() => ctx.command("reset")} />
      )}

      {/* Open sensors — collapsed: chips; expanded: list. */}
      <div style={{ marginBottom: "var(--space-4)" }}>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: "var(--space-3)", marginBottom: 10 }}>
          <span style={{ fontSize: "var(--text-sm)", fontWeight: "var(--fw-semibold)" }}>{L.open}</span>
          {open.length > 0 && (
            <button className="senscount--btn" onClick={() => setExpanded((v) => !v)}>
              {L.open_expand.replace("{n}", open.length)}
              <Icon name="chevronDown" size={14} style={{ transform: expanded ? "rotate(180deg)" : "none", transition: "transform .18s" }} />
            </button>
          )}
        </div>
        {!connected ? (
          <div className="senschips"><span className="senschip senschip--muted"><span className="senschip__dot" />{L.nodata}</span></div>
        ) : open.length === 0 ? (
          <div className="senschips"><span className="senschip senschip--ok"><span className="senschip__dot" />{L.allclosed}</span></div>
        ) : !expanded ? (
          <div className="senschips">
            {open.slice(0, 3).map((x) => {
              const s = smap[x.address]; const unconf = s && s.status !== "confirmed";
              return (
                <span className={`senschip${unconf ? " senschip--muted" : ""}`} key={x.address}>
                  <span className="senschip__dot" /><span className="senschip__name">{s ? s.name : hex(x.address)}</span>
                </span>
              );
            })}
            {open.length > 3 && <button className="senschip senschip--more" onClick={() => setExpanded(true)}>{L.more.replace("{n}", open.length - 3)}</button>}
          </div>
        ) : (
          <>
            <div className="senslist">
              {open.map((x) => {
                const s = smap[x.address]; const unconf = s && s.status !== "confirmed";
                return (
                  <div className="sensrow" key={x.address}>
                    <span className={`sensrow__dot${unconf ? " is-muted" : ""}`} />
                    <span className="cell-mono">{hex(x.address)}</span>
                    <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{s ? s.name : hex(x.address)}</span>
                    <span className={`sensrow__tag${unconf ? " is-muted" : ""}`}>{unconf ? L.unconf : (s ? (kl[s.kind] || s.kind) : "—")}</span>
                  </div>
                );
              })}
            </div>
            <Button variant="ghost" size="sm" style={{ marginTop: 8 }} onClick={() => setExpanded(false)}>{L.less}</Button>
          </>
        )}
      </div>

      {/* Arm switcher + night/reset grid. Without a connected panel (availability≠online)
          everything is disabled (commands would be discarded); otherwise panel bits gate away/home. */}
      {!connected && <Callout tone="warn" icon="alert">{L.offline}</Callout>}
      <div className="armsw" style={{ marginTop: connected ? undefined : "var(--space-3)" }}>
        <ArmSeg on={armUi === "ARMED_AWAY"} ready={externReady} notReadyLabel={connected ? L.notready : L.notconn}
          tone="armed" icon="shield" title={L.away} desc={L.seg_ext_d} onClick={() => ctx.command("arm_away")} />
        <ArmSeg on={armUi === "ARMED_HOME"} ready={internReady} notReadyLabel={connected ? L.notready : L.notconn}
          tone="warn" icon="home" title={L.home} desc={L.seg_int_d} onClick={() => ctx.command("arm_home")} />
        <ArmSeg on={armUi === "DISARMED"} ready={connected} notReadyLabel={L.notconn}
          tone="disarmed" icon="unlock" title={L.disarm} desc={L.seg_off_d} onClick={() => ctx.openDisarm()} />
      </div>
      <div className="dgrid">
        <Button variant={armUi === "ARMED_NIGHT" ? "primary" : "secondary"} iconLeft="moon" disabled={!connected || armUi === "ARMED_NIGHT"} onClick={() => ctx.command("arm_night")}>{L.night}</Button>
        <Button variant="secondary" iconLeft="refresh" disabled={!connected} onClick={() => ctx.command("reset")}>{L.reset}</Button>
      </div>

      <div className="stack stack-3" style={{ marginTop: "var(--space-4)" }}>
        {state && state.command_result && (
          <div className="cmd-result-row">
            <span className="field__label">{L.lastcmd}</span>
            <Badge tone={state.command_result.text.startsWith("OK") ? "ok" : "alarm"}
              icon={state.command_result.text.startsWith("OK") ? "check" : "alert"}>
              {state.command_result.text}
            </Badge>
            <span className="text-muted">{L.ago.replace("{s}", Math.round(state.command_result.ms_ago / 1000))}</span>
          </div>
        )}
        <Callout tone="warn" icon="alert">{op ? L.warn_op : L.warn}</Callout>
      </div>
    </div>
  );
}
export { DASH, AlarmOverlay, DisarmModal, ScreenDashboard };

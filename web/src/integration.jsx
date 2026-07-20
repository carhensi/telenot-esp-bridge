// Integration S5/S6: MQTT + HomeKit setup, security PIN, HomeKit QR.
import React from 'preact/compat';
const { useState, useEffect, useMemo } = React;
import { API } from './api.js';
import { Badge, Button, Callout, Checkbox, copyText, Field, fmtTime, Icon, Modal, PasswordInput, Toggle } from './ui.jsx';
import { PageHead, WizFooter } from './wizard.jsx';
import qrcode from 'qrcode-generator';

function HomekitQR({ payload }) {
  const html = useMemo(() => {
    try { const qr = qrcode(0, "M"); qr.addData(payload); qr.make(); return qr.createSvgTag({ cellSize: 5, margin: 2, scalable: true }); }
    catch (_) { return ""; }
  }, [payload]);
  return <div className="hk-qr" dangerouslySetInnerHTML={{ __html: html }} />;
}

function ScreenMqtt({ ctx }) {
  const { t, mqtt, setMqtt } = ctx;
  const [test, setTest] = useState({ state: "idle" }); // idle | running | done
  const [haCopied, setHaCopied] = useState(false);
  const set = (k, v) => setMqtt({ ...mqtt, [k]: v });
  const exampleTopic = `${mqtt.topic_root}/sensor/im_essen_eg/state`;
  // Compact Lovelace starter snippet (built-in alarm-panel card, no HACS needed). The full
  // button-card dashboard + entity mapping live in docs/homeassistant.md.
  const HA_SNIPPET = "type: alarm-panel\nname: Alarmanlage\nentity: alarm_control_panel.alarm\nstates:\n  - arm_home\n  - arm_away\n  - arm_night";
  const copyHa = () => { copyText(HA_SNIPPET); setHaCopied(true); setTimeout(() => setHaCopied(false), 1600); };
  // HomeKit runs live (no reboot): once the HAP task is up, /homekit returns the pairing
  // payload. We poll while HomeKit is on and no payload has arrived yet.
  const [hkPair, setHkPair] = useState(null);
  const [starting, setStarting] = useState(false);
  useEffect(() => {
    if (!(ctx.live && API && mqtt.homekit_mode)) { setHkPair(null); return; }
    let stop = false;
    const poll = async () => {
      try { const r = await API.getHomekit(); if (!stop && r && r.payload) { setHkPair(r); setStarting(false); return; } } catch (_) {}
      if (!stop) setTimeout(poll, 1500);
    };
    poll();
    return () => { stop = true; };
  }, [mqtt.homekit_mode, ctx.live]);
  // "Save & start HomeKit": persists the settings; firmware starts HAP live.
  const saveStart = async () => {
    if (!(ctx.live && API)) return;
    setStarting(true);
    try {
      await API.putMqtt(payload());
      ctx.toast("ok", t("hk.saved"));
    } catch (e) {
      ctx.toast("err", (e && e.message) || t("hk.startfail"));
    } finally {
      setStarting(false);
    }
  };
  // Switch HomeKit → MQTT: HAP cannot be stopped live → persist + reboot.
  const saveMqttReboot = async () => {
    if (!(ctx.live && API)) return;
    setStarting(true);
    try { await API.putMqtt(payload()); await API.reboot(); } catch (_) { setStarting(false); }
  };

  // Live: load current MQTT settings from the device (password is write-only → _pw left empty).
  useEffect(() => {
    if (ctx.live && API) {
      API.getMqtt().then((m) => setMqtt({ ...m, _pw: "" })).catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- hydrate MQTT settings from the device once on mount
  }, []);

  const payload = () => ({
    host: mqtt.host, port: Number(mqtt.port) || 0, tls: !!mqtt.tls,
    username: mqtt.username || "", topic_root: mqtt.topic_root || "", ha_discovery: !!mqtt.ha_discovery,
    homekit_mode: !!mqtt.homekit_mode, homekit_disarm: !!mqtt.homekit_disarm,
    password: mqtt._pw ? mqtt._pw : undefined,
  });

  const runTest = () => {
    if (ctx.live && API) {
      setTest({ state: "running" });
      API.putMqtt(payload()).then(() => API.testMqtt()).then(() => {
        let n = 0;
        const id = setInterval(async () => {
          n++;
          try {
            const r = await API.getMqttTest();
            if (r.state === "done") {
              clearInterval(id);
              const lbl = r.ok ? t("s5.test.ok") : t("s5.test." + (r.detail || "connect"));
              setTest({ state: "done", ok: r.ok, label: lbl, time: fmtTime(), cert: r.cert || null });
            }
          } catch (_) { clearInterval(id); }
          if (n > 20) clearInterval(id);
        }, 600);
      }).catch(() => setTest({ state: "done", ok: false, label: t("s5.test.connect"), time: fmtTime() }));
      return;
    }
    setTest({ state: "running" });
    setTimeout(() => setTest({ state: "done", ok: true, label: t("s5.test.ok"), time: fmtTime() }), 1500);
  };

  // TOFU: trust the self-signed cert presented during the test, then re-test (→ green).
  const pinCert = async () => {
    if (!(ctx.live && API)) return;
    try {
      await API.pinCert();
      const m = await API.getMqtt();
      setMqtt({ ...m, _pw: mqtt._pw || "" });
      ctx.toast("ok", t("s5.pinned"));
    } catch (_) {}
    runTest();
  };
  const unpinCert = async () => {
    if (!(ctx.live && API)) return;
    try {
      await API.unpinCert();
      const m = await API.getMqtt();
      setMqtt({ ...m, _pw: mqtt._pw || "" });
    } catch (_) {}
    setTest({ state: "idle" });
  };

  const saveAndNext = async () => {
    if (ctx.live && API) { try { await API.putMqtt(payload()); } catch (_) {} }
    ctx.next();
  };

  return (
    <div className="page page--mid">
      <PageHead ctx={ctx} eyebrow={t("integ.eyebrow")}
        title={mqtt.homekit_mode ? t("integ.hk.title") : t("integ.mqtt.title")}
        sub={mqtt.homekit_mode ? t("integ.hk.sub") : t("integ.mqtt.sub")} />
      <Callout tone="warn" icon="alert" style={{ marginBottom: "var(--space-4)" }}>{mqtt.homekit_mode ? t("integ.excl.hk") : t("integ.excl.mqtt")}</Callout>
      {mqtt.homekit_mode && (<>
        <div className="card" style={{ marginBottom: "var(--space-4)" }}><div className="card__body stack stack-3">
          <label className="toggle" style={{ justifyContent: "space-between", width: "100%" }}>
            <span><strong style={{ fontWeight: 600 }}>{t("hk.disarm")}</strong><br /><span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("hk.disarm.note")}</span></span>
            <Toggle checked={!!mqtt.homekit_disarm} onChange={(v) => set("homekit_disarm", v)} />
          </label>
          {hkPair ? (
            <div className="stack stack-3" style={{ alignItems: "center", textAlign: "center" }}>
              <HomekitQR payload={hkPair.payload} />
              <div><strong>{t("hk.scan")}</strong><br /><span className="mono" style={{ fontSize: "clamp(1rem, 4.5vw, var(--text-lg))" }}>{hkPair.code}</span></div>
              <span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("hk.scan.note")}</span>
            </div>
          ) : (
            <Callout tone="accent" icon="shield">{t("hk.mode.on")}</Callout>
          )}
          {ctx.live && <Button variant="primary" iconLeft="shield" onClick={saveStart} disabled={starting}>{starting ? t("hk.starting") : (hkPair ? t("hk.apply") : t("hk.saveStart"))}</Button>}
        </div></div>
        <div className="cluster" style={{ justifyContent: "center", marginBottom: "var(--space-4)" }}>
          <span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("integ.toMqtt.hint")}</span>
          <Button variant="ghost" size="sm" onClick={() => set("homekit_mode", false)}>{t("integ.toMqtt")}</Button>
        </div>
      </>)}
      {!mqtt.homekit_mode && (
      <div className="card"><div className="card__body stack stack-5">
        <Callout tone="info" icon="spark">{t("integ.toHk.hint")}
          <div style={{ marginTop: "var(--space-2)" }}><Button variant="ghost" size="sm" onClick={() => set("homekit_mode", true)}>{t("integ.toHk")}</Button></div>
        </Callout>
        <div className="field__row">
          <Field label={t("s5.host")}><input className="input input--mono" value={mqtt.host} onChange={(e) => set("host", e.target.value)} /></Field>
          <div className="field--port"><Field label={t("s5.port")}><input className="input input--mono" value={mqtt.port} onChange={(e) => set("port", e.target.value)} /></Field></div>
        </div>
        <label className="toggle" style={{ justifyContent: "space-between" }}>
          <span><strong style={{ fontWeight: 600 }}>{t("s5.tls")}</strong></span>
          <Toggle checked={mqtt.tls} onChange={(v) => {
            // Standard port follows the TLS mode; a custom port is left untouched.
            const port = v && String(mqtt.port) === "1883" ? "8883" : !v && String(mqtt.port) === "8883" ? "1883" : mqtt.port;
            setMqtt({ ...mqtt, tls: v, port });
          }} />
        </label>
        <div className="field__row">
          <Field label={t("s5.user")}><input className="input" value={mqtt.username} onChange={(e) => set("username", e.target.value)} /></Field>
          <Field label={t("s5.pw")} hint={mqtt.password_set ? t("s5.pwkeep") : null}><PasswordInput value={mqtt._pw || ""} onChange={(v) => set("_pw", v)} placeholder={mqtt.password_set ? "••••••••" : ""} /></Field>
        </div>
        <Field label={t("s5.topicroot")}><input className="input input--mono" value={mqtt.topic_root} onChange={(e) => set("topic_root", e.target.value)} /></Field>
        <div className="callout callout--info" style={{ flexDirection: "column", gap: 4 }}>
          <span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)", textTransform: "uppercase", letterSpacing: "0.05em", fontWeight: 600 }}>{t("s5.preview")}</span>
          <code className="mono" style={{ fontSize: "var(--text-sm)", wordBreak: "break-all" }}>{exampleTopic}</code>
        </div>
        <label className="toggle" style={{ justifyContent: "space-between" }}>
          <span><strong style={{ fontWeight: 600 }}>{t("s5.hadisc")}</strong><br /><span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("s5.hadiscnote")}</span></span>
          <Toggle checked={mqtt.ha_discovery} onChange={(v) => set("ha_discovery", v)} />
        </label>
        <Callout tone="accent" icon="shield" title={t("ha.title")}>
          {mqtt.ha_discovery ? t("ha.body.on") : t("ha.body.off")}
          <div style={{ marginTop: "var(--space-3)", display: "flex", gap: "var(--space-2)", flexWrap: "wrap", alignItems: "center" }}>
            <Button variant="secondary" size="sm" iconLeft={haCopied ? "check" : "copy"} onClick={copyHa}>{haCopied ? t("ha.copied") : t("ha.copy")}</Button>
            <span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("ha.doc")}</span>
          </div>
        </Callout>
        <div className="cluster" style={{ gap: "var(--space-3)" }}>
          <Button variant="secondary" iconLeft="plug" onClick={runTest} disabled={test.state === "running"}>{test.state === "running" ? t("s5.testing") : t("s5.test")}</Button>
          {test.state === "done" && (test.ok || !test.cert) && <Badge tone={test.ok ? "ok" : "alarm"} icon={test.ok ? "check" : "alert"}>{test.label}</Badge>}
          {test.state === "done" && <span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("s5.testat", { time: test.time })}</span>}
        </div>
        {!mqtt.tls && <Callout tone="warn" icon="alert">{t("s5.plainwarn")}</Callout>}
        {mqtt.tls && mqtt.pinned && !test.cert && (
          <div className="cluster" style={{ gap: "var(--space-2)" }}>
            <Badge tone="ok" icon="shield">{t("s5.pinned")}</Badge>
            <Button variant="ghost" size="sm" onClick={unpinCert}>{t("s5.unpin")}</Button>
          </div>
        )}
        {test.state === "done" && test.ok && test.cert && (
          <div className="cluster" style={{ gap: "var(--space-2)", alignItems: "center" }}>
            <span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("s5.cert.seen")}</span>
            <span className="mono" style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)", wordBreak: "break-all" }}>{test.cert.sha256}</span>
            {!mqtt.pinned && <Button variant="ghost" size="sm" iconLeft="shield" onClick={pinCert}>{t("s5.pin.trust")}</Button>}
          </div>
        )}
        {test.state === "done" && !test.ok && test.cert && (
          <div className="card"><div className="card__body stack stack-3">
            <div className="cluster"><Badge tone="warn" icon="shield">{mqtt.pinned ? t("s5.pin.changed") : t("s5.pin.title")}</Badge></div>
            <p style={{ color: "var(--fg-muted)", margin: 0, fontSize: "var(--text-sm)" }}>{mqtt.pinned ? t("s5.pin.changedbody") : t("s5.pin.body")}</p>
            <dl className="deflist" style={{ margin: 0 }}>
              <dt>{t("s5.pin.subject")}</dt><dd className="mono" style={{ wordBreak: "break-all" }}>{test.cert.subject || "—"}</dd>
              <dt>{t("s5.pin.issuer")}</dt><dd className="mono" style={{ wordBreak: "break-all" }}>{test.cert.issuer || "—"}</dd>
              <dt>{t("s5.pin.fingerprint")}</dt><dd className="mono" style={{ wordBreak: "break-all", fontSize: "var(--text-xs)" }}>{test.cert.sha256}</dd>
            </dl>
            <Button variant="primary" iconLeft="shield" onClick={pinCert}>{t("s5.pin.trust")}</Button>
          </div></div>
        )}
        {ctx.live && (ctx.scan.phase === "belegt" || ctx.scan.phase === "naming") && (
          <Callout tone="warn" icon="alert">{t("integ.mqtt.scanblock")}</Callout>
        )}
        {ctx.live && <Button variant="primary" iconLeft="power" onClick={saveMqttReboot}
          disabled={starting || ctx.scan.phase === "belegt" || ctx.scan.phase === "naming"}>
          {starting ? t("integ.mqtt.rebooting") : t("integ.mqtt.saveReboot")}</Button>}
      </div></div>
      )}
      <WizFooter ctx={ctx} nextLabel={t("btn.next")} onNext={saveAndNext} />
    </div>
  );
}

/* ===================== S6 — SECURITY ===================== */
function ScreenSecurity({ ctx }) {
  const { t, sec, setSec } = ctx;
  const [pin, setPin] = useState(""); const [pin2, setPin2] = useState("");
  const [warn, setWarn] = useState(false);
  const [ack, setAck] = useState(false);
  const weak = pin.length >= 4 && /^(0000|1234|1111|0123)/.test(pin);
  const pinOk = pin.length >= 4 && pin === pin2;
  const pinSet = sec.pinSet;

  const savePin = () => {
    if (ctx.live && API) {
      API.putPin(pin)
        .then(() => { setSec({ ...sec, pinSet: true, pin }); ctx.toast("ok", t("s6.pin")); })
        .catch((e) => ctx.toast("err", (e && e.message) || "PIN"));
      return;
    }
    setSec({ ...sec, pinSet: true, pin }); ctx.toast("ok", t("s6.pin"));
  };

  return (
    <div className="page page--mid">
      <PageHead ctx={ctx} eyebrow={t("s6.eyebrow")} title={t("s6.title")} sub={t("s6.sub")} />

      <div className="card"><div className="card__body stack stack-5">
        <Callout tone="accent" icon="shield">{t("s6.threat")}</Callout>
        <div className="field__row">
          <Field label={t("s6.pin")} hint={t("s6.pinnote")} error={weak ? null : null}>
            <input className="input input--mono" inputMode="numeric" autoComplete="off" data-1p-ignore data-lpignore="true" value={pin} onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))} placeholder={pinSet ? "••••" : ""} />
          </Field>
          <Field label={t("s6.pin2")} error={pin2.length > 0 && pin !== pin2 ? t("s1.pwnomatch") : null}>
            <input className="input input--mono" inputMode="numeric" autoComplete="off" data-1p-ignore data-lpignore="true" value={pin2} onChange={(e) => setPin2(e.target.value.replace(/\D/g, ""))} />
          </Field>
        </div>
        {weak && <Callout tone="warn" icon="alert">{t("s6.pinweak")}</Callout>}
        <div className="cluster">
          <Button variant={pinSet ? "secondary" : "primary"} iconLeft={pinSet ? "check" : null} disabled={!pinOk} onClick={savePin}>{pinSet ? t("st.ok") : t("btn.save")}</Button>
          {pinSet && <Badge tone="ok" icon="check">PIN {t("st.ok")}</Badge>}
        </div>
      </div></div>

      <div className="card" style={{ borderColor: sec.remoteDisarm ? "var(--state-alarm-border)" : undefined }}>
        <div className="card__body">
          <div className="row-between">
            <div style={{ maxWidth: "62%" }}>
              <div style={{ fontWeight: 600, display: "flex", alignItems: "center", gap: 8 }}>
                {sec.remoteDisarm && <Icon name="alert" size={16} style={{ color: "var(--state-alarm-fg)" }} />}{t("s6.remote")}
              </div>
              <p style={{ color: "var(--fg-subtle)", fontSize: "var(--text-sm)", margin: "4px 0 0" }}>{t("s6.remotenote")}</p>
            </div>
            <Toggle checked={sec.remoteDisarm} danger onChange={(v) => {
              if (v) { if (!pinSet) { ctx.toast("err", t("s6.needpin")); return; } setAck(false); setWarn(true); }
              else { if (ctx.live && API) API.putRemoteDisarm(false, false).catch(() => {}); setSec({ ...sec, remoteDisarm: false }); }
            }} />
          </div>
          {sec.remoteDisarm && <div style={{ marginTop: "var(--space-4)" }}><Callout tone="alarm" icon="alert"><strong>{t("s6.remoteon")}</strong> — {t("s8.w.remote", {})}</Callout></div>}
        </div>
      </div>

      <WizFooter ctx={ctx} nextLabel={t("btn.next")} />

      <Modal open={warn} onClose={() => setWarn(false)} title={t("s6.warntitle")} icon="alert" iconTone="alarm"
        footer={<><Button variant="ghost" onClick={() => setWarn(false)}>{t("btn.cancel")}</Button>
          <Button variant="danger" disabled={!ack} iconLeft="shield" onClick={() => { if (ctx.live && API) API.putRemoteDisarm(true, true).catch(() => {}); setSec({ ...sec, remoteDisarm: true }); setWarn(false); }}>{t("s6.activate")}</Button></>}>
        <div className="stack stack-4">
          <Callout tone="alarm" icon="alert">{t("s6.warnbody")}</Callout>
          <label className="toggle" style={{ alignItems: "flex-start", gap: "var(--space-3)" }}>
            <Checkbox checked={ack} onChange={() => setAck(!ack)} />
            <span style={{ fontSize: "var(--text-sm)" }}>{t("s6.ack")}</span>
          </label>
        </div>
      </Modal>
    </div>
  );
}

/* ===================== S7 — DIAGNOSTICS ===================== */
export { ScreenMqtt, ScreenSecurity, HomekitQR };

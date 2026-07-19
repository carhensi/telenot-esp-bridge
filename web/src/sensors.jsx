// Sensor table S4 (core screen): bulk edit, room grouping, inline edit, drawer, expert editor.
import React from 'preact/compat';
const { useState, useEffect, useRef, useCallback, useMemo } = React;
import { API } from './api.js';
import { Icon, Button, Badge, StatusDot, Checkbox, Toggle, Field, Modal, Callout, STAT_TONE, hex } from './ui.jsx';


const KIND_LABELS = {
  de: { bewegungsmelder: "Bewegungsmelder", magnetkontakt: "Magnetkontakt", schliesskontakt: "Schließkontakt",
    rauchmelder: "Rauchmelder", wassermelder: "Wassermelder", ueberfallmelder: "Überfallmelder",
    gehaeuse: "Gehäuse", signalgeber: "Signalgeber", sabotage: "Sabotage", systemstatus: "Systemstatus",
    sicherung: "Sicherung", batterie: "Batterie/Akku", netzstoerung: "Netzstörung", uebertragung: "Übertragung (ÜE)",
    stoerung: "Störung", technik: "Technik",
    bedienteil: "Bedienteil", gebaeudetechnik: "Gebäudetechnik", unbekannt: "Unbekannt" },
  en: { bewegungsmelder: "Motion", magnetkontakt: "Magnet contact", schliesskontakt: "Lock contact",
    rauchmelder: "Smoke", wassermelder: "Water", ueberfallmelder: "Panic", gehaeuse: "Enclosure",
    signalgeber: "Siren", sabotage: "Tamper", systemstatus: "System", sicherung: "Fuse",
    batterie: "Battery", netzstoerung: "Mains power", uebertragung: "Transmission (ARC)",
    stoerung: "Fault", technik: "Technical",
    bedienteil: "Keypad", gebaeudetechnik: "Building tech", unbekannt: "Unknown" },
};
const KINDS = ["bewegungsmelder","magnetkontakt","schliesskontakt","rauchmelder","wassermelder","ueberfallmelder","gehaeuse","signalgeber","sabotage","systemstatus","sicherung","batterie","netzstoerung","uebertragung","stoerung","technik","bedienteil","gebaeudetechnik","unbekannt"];

// Room assignment currently OFF (rooms are managed in Home Assistant). Code + design fully
// preserved — set to `true` to re-enable room grouping, bulk "assign room", and the room
// field in the sensor drawer.
const ROOMS_ENABLED = false;
const POLS = ["active_low","active_high","unconfirmed"];
const ROOM_ORDER = ["Erdgeschoss","Obergeschoss","Dachgeschoss","Keller","Garage","Außen",""];

function StatusBadge({ status, t }) {
  return <Badge tone={STAT_TONE[status]} icon={status === "confirmed" ? "check" : status === "excluded" ? "x" : "alert"}>{t("stat." + status)}</Badge>;
}

/* ============================================================
   Expert editor (S4): all visible sensors as editable text
   (table/JSON), live validation, short codes, bulk apply by address.
   ============================================================ */
const EXP_KINDS = ["magnetkontakt","schliesskontakt","bewegungsmelder","rauchmelder","wassermelder","ueberfallmelder","gehaeuse","signalgeber","sabotage","systemstatus","sicherung","batterie","netzstoerung","uebertragung","stoerung","technik","bedienteil","gebaeudetechnik","unbekannt"];
const KIND2CODE = { magnetkontakt:"mk", schliesskontakt:"sk", bewegungsmelder:"bm", rauchmelder:"rm", wassermelder:"wm", ueberfallmelder:"uf", gehaeuse:"gk", signalgeber:"sg", sabotage:"sab", systemstatus:"sys", sicherung:"si", batterie:"bat", netzstoerung:"netz", uebertragung:"ue", stoerung:"st", technik:"tec", bedienteil:"bt", gebaeudetechnik:"cl", unbekannt:"?" };
const CODE2KIND = (() => { const m = {}; Object.entries(KIND2CODE).forEach(([k,c]) => m[c]=k); EXP_KINDS.forEach(k => m[k]=k); return m; })();
const POL2CODE = { active_low:"lo", active_high:"hi", unconfirmed:"?" };
const CODE2POL = { lo:"active_low", hi:"active_high", "?":"unconfirmed", active_low:"active_low", active_high:"active_high", unconfirmed:"unconfirmed" };

function expHex(a) { return "0x" + a.toString(16).toUpperCase().padStart(4, "0"); }
function expParseHex(s) {
  const m = String(s).trim().match(/^0x([0-9a-f]+)$/i) || String(s).trim().match(/^([0-9a-f]+)$/i);
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return Number.isFinite(n) ? n : null;
}

function buildTSV(list) {
  const roomW = Math.max(4, ...list.map((s) => (s.location || "").length));
  const head = `# ${"addr".padEnd(6)} | ${"kind".padEnd(4)} | ${"pol".padEnd(3)} | inc | ${"room".padEnd(roomW)} | name`;
  const rows = list.map((s) =>
    `${expHex(s.address).padEnd(6)} | ${(KIND2CODE[s.kind]||"?").padEnd(4)} | ${(POL2CODE[s.polarity]||"?").padEnd(3)} | ${(s.include?"y":"n").padEnd(3)} | ${(s.location||"").padEnd(roomW)} | ${s.name||""}`
  );
  return head + "\n" + rows.join("\n");
}
function buildJSON(list) {
  return JSON.stringify(list.map((s) => ({
    address: expHex(s.address), name: s.name, name_ha: s.name_ha,
    kind: s.kind, polarity: s.polarity, location: s.location, topic: s.topic, include: s.include,
  })), null, 2);
}

function parseEditor(text, fmt, byAddr, t, allByAddr) {
  const errors = [];
  // Distinguish "unknown" from "merely hidden" — hidden addresses are reachable via the filter.
  const addrError = (ln, addr) =>
    t(allByAddr && allByAddr.has(addr) ? "exp.err.hiddenaddr" : "exp.err.addr", { ln, a: expHex(addr) });
  const changes = [];
  const seen = new Set();
  const diffsFrom = (cur, next) => {
    const out = {};
    ["name","name_ha","kind","polarity","location","topic","include"].forEach((k) => {
      if (next[k] !== undefined && next[k] !== cur[k]) out[k] = next[k];
    });
    return out;
  };

  if (fmt === "json") {
    let arr;
    try { arr = JSON.parse(text); } catch (e) { return { changes: [], errors: [t("exp.err.json", { m: e.message })], addrs: [] }; }
    if (!Array.isArray(arr)) return { changes: [], errors: [t("exp.err.json", { m: "expected array" })], addrs: [] };
    arr.forEach((o, i) => {
      const ln = i + 1;
      const addr = expParseHex(o.address);
      if (addr == null) { errors.push(t("exp.err.hex", { ln, a: String(o.address) })); return; }
      const cur = byAddr.get(addr);
      if (!cur) { errors.push(addrError(ln, addr)); return; }
      if (seen.has(addr)) { errors.push(t("exp.err.dupaddr", { ln, a: expHex(addr) })); return; }
      seen.add(addr);
      const next = {};
      if (o.name !== undefined) next.name = String(o.name);
      if (o.name_ha !== undefined) next.name_ha = String(o.name_ha);
      if (o.topic !== undefined) next.topic = String(o.topic);
      if (o.location !== undefined) next.location = String(o.location);
      if (o.kind !== undefined) {
        const k = CODE2KIND[String(o.kind).toLowerCase()];
        if (!k) { errors.push(t("exp.err.kind", { ln, v: o.kind })); return; }
        next.kind = k;
      }
      if (o.polarity !== undefined) {
        const p = CODE2POL[String(o.polarity).toLowerCase()];
        if (!p) { errors.push(t("exp.err.pol", { ln, v: o.polarity })); return; }
        next.polarity = p;
      }
      if (o.include !== undefined) next.include = !!o.include;
      const d = diffsFrom(cur, next);
      if (Object.keys(d).length) changes.push({ address: addr, ...d });
    });
    return { changes, errors, addrs: [...seen] };
  }

  // TSV
  text.split("\n").forEach((raw, i) => {
    const ln = i + 1;
    const line = raw.trim();
    if (!line || line.startsWith("#")) return;
    const cells = line.split("|");
    if (cells.length < 6) { errors.push(t("exp.err.cols", { ln })); return; }
    const addr = expParseHex(cells[0]);
    if (addr == null) { errors.push(t("exp.err.hex", { ln, a: cells[0].trim() })); return; }
    const cur = byAddr.get(addr);
    if (!cur) { errors.push(addrError(ln, addr)); return; }
    if (seen.has(addr)) { errors.push(t("exp.err.dupaddr", { ln, a: expHex(addr) })); return; }
    seen.add(addr);
    const kc = cells[1].trim().toLowerCase();
    const pc = cells[2].trim().toLowerCase();
    const ic = cells[3].trim().toLowerCase();
    const room = cells[4].trim();
    const name = cells.slice(5).join("|").trim();
    const kind = CODE2KIND[kc];
    if (!kind) { errors.push(t("exp.err.kind", { ln, v: cells[1].trim() })); return; }
    const pol = CODE2POL[pc];
    if (!pol) { errors.push(t("exp.err.pol", { ln, v: cells[2].trim() })); return; }
    if (ic !== "y" && ic !== "n") { errors.push(t("exp.err.inc", { ln })); return; }
    const next = { name, kind, polarity: pol, location: room, include: ic === "y" };
    const d = diffsFrom(cur, next);
    if (Object.keys(d).length) changes.push({ address: addr, ...d });
  });
  return { changes, errors, addrs: [...seen] };
}

function ExpertEditor({ open, onClose, list, all, t, onApply, toast }) {
  const [fmt, setFmt] = React.useState("tsv");
  const [text, setText] = React.useState("");
  const [copied, setCopied] = React.useState(false);
  const taRef = React.useRef(null);

  const byAddr = React.useMemo(() => { const m = new Map(); list.forEach((s) => m.set(s.address, s)); return m; }, [list]);
  const allByAddr = React.useMemo(() => { const m = new Map(); (all || list).forEach((s) => m.set(s.address, s)); return m; }, [all, list]);

  const regen = React.useCallback((f = fmt) => { setText(f === "json" ? buildJSON(list) : buildTSV(list)); }, [fmt, list]);
  // eslint-disable-next-line react-hooks/exhaustive-deps -- (re)generate editor text only when opening; regen excluded so list/fmt changes don't clobber edits
  React.useEffect(() => { if (open) regen(); }, [open]);

  const switchFmt = (f) => { setFmt(f); setText(f === "json" ? buildJSON(list) : buildTSV(list)); };

  const [confirmOnApply, setConfirmOnApply] = React.useState(false);
  const { changes, errors, addrs } = React.useMemo(() => parseEditor(text, fmt, byAddr, t, allByAddr), [text, fmt, byAddr, t, allByAddr]);
  const lineCount = React.useMemo(() => {
    if (fmt === "json") { try { const a = JSON.parse(text); return Array.isArray(a) ? a.length : 0; } catch { return 0; } }
    return text.split("\n").filter((l) => l.trim() && !l.trim().startsWith("#")).length;
  }, [text, fmt]);

  const doCopy = () => { navigator.clipboard && navigator.clipboard.writeText(text); setCopied(true); setTimeout(() => setCopied(false), 1400); };
  const doApply = () => {
    if (errors.length || (!changes.length && !confirmOnApply)) return;
    onApply(changes, confirmOnApply ? addrs : null);
    toast && toast("ok", t("exp.applied", { n: changes.length }));
    onClose();
  };

  const onKey = (e) => {
    if (e.key === "Tab") {
      e.preventDefault();
      const el = e.target, s = el.selectionStart, en = el.selectionEnd;
      const nv = text.slice(0, s) + "  " + text.slice(en);
      setText(nv);
      requestAnimationFrame(() => { el.selectionStart = el.selectionEnd = s + 2; });
    }
    if (e.key === "Escape") onClose();
  };

  if (!open) return null;
  const ok = errors.length === 0;

  return (
    <div className="exp-scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="exp" role="dialog" aria-label={t("exp.title")}>
        <header className="exp__head">
          <span className="exp__icon"><Icon name="code" size={18} /></span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="exp__title">{t("exp.title")}</div>
            <div className="exp__sub">{t("exp.sub", { n: list.length })}</div>
          </div>
          <div className="segmented" role="group" aria-label="Format">
            <button aria-pressed={fmt === "tsv"} onClick={() => switchFmt("tsv")}>{t("exp.fmt.tsv")}</button>
            <button aria-pressed={fmt === "json"} onClick={() => switchFmt("json")}>{t("exp.fmt.json")}</button>
          </div>
          <button className="icon-btn" onClick={onClose} aria-label={t("btn.close")}><Icon name="x" size={18} /></button>
        </header>

        <div className="exp__body">
          <div className="exp__editor">
            <textarea ref={taRef} className="exp__ta" spellCheck={false} wrap="off" value={text}
              onChange={(e) => setText(e.target.value)} onKeyDown={onKey} aria-label={t("exp.title")} />
          </div>

          <aside className="exp__side scroll">
            <div className={`exp__status exp__status--${ok ? "ok" : "err"}`}>
              <div className="exp__statline">
                <Icon name={ok ? "check" : "alert"} size={15} />
                <strong>{ok ? t("exp.noerr") : t("exp.errors", { n: errors.length })}</strong>
              </div>
              <div className="exp__metrics">
                <span>{t("exp.parsed", { n: lineCount })}</span>
                <span className="exp__dot">·</span>
                <span style={{ color: changes.length ? "var(--accent)" : "var(--fg-subtle)", fontWeight: 600 }}>{t("exp.changed", { n: changes.length })}</span>
              </div>
            </div>

            {errors.length > 0 && (
              <div className="exp__errors">
                {errors.slice(0, 40).map((e, i) => <div key={i} className="exp__errrow">{e}</div>)}
                {errors.length > 40 && <div className="exp__errrow" style={{ color: "var(--fg-subtle)" }}>… +{errors.length - 40}</div>}
              </div>
            )}

            {fmt === "tsv" && (
              <div className="exp__legend">
                <div className="exp__legtitle">{t("exp.legend.kind")}</div>
                <div className="exp__codes">
                  {EXP_KINDS.map((k) => <span key={k} className="exp__code"><b>{KIND2CODE[k]}</b> {k}</span>)}
                </div>
                <div className="exp__legtitle">{t("exp.legend.pol")}</div>
                <div className="exp__codes">
                  <span className="exp__code"><b>lo</b> active_low</span>
                  <span className="exp__code"><b>hi</b> active_high</span>
                  <span className="exp__code"><b>?</b> unconfirmed</span>
                </div>
                <div className="exp__legtitle">inc</div>
                <div className="exp__codes"><span className="exp__code">{t("exp.legend.inc")}</span></div>
              </div>
            )}

            <p className="exp__hint">{t("exp.hint")}</p>
            <p className="exp__hint exp__hint--muted">{t("exp.scope")}</p>
          </aside>
        </div>

        <footer className="exp__foot">
          <Button variant="ghost" size="sm" iconLeft="refresh" onClick={() => regen()}>{t("exp.reset")}</Button>
          <Button variant="ghost" size="sm" iconLeft={copied ? "check" : "copy"} onClick={doCopy}>{copied ? t("exp.copied") : t("exp.copy")}</Button>
          <span style={{ flex: 1 }} />
          <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: "var(--text-sm)", cursor: "pointer" }}>
            <input type="checkbox" checked={confirmOnApply} onChange={(e) => setConfirmOnApply(e.target.checked)} />
            {t("exp.confirm")}
          </label>
          <Button variant="ghost" onClick={onClose}>{t("btn.cancel")}</Button>
          <Button variant="primary" iconLeft="check" disabled={!ok || (!changes.length && !confirmOnApply)} onClick={doApply}>
            {changes.length ? t("exp.applyn", { n: changes.length }) : t("exp.nochange")}
          </Button>
        </footer>
      </div>
    </div>
  );
}

// Bulk action as a clean dropdown menu (replaces the <select> that oddly snapped back
// to the placeholder after selection). Closes on pick or outside click.
function BulkMenu({ label, options, onPick }) {
  const [open, setOpen] = useState(false);
  const ref = useRef(null);
  useEffect(() => {
    if (!open) return;
    const onDoc = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);
  return (
    <div className="menu-wrap" ref={ref}>
      <Button variant="secondary" size="sm" iconRight="chevronDown" aria-haspopup="true" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {label}
      </Button>
      {open && (
        <div className="menu" role="menu">
          {options.map((o) => (
            <button key={o.value} className="menu__item" role="menuitem" onClick={() => { onPick(o.value); setOpen(false); }}>{o.label}</button>
          ))}
        </div>
      )}
    </div>
  );
}

function SensorTable({ ctx, sensors, setSensors, t, lang, rooms, toast }) {
  const kl = KIND_LABELS[lang] || KIND_LABELS.de;
  const [sel, setSel] = useState(() => new Set());
  const [search, setSearch] = useState("");
  const [filters, setFilters] = useState({ unconfirmed: false, dups: false, polarity: false, excluded: false });
  const [collapsed, setCollapsed] = useState(() => new Set());
  const [drawer, setDrawer] = useState(null); // sensor address or null
  const [expOpen, setExpOpen] = useState(false);

  const patch = useCallback((addrs, fn) => {
    // addrs may be a Set, an array (e.g. "confirm all" → visibleAddrs), or a single address.
    // Formerly an array was mistakenly wrapped as new Set([array]) → one broken composite address.
    const set = addrs instanceof Set ? addrs : new Set(Array.isArray(addrs) ? addrs : [addrs]);
    setSensors((prev) => prev.map((s) => set.has(s.address) ? { ...s, ...fn(s) } : s));
    set.forEach((addr) => {
      const s = sensors.find((x) => x.address === addr) || { address: addr };
      const fields = fn(s);
      // Stage for HomeKit: on show_in_homekit toggle OR rename (name/name_ha) of a sensor already
      // in HomeKit — only in HomeKit mode. This also triggers the "Apply" bar on rename.
      const hkTouched = "show_in_homekit" in fields
        || ((("name" in fields) || ("name_ha" in fields)) && s.show_in_homekit);
      if (hkTouched && ctx && ctx.mqtt && ctx.mqtt.homekit_mode) ctx.hkMark(addr);
      // Live: send the same change to the backend (status is UI-derived → omit). Do NOT
      // silently swallow errors — otherwise a rejected save looks like "doesn't work".
      if (API && API.live) {
        const { status, ...rest } = fields;
        API.patchSensor(addr, rest).catch(() => toast("err", t("s4.savefail")));
      }
    });
  }, [setSensors, sensors, ctx, toast, t]);

  const confirmRows = (addrs) => patch(addrs, () => ({ status: "confirmed", confirmed: true }));
  const excludeRows = (addrs) => patch(addrs, () => ({ status: "excluded", include: false }));
  const includeRows = (addrs) => patch(addrs, (s) => ({ status: s.confirmed ? "confirmed" : "unconfirmed", include: true }));

  const toggleFilter = (k) => setFilters((f) => ({ ...f, [k]: !f[k] }));

  // filtered list
  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    return sensors.filter((s) => {
      if (!filters.excluded && s.status === "excluded") return false;
      if (filters.unconfirmed && s.status !== "unconfirmed") return false;
      if (filters.dups && s.dupOf == null) return false;
      if (filters.polarity && s.polarity !== "unconfirmed") return false;
      if (q) {
        const hay = (s.name + " " + hex(s.address) + " " + s.raw_name + " " + s.topic).toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [sensors, search, filters]);

  // group by room
  const groups = useMemo(() => {
    const map = new Map();
    visible.forEach((s) => { const k = s.location || ""; if (!map.has(k)) map.set(k, []); map.get(k).push(s); });
    const keys = [...map.keys()].sort((a, b) => {
      const ia = ROOM_ORDER.indexOf(a), ib = ROOM_ORDER.indexOf(b);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b);
    });
    return keys.map((k) => ({ room: k, items: map.get(k) }));
  }, [visible]);

  const counts = useMemo(() => {
    let conf = 0, unconf = 0, excl = 0;
    sensors.forEach((s) => { if (s.status === "confirmed") conf++; else if (s.status === "excluded") excl++; else unconf++; });
    return { conf, unconf, excl };
  }, [sensors]);

  const filterCounts = useMemo(() => ({
    unconfirmed: sensors.filter((s) => s.status === "unconfirmed").length,
    dups: sensors.filter((s) => s.dupOf != null && s.status !== "excluded").length,
    polarity: sensors.filter((s) => s.polarity === "unconfirmed" && s.status !== "excluded").length,
  }), [sensors]);

  const visibleAddrs = useMemo(() => visible.map((s) => s.address), [visible]);
  const allVisSel = visibleAddrs.length > 0 && visibleAddrs.every((a) => sel.has(a));
  const someVisSel = visibleAddrs.some((a) => sel.has(a));

  const toggleSel = (a) => setSel((p) => { const n = new Set(p); n.has(a) ? n.delete(a) : n.add(a); return n; });
  const selectAllVisible = () => setSel(allVisSel ? new Set() : new Set(visibleAddrs));
  const clearSel = () => setSel(new Set());
  const toggleCollapse = (room) => setCollapsed((p) => { const n = new Set(p); n.has(room) ? n.delete(room) : n.add(room); return n; });

  const bulkApply = (fn) => { patch(sel, fn); };
  const drawerSensor = drawer != null ? sensors.find((s) => s.address === drawer) : null;

  // Expert editor: apply changes (keyed by address) — locally + live to the backend (status
  // is UI-derived from include/confirmed → do not send). `confirmAddrs` (optional):
  // also mark inserted rows as confirmed (import shortcut) — unless polarity is still
  // unclear or the row is excluded.
  const applyExpert = useCallback((changes, confirmAddrs) => {
    const map = new Map(changes.map((c) => { const { address, ...rest } = c; return [address, rest]; }));
    const confirmSet = confirmAddrs ? new Set(confirmAddrs) : null;
    const patches = new Map(); // address → Backend-Payload
    setSensors(sensors.map((s) => {
      const c = map.get(s.address) || {};
      const inConfirm = confirmSet && confirmSet.has(s.address);
      if (!map.has(s.address) && !inConfirm) return s;
      const next = { ...s, ...c };
      const excluded = c.include !== undefined ? !c.include : next.status === "excluded";
      if (inConfirm && !excluded && next.polarity !== "unconfirmed") next.confirmed = true;
      next.status = excluded ? "excluded" : (next.confirmed ? "confirmed" : "unconfirmed");
      const payload = { ...c };
      if (next.confirmed !== s.confirmed) payload.confirmed = next.confirmed;
      if (Object.keys(payload).length) patches.set(s.address, payload);
      return next;
    }));
    if (API && API.live) {
      patches.forEach((payload, address) => { API.patchSensor(address, payload).catch(() => {}); });
    }
  }, [setSensors, sensors]);

  const s4total = counts.conf + counts.unconf;
  const s4pct = s4total > 0 ? Math.round((counts.conf / s4total) * 100) : 100;
  const unconfAddrs = () => new Set(sensors.filter((s) => s.status === "unconfirmed").map((s) => s.address));
  return (
    <div>
      {/* Progress: "X of Y confirmed" — at the top where the eye lands first. */}
      <div className="s4-progress">
        <div className="row-between" style={{ marginBottom: 6 }}>
          <span style={{ fontSize: "var(--text-sm)", fontWeight: 600 }}>{t("s4.progress", { conf: counts.conf, total: s4total })}</span>
          <span className="mono" style={{ fontSize: "var(--text-sm)", color: "var(--fg-muted)" }}>{s4pct}%</span>
        </div>
        <div className="progress-track"><div className="progress-fill" style={{ width: s4pct + "%" }} /></div>
      </div>
      {/* toolbar */}
      <div className="tbl-toolbar">
        <div style={{ display: "flex", gap: "var(--space-3)", flexWrap: "wrap" }}>
          <div className="tbl-search">
            <span className="tbl-search__icon"><Icon name="search" size={17} /></span>
            <input className="input" placeholder={t("s4.search")} value={search} onChange={(e) => setSearch(e.target.value)} />
          </div>
          <Button variant="secondary" size="sm" iconLeft="code" onClick={() => setExpOpen(true)}>{t("exp.open")}</Button>
          <Button variant="primary" size="sm" iconLeft="check" onClick={() => confirmRows(visibleAddrs)}>{t("s4.confirmall")}</Button>
        </div>
        <div className="filterbar">
          <button className="chip chip--unconfirmed" aria-pressed={filters.unconfirmed} onClick={() => toggleFilter("unconfirmed")}>
            <StatusDot tone="unconfirmed" />{t("s4.f.unconfirmed")}<span className="chip__count">{filterCounts.unconfirmed}</span>
          </button>
          <button className="chip" aria-pressed={filters.dups} onClick={() => toggleFilter("dups")}>
            <Icon name="layers" size={14} />{t("s4.f.dups")}<span className="chip__count">{filterCounts.dups}</span>
          </button>
          <button className="chip" aria-pressed={filters.polarity} onClick={() => toggleFilter("polarity")}>
            <Icon name="spark" size={14} />{t("s4.f.polarity")}<span className="chip__count">{filterCounts.polarity}</span>
          </button>
          <button className="chip" aria-pressed={filters.excluded} onClick={() => toggleFilter("excluded")}>
            <Icon name={filters.excluded ? "eye" : "eyeOff"} size={14} />{t("s4.f.excluded")}<span className="chip__count">{counts.excl}</span>
          </button>
        </div>
      </div>

      {/* bulk bar */}
      {sel.size > 0 && (
        <div className="bulkbar" role="region" aria-label="Bulk actions">
          <span className="bulkbar__count">{t("s4.sel", { n: sel.size })}</span>
          <span className="bulkbar__sep" />
          <Button variant="ghost" size="sm" iconLeft="check" onClick={() => { confirmRows(sel); clearSel(); }}>{t("s4.bulk.confirm")}</Button>
          <BulkMenu label={t("s4.bulk.settype")} options={KINDS.map((k) => ({ value: k, label: kl[k] }))} onPick={(v) => bulkApply(() => ({ kind: v }))} />
          <BulkMenu label={t("s4.bulk.setpol")} options={POLS.map((p) => ({ value: p, label: t("pol." + p) }))} onPick={(v) => bulkApply(() => ({ polarity: v }))} />
          {ROOMS_ENABLED && (
          <BulkMenu label={t("s4.bulk.room")} options={[...rooms.map((r) => ({ value: r, label: r })), { value: "__none", label: t("s4.noroom") }]} onPick={(v) => bulkApply(() => ({ location: v === "__none" ? "" : v }))} />
          )}
          <span className="bulkbar__spacer" style={{ flex: 1 }} />
          <Button variant="ghost" size="sm" iconLeft="x" onClick={() => { excludeRows(sel); clearSel(); }}>{t("s4.bulk.exclude")}</Button>
          <Button variant="ghost" size="sm" onClick={clearSel}>{t("s4.clearsel")}</Button>
        </div>
      )}

      {/* DESKTOP TABLE */}
      <div className="tbl-wrap scroll">
        <table className="tbl">
          <thead>
            <tr>
              <th className="col-check"><Checkbox checked={allVisSel ? true : someVisSel ? "mixed" : false} onChange={selectAllVisible} ariaLabel={t("s4.selall")} /></th>
              <th>{t("s4.col.status")}</th>
              <th className="col-addr">{t("s4.col.addr")}</th>
              <th>{t("s4.col.name")}</th>
              <th>{t("s4.col.class")}</th>
              <th>{t("s4.col.topic")}</th>
              <th className="col-pol">{t("s4.col.pol")}</th>
              <th className="col-inc">{t("s4.col.inc")}</th>
              <th className="col-inc" title={t("s4.col.hk.hint")}>{t("s4.col.hk")}</th>
              <th className="col-actions"></th>
            </tr>
          </thead>
          <tbody>
            {groups.length === 0 && <tr><td colSpan={10} style={{ textAlign: "center", color: "var(--fg-subtle)", height: 80 }}>{t("s4.empty")}</td></tr>}
            {groups.map((g) => {
              const isCol = collapsed.has(g.room);
              const gUnconf = g.items.filter((s) => s.status === "unconfirmed").length;
              const gAddrs = new Set(g.items.map((s) => s.address));
              return (
                <React.Fragment key={g.room || "__none"}>
                  {g.room && (
                  <tr className="grp-head" data-collapsed={isCol}>
                    <td colSpan={10}>
                      <div style={{ display: "flex", alignItems: "center", gap: "var(--space-3)" }}>
                        <button className="grp-head__btn" onClick={() => toggleCollapse(g.room)} style={{ flex: 1 }}>
                          <Icon name="chevronDown" size={16} className="grp-head__chev" />
                          <span className="grp-head__name">{g.room}</span>
                          <span className="grp-head__meta">· {t("s4.grp.sensors", { n: g.items.length })}{gUnconf > 0 ? " · " : ""}</span>
                          {gUnconf > 0 && <Badge tone="unconfirmed">{t("s4.grp.unconf", { n: gUnconf })}</Badge>}
                        </button>
                        <Button variant="ghost" size="sm" iconLeft="check" onClick={() => confirmRows(gAddrs)}>{t("s4.grp.confirmall")}</Button>
                      </div>
                    </td>
                  </tr>
                  )}
                  {(!g.room || !isCol) && g.items.map((s) => (
                    <SensorRow key={s.address} s={s} t={t} kl={kl} selected={sel.has(s.address)} pending={ctx && ctx.hkPending.has(s.address)}
                      onSel={() => toggleSel(s.address)} onPatch={(p) => patch(s.address, () => p)}
                      onConfirm={() => confirmRows(s.address)} onExclude={() => excludeRows(s.address)}
                      onInclude={() => includeRows(s.address)} onOpen={() => setDrawer(s.address)} sensors={sensors} />
                  ))}
                </React.Fragment>
              );
            })}
          </tbody>
        </table>
      </div>

      {/* MOBILE CARDS */}
      <div className="cardlist">
        {groups.map((g) => {
          const isCol = collapsed.has(g.room);
          const gUnconf = g.items.filter((s) => s.status === "unconfirmed").length;
          const gAddrs = new Set(g.items.map((s) => s.address));
          return (
            <div key={g.room || "__none"}>
              {g.room && (
              <div className="grp-head" data-collapsed={isCol} style={{ display: "flex", alignItems: "center", padding: "10px 4px", gap: 8 }}>
                <button className="grp-head__btn" onClick={() => toggleCollapse(g.room)} style={{ flex: 1 }}>
                  <Icon name="chevronDown" size={16} className="grp-head__chev" />
                  <span className="grp-head__name">{g.room}</span>
                  <span className="grp-head__meta">· {g.items.length}</span>
                  {gUnconf > 0 && <Badge tone="unconfirmed">{gUnconf}</Badge>}
                </button>
                <Button variant="ghost" size="sm" iconLeft="check" onClick={() => confirmRows(gAddrs)}>{t("s4.grp.confirmall")}</Button>
              </div>
              )}
              {(!g.room || !isCol) && <div className="stack stack-2" style={{ marginBottom: "var(--space-3)" }}>
                {g.items.map((s) => (
                  <SensorCard key={s.address} s={s} t={t} kl={kl} selected={sel.has(s.address)} pending={ctx && ctx.hkPending.has(s.address)}
                    onSel={() => toggleSel(s.address)} onPatch={(p) => patch(s.address, () => p)}
                    onConfirm={() => confirmRows(s.address)} onExclude={() => excludeRows(s.address)}
                    onInclude={() => includeRows(s.address)} onOpen={() => setDrawer(s.address)} sensors={sensors} />
                ))}
              </div>}
            </div>
          );
        })}
        {groups.length === 0 && <div className="callout callout--info">{t("s4.empty")}</div>}
      </div>

      {/* Proceed notice (non-blocking): unconfirmed sensors stay inactive. */}
      {counts.unconf > 0 ? (
        <div className="callout callout--warn" style={{ marginTop: "var(--space-4)", marginBottom: "var(--space-3)", alignItems: "center", gap: "var(--space-3)" }}>
          <span className="callout__icon"><Icon name="alert" size={18} /></span>
          <span style={{ flex: 1 }}>{t("s4.warn.unconf", { n: counts.unconf })}</span>
          <Button variant="primary" size="sm" iconLeft="check" onClick={() => confirmRows(unconfAddrs())}>{t("s4.confirmall")}</Button>
        </div>
      ) : (
        <div style={{ marginTop: "var(--space-4)", marginBottom: "var(--space-3)", display: "flex", gap: "var(--space-2)", alignItems: "center", color: "var(--state-ok-fg)", fontSize: "var(--text-sm)", fontWeight: 600 }}>
          <Icon name="check" size={16} />{t("s4.alldone")}{counts.excl > 0 && <span style={{ color: "var(--fg-subtle)", fontWeight: 400 }}>· {t("s4.counts", counts)}</span>}
        </div>
      )}

      <SensorDrawer s={drawerSensor} t={t} kl={kl} rooms={rooms} sensors={sensors}
        onClose={() => setDrawer(null)} onPatch={(p) => drawerSensor && patch(drawerSensor.address, () => p)}
        onConfirm={() => { drawerSensor && confirmRows(drawerSensor.address); }} />

      <ExpertEditor open={expOpen} onClose={() => setExpOpen(false)} list={visible} all={sensors} t={t}
        onApply={applyExpert} toast={toast} />
    </div>
  );
}

/* ---------- one desktop row ---------- */
function SensorRow({ s, t, kl, selected, pending, onSel, onPatch, onConfirm, onExclude, onInclude, onOpen, sensors }) {
  const dup = s.dupOf != null ? sensors.find((x) => x.address === s.dupOf) : null;
  return (
    <tr data-status={s.status} data-sel={selected} data-dup={s.dupOf != null} data-hk-pending={!!pending}>
      <td className="col-check"><Checkbox checked={selected} onChange={onSel} ariaLabel={s.name} /></td>
      <td><StatusBadge status={s.status} t={t} /></td>
      <td className="col-addr"><span className="cell-mono">{hex(s.address)}</span></td>
      <td>
        <input className="inline-edit" value={s.name} placeholder={s.raw_name || "—"} onChange={(e) => onPatch({ name: e.target.value })} />
        {dup && <span className="dup-flag" title={t("s4.dupof", { addr: hex(dup.address), name: dup.name })}><Icon name="layers" size={11} />{t("s4.dupflag")}</span>}
      </td>
      <td>
        <select className="inline-select" value={s.kind} onChange={(e) => onPatch({ kind: e.target.value })}>
          {KINDS.map((k) => <option key={k} value={k}>{kl[k]}</option>)}
        </select>
      </td>
      <td><span className="cell-topic mono" title={s.topic}>{s.topic}</span></td>
      <td className="col-pol">
        <select className="inline-select" value={s.polarity} onChange={(e) => onPatch({ polarity: e.target.value })}
          style={s.polarity === "unconfirmed" ? { color: "var(--state-unconfirmed-fg)", fontWeight: 600 } : undefined}>
          {POLS.map((p) => <option key={p} value={p}>{t("pol." + p)}</option>)}
        </select>
      </td>
      <td className="col-inc" style={{ textAlign: "center" }}>
        <Toggle checked={s.include} onChange={(v) => v ? onInclude() : onExclude()} ariaLabel={t("s4.col.inc")} />
      </td>
      <td className="col-inc" style={{ textAlign: "center" }}>
        {HK_KINDS.has(s.kind)
          ? <Toggle checked={!!s.show_in_homekit} onChange={(v) => onPatch({ show_in_homekit: v })} ariaLabel={t("s4.col.hk")} />
          : <span style={{ color: "var(--fg-subtle)" }}>—</span>}
      </td>
      <td className="col-actions">
        <button className="icon-btn" onClick={onOpen} aria-label={t("s4.detail")} style={{ width: 34, height: 34 }}><Icon name="dots" size={16} /></button>
      </td>
    </tr>
  );
}

/* ---------- one mobile card ---------- */
function SensorCard({ s, t, kl, selected, pending, onSel, onPatch, onConfirm, onExclude, onInclude, onOpen, sensors }) {
  const dup = s.dupOf != null ? sensors.find((x) => x.address === s.dupOf) : null;
  return (
    <div className="scard" data-status={s.status} data-sel={selected} data-hk-pending={!!pending}>
      <div className="scard__top">
        <Checkbox checked={selected} onChange={onSel} ariaLabel={s.name} />
        <div className="scard__main">
          <input className="inline-edit" value={s.name} placeholder={s.raw_name || "—"} onChange={(e) => onPatch({ name: e.target.value })} style={{ fontWeight: 600, padding: "2px 4px" }} />
          <div className="scard__meta">
            <span className="scard__addr">{hex(s.address)}</span>
            <StatusBadge status={s.status} t={t} />
            {dup && <Badge tone="warn" icon="layers">{t("s4.dupflag")}</Badge>}
          </div>
        </div>
        <button className="icon-btn" onClick={onOpen} aria-label={t("s4.detail")}><Icon name="dots" size={16} /></button>
      </div>
      <div className="scard__fields">
        <select className="select" value={s.kind} onChange={(e) => onPatch({ kind: e.target.value })} style={{ minHeight: 36 }}>
          {KINDS.map((k) => <option key={k} value={k}>{kl[k]}</option>)}
        </select>
        <select className="select" value={s.polarity} onChange={(e) => onPatch({ polarity: e.target.value })} style={{ minHeight: 36 }}>
          {POLS.map((p) => <option key={p} value={p}>{t("pol." + p)}</option>)}
        </select>
      </div>
      <div style={{ display: "flex", gap: "var(--space-2)", marginTop: "var(--space-2)", alignItems: "center", flexWrap: "wrap" }}>
        <Button variant={s.status === "confirmed" ? "secondary" : "primary"} size="sm" iconLeft="check" onClick={onConfirm} style={{ flex: 1 }}>{t("s4.bulk.confirm")}</Button>
        <label className="toggle"><span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("s4.col.inc")}</span>
          <input type="checkbox" checked={s.include} onChange={(e) => e.target.checked ? onInclude() : onExclude()} />
          <span className="toggle__track"><span className="toggle__thumb" /></span>
        </label>
        {HK_KINDS.has(s.kind) && (
          <label className="toggle"><span style={{ fontSize: "var(--text-xs)", color: "var(--fg-subtle)" }}>{t("s4.col.hk")}</span>
            <input type="checkbox" checked={!!s.show_in_homekit} onChange={(e) => onPatch({ show_in_homekit: e.target.checked })} />
            <span className="toggle__track"><span className="toggle__thumb" /></span>
          </label>
        )}
      </div>
    </div>
  );
}

/* ---------- detail drawer ---------- */
// Sensor kinds with a meaningful HomeKit equivalent (mirrors SensorClass::from_kind in homekit.rs).
const HK_KINDS = new Set(["bewegungsmelder", "magnetkontakt", "schliesskontakt", "gehaeuse", "sabotage", "ueberfallmelder", "rauchmelder", "wassermelder"]);
function SensorDrawer({ s, t, kl, rooms, sensors, onClose, onPatch, onConfirm }) {
  if (!s) return null;
  const dup = s.dupOf != null ? sensors.find((x) => x.address === s.dupOf) : null;
  return (
    <Modal open={!!s} onClose={onClose} title={t("dr.title")} icon="server" iconTone="accent"
      footer={<><Button variant="ghost" onClick={onClose}>{t("btn.close")}</Button>
        <Button variant="primary" iconLeft="check" onClick={() => { onConfirm(); onClose(); }}>{t("s4.bulk.confirm")}</Button></>}>
      <div className="stack stack-4">
        {dup && <Callout tone="warn" icon="layers">{t("s4.dupof", { addr: hex(dup.address), name: dup.name })}</Callout>}
        <div className="stack-3" style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "var(--space-3)" }}>
          <Field label={t("dr.rawaddr")}><input className="input input--mono" value={hex(s.address)} readOnly /></Field>
          <Field label={t("dr.rawname")}><input className="input input--mono" value={s.raw_name || "—"} readOnly /></Field>
        </div>
        <Field label={t("dr.name")}><input className="input" value={s.name} onChange={(e) => onPatch({ name: e.target.value })} /></Field>
        <Field label={t("dr.nameha")} hint="MQTT / Home Assistant"><input className="input input--mono" value={s.name_ha} onChange={(e) => onPatch({ name_ha: e.target.value })} /></Field>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "var(--space-3)" }}>
          <Field label={t("dr.class")}>
            <select className="select" value={s.kind} onChange={(e) => onPatch({ kind: e.target.value })}>{KINDS.map((k) => <option key={k} value={k}>{kl[k]}</option>)}</select>
          </Field>
          <Field label={t("dr.polarity")}>
            <select className="select" value={s.polarity} onChange={(e) => onPatch({ polarity: e.target.value })}>{POLS.map((p) => <option key={p} value={p}>{t("pol." + p)}</option>)}</select>
          </Field>
        </div>
        <div style={{ display: "grid", gridTemplateColumns: ROOMS_ENABLED ? "1fr 1fr" : "1fr", gap: "var(--space-3)" }}>
          {ROOMS_ENABLED && (
          <Field label={t("dr.room")}>
            <select className="select" value={s.location} onChange={(e) => onPatch({ location: e.target.value })}>
              <option value="">{t("s4.noroom")}</option>{rooms.map((r) => <option key={r} value={r}>{r}</option>)}
            </select>
          </Field>
          )}
          <Field label={t("dr.topic")}><input className="input input--mono" value={s.topic} onChange={(e) => onPatch({ topic: e.target.value })} /></Field>
        </div>
        <label className="toggle" style={{ justifyContent: "space-between", width: "100%" }}>
          <span>{t("dr.include")}</span>
          <input type="checkbox" checked={s.include} onChange={(e) => onPatch({ include: e.target.checked, status: e.target.checked ? (s.confirmed ? "confirmed" : "unconfirmed") : "excluded" })} />
          <span className="toggle__track"><span className="toggle__thumb" /></span>
        </label>
        {s.address >= 0x0500 && (
          <div className="stack stack-2">
            <label className="toggle" style={{ justifyContent: "space-between", width: "100%" }}>
              <span>{t("dr.switchable")}</span>
              <input type="checkbox" checked={!!s.switchable} onChange={(e) => onPatch({ switchable: e.target.checked })} />
              <span className="toggle__track"><span className="toggle__thumb" /></span>
            </label>
            {s.switchable && <Callout tone={s.kind === "signalgeber" ? "alarm" : "info"} icon={s.kind === "signalgeber" ? "alert" : "spark"}>
              {s.kind === "signalgeber" ? t("dr.switchable.siren") : t("dr.switchable.hint")}
            </Callout>}
          </div>
        )}
        {HK_KINDS.has(s.kind) && (
          <div className="stack stack-2">
            <label className="toggle" style={{ justifyContent: "space-between", width: "100%" }}>
              <span>{t("dr.homekit")}</span>
              <input type="checkbox" checked={!!s.show_in_homekit} onChange={(e) => onPatch({ show_in_homekit: e.target.checked })} />
              <span className="toggle__track"><span className="toggle__thumb" /></span>
            </label>
            {s.show_in_homekit && <Callout tone="info" icon="spark">{t("dr.homekit.hint")}</Callout>}
          </div>
        )}
        <Callout tone="info" icon="spark"><strong>{t("dr.trigger")}</strong><br />{t("dr.triggernote")}</Callout>
      </div>
    </Modal>
  );
}

export { SensorTable, KIND_LABELS };

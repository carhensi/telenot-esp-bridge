// UI primitives: icons, buttons/modal/field/…, format helpers. Extracted from main.jsx.
import React from 'preact/compat';
const { useState, useEffect, useRef } = React;


/* ---------- Icons (Inline-SVG, ~Stroke, geometrisch) ---------- */
const ICON_PATHS = {
  check: <polyline points="20 6 9 17 4 12" />,
  x: <g><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></g>,
  alert: <g><path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" /><line x1="12" y1="9" x2="12" y2="13" /><line x1="12" y1="17" x2="12.01" y2="17" /></g>,
  info: <g><circle cx="12" cy="12" r="9" /><line x1="12" y1="11" x2="12" y2="16" /><line x1="12" y1="8" x2="12.01" y2="8" /></g>,
  chevronDown: <polyline points="6 9 12 15 18 9" />,
  chevronRight: <polyline points="9 6 15 12 9 18" />,
  search: <g><circle cx="11" cy="11" r="7" /><line x1="21" y1="21" x2="16.65" y2="16.65" /></g>,
  link: <g><path d="M10 13a5 5 0 0 0 7.07 0l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" /><path d="M14 11a5 5 0 0 0-7.07 0l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" /></g>,
  server: <g><rect x="2" y="3" width="20" height="8" rx="2" /><rect x="2" y="13" width="20" height="8" rx="2" /><line x1="6" y1="7" x2="6.01" y2="7" /><line x1="6" y1="17" x2="6.01" y2="17" /></g>,
  shield: <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />,
  eye: <g><path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7-11-7-11-7z" /><circle cx="12" cy="12" r="3" /></g>,
  eyeOff: <g><path d="M17.94 17.94A10 10 0 0 1 12 20c-7 0-11-8-11-8a18 18 0 0 1 5.06-5.94M9.9 4.24A9 9 0 0 1 12 4c7 0 11 8 11 8a18 18 0 0 1-2.16 3.19" /><line x1="1" y1="1" x2="23" y2="23" /></g>,
  copy: <g><rect x="9" y="9" width="13" height="13" rx="2" /><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" /></g>,
  refresh: <g><polyline points="23 4 23 10 17 10" /><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" /></g>,
  arrowRight: <g><line x1="5" y1="12" x2="19" y2="12" /><polyline points="12 5 19 12 12 19" /></g>,
  arrowLeft: <g><line x1="19" y1="12" x2="5" y2="12" /><polyline points="12 19 5 12 12 5" /></g>,
  sun: <g><circle cx="12" cy="12" r="4.5" /><line x1="12" y1="1.5" x2="12" y2="4" /><line x1="12" y1="20" x2="12" y2="22.5" /><line x1="4.2" y1="4.2" x2="6" y2="6" /><line x1="18" y1="18" x2="19.8" y2="19.8" /><line x1="1.5" y1="12" x2="4" y2="12" /><line x1="20" y1="12" x2="22.5" y2="12" /><line x1="4.2" y1="19.8" x2="6" y2="18" /><line x1="18" y1="6" x2="19.8" y2="4.2" /></g>,
  moon: <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />,
  sliders: <g><line x1="4" y1="21" x2="4" y2="14" /><line x1="4" y1="10" x2="4" y2="3" /><line x1="12" y1="21" x2="12" y2="12" /><line x1="12" y1="8" x2="12" y2="3" /><line x1="20" y1="21" x2="20" y2="16" /><line x1="20" y1="12" x2="20" y2="3" /><line x1="1" y1="14" x2="7" y2="14" /><line x1="9" y1="8" x2="15" y2="8" /><line x1="17" y1="16" x2="23" y2="16" /></g>,
  power: <g><path d="M18.36 6.64a9 9 0 1 1-12.73 0" /><line x1="12" y1="2" x2="12" y2="12" /></g>,
  clock: <g><circle cx="12" cy="12" r="9" /><polyline points="12 7 12 12 15 14" /></g>,
  unlock: <g><rect x="3" y="11" width="18" height="11" rx="2" /><path d="M7 11V7a5 5 0 0 1 9.9-1" /></g>,
  home: <g><path d="M3 9.5 12 3l9 6.5V21H3z" /><path d="M9 21v-6h6v6" /></g>,
  trash: <g><polyline points="3 6 5 6 21 6" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></g>,
  layers: <g><polygon points="12 2 2 7 12 12 22 7 12 2" /><polyline points="2 17 12 22 22 17" /><polyline points="2 12 12 17 22 12" /></g>,
  dots: <g><circle cx="12" cy="5" r="1.6" /><circle cx="12" cy="12" r="1.6" /><circle cx="12" cy="19" r="1.6" /></g>,
  help: <g><circle cx="12" cy="12" r="9" /><path d="M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3" /><line x1="12" y1="17" x2="12.01" y2="17" /></g>,
  plug: <g><path d="M9 2v6M15 2v6M6 8h12v3a6 6 0 0 1-12 0z" /><line x1="12" y1="17" x2="12" y2="22" /></g>,
  spark: <path d="M12 2v6m0 8v6M2 12h6m8 0h6M5 5l4 4m6 6 4 4M19 5l-4 4m-6 6-4 4" />,
  // Status pill icons
  chip: <g><rect x="6" y="6" width="12" height="12" rx="1.5" /><path d="M9 2v2M15 2v2M9 20v2M15 20v2M2 9h2M2 15h2M20 9h2M20 15h2" /></g>,
  house: <g><path d="M3 10.5 12 3l9 7.5" /><path d="M5.5 9.5V20h13V9.5" /></g>,
  broadcast: <g><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none" /><path d="M8 16a5.5 5.5 0 0 1 0-8M16 8a5.5 5.5 0 0 1 0 8M5 19a9.5 9.5 0 0 1 0-14M19 5a9.5 9.5 0 0 1 0 14" /></g>,
  webface: <g><circle cx="12" cy="12" r="9" opacity="0.28" /><path d="M12 3a9 9 0 0 1 8.5 6" /><polyline points="12 7.5 12 12 15 13.5" /></g>,
  bell: <g><path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9" /><path d="M13.73 21a2 2 0 0 1-3.46 0" /></g>,
  checkcircle: <g><circle cx="12" cy="12" r="9" /><polyline points="8.5 12.2 11 14.7 15.8 9.5" /></g>,
  download: <g><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><polyline points="7 10 12 15 17 10" /><line x1="12" y1="3" x2="12" y2="15" /></g>,
  upload: <g><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><polyline points="17 8 12 3 7 8" /><line x1="12" y1="3" x2="12" y2="15" /></g>,
};
function Icon({ name, size = 18, sw = 2, className = "", style }) {
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 24 24" fill="none"
      stroke="currentColor" strokeWidth={sw} strokeLinecap="round" strokeLinejoin="round"
      style={style} aria-hidden="true">
      {ICON_PATHS[name]}
    </svg>
  );
}

/* ---------- Brandmark (own identity, NOT Telenot branding) ---------- */
function Brandmark({ size = 30 }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" fill="none" aria-hidden="true" className="brandmark">
      <rect x="1.5" y="1.5" width="29" height="29" rx="8" fill="var(--accent)" />
      <circle cx="10" cy="16" r="3.1" fill="var(--accent-fg)" />
      <circle cx="22" cy="16" r="3.1" fill="var(--accent-fg)" />
      <path d="M10 16 H22" stroke="var(--accent-fg)" strokeWidth="2" strokeLinecap="round" />
      <path d="M16 10.5 V21.5" stroke="var(--accent-fg)" strokeWidth="2" strokeLinecap="round" opacity="0.55" />
    </svg>
  );
}

/* ---------- Button ---------- */
function Button({ variant = "secondary", size, block, iconLeft, iconRight, children, className = "", ...rest }) {
  const cls = ["btn", `btn--${variant}`, size && `btn--${size}`, block && "btn--block", className].filter(Boolean).join(" ");
  return (
    <button className={cls} {...rest}>
      {iconLeft && <Icon name={iconLeft} size={size === "sm" ? 15 : 17} />}
      {children}
      {iconRight && <Icon name={iconRight} size={size === "sm" ? 15 : 17} />}
    </button>
  );
}

/* ---------- StatusDot ---------- */
function StatusDot({ tone, live }) {
  return <span className={`dot dot--${tone}${live ? " dot--live" : ""}`} />;
}

/* ---------- Badge ---------- */
const STAT_TONE = { confirmed: "ok", unconfirmed: "unconfirmed", excluded: "unavail" };
function Badge({ tone = "neutral", icon, children }) {
  return <span className={`badge badge--${tone}`}>{icon && <Icon name={icon} size={12} />}{children}</span>;
}

/* ---------- Checkbox ---------- */
function Checkbox({ checked, onChange, label, ariaLabel }) {
  const state = checked === "mixed" ? "mixed" : checked ? "true" : "false";
  return (
    <button type="button" className="check-tap" onClick={(e) => { e.stopPropagation(); onChange && onChange(); }}
      aria-label={ariaLabel || label} aria-checked={checked === "mixed" ? "mixed" : !!checked} role="checkbox">
      <span className="check" data-checked={state}><Icon name="check" size={14} sw={3} /></span>
    </button>
  );
}

/* ---------- Toggle ---------- */
function Toggle({ checked, onChange, danger, id, ariaLabel }) {
  return (
    <label className={`toggle${danger ? " toggle--danger" : ""}`}>
      <input type="checkbox" checked={!!checked} onChange={(e) => onChange(e.target.checked)} id={id} aria-label={ariaLabel} />
      <span className="toggle__track"><span className="toggle__thumb" /></span>
    </label>
  );
}

/* ---------- Field wrapper ---------- */
function Field({ label, hint, error, children, htmlFor }) {
  return (
    <div className="field">
      {label && <label className="field__label" htmlFor={htmlFor}>{label}</label>}
      {children}
      {error ? <span className="field__err"><Icon name="alert" size={13} />{error}</span>
        : hint ? <span className="field__hint">{hint}</span> : null}
    </div>
  );
}

/* ---------- Password input with reveal ---------- */
function PasswordInput({ value, onChange, placeholder, id, error, autoFocus }) {
  const [show, setShow] = useState(false);
  return (
    <div className="input-affix">
      <input id={id} className={`input input--mono${error ? " input--err" : ""}`} type={show ? "text" : "password"}
        value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} autoFocus={autoFocus}
        autoComplete="off" data-1p-ignore data-lpignore="true" />
      <button type="button" className="input-affix__btn" onClick={() => setShow(!show)} aria-label={show ? "Hide" : "Show"} tabIndex={-1}>
        <Icon name={show ? "eyeOff" : "eye"} size={17} />
      </button>
    </div>
  );
}

/* ---------- Modal ---------- */
function Modal({ open, onClose, title, icon, iconTone = "accent", children, footer, wide }) {
  const ref = useRef(null);
  // Mirror onClose into a ref so the focus/Escape effect does NOT depend on onClose: callers
  // often pass a fresh closure (e.g. () => setDrawer(null)) → otherwise the effect would re-run
  // on every render and the setTimeout auto-focus would steal the cursor while typing (into the
  // first field = read-only raw address). Focus therefore only on open ([open]).
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    if (!open) return;
    const onKey = (e) => { if (e.key === "Escape") onCloseRef.current && onCloseRef.current(); };
    document.addEventListener("keydown", onKey);
    // Focus the first EDITABLE field (skip readOnly raw fields / checkboxes).
    const t = setTimeout(() => {
      const f = ref.current && ref.current.querySelector("input:not([readonly]):not([type=checkbox]), textarea, button, [tabindex]");
      // preventScroll: otherwise .focus() scrolls the modal body to the first button (e.g. "Copy log"
      // below the metrics) → dialog starts scrolled down. This keeps it at the top.
      f && f.focus({ preventScroll: true });
    }, 30);
    return () => { document.removeEventListener("keydown", onKey); clearTimeout(t); };
  }, [open]);
  if (!open) return null;
  return (
    <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose && onClose(); }}>
      <div className={`modal${wide ? " modal--wide" : ""}`} role="dialog" aria-modal="true" aria-label={title} ref={ref}>
        <div className="modal__head">
          {icon && <span className={`modal__icon modal__icon--${iconTone}`}><Icon name={icon} size={20} /></span>}
          <h2 className="modal__title">{title}</h2>
        </div>
        <div className="modal__body scroll">{children}</div>
        {footer && <div className="modal__foot">{footer}</div>}
      </div>
    </div>
  );
}

/* ---------- Callout ---------- */
function Callout({ tone = "info", icon, title, children }) {
  const ic = icon || (tone === "alarm" || tone === "warn" ? "alert" : tone === "accent" ? "shield" : "info");
  return (
    <div className={`callout callout--${tone}`}>
      <span className="callout__icon"><Icon name={ic} size={18} /></span>
      <div>{title && <div className="callout__title">{title}</div>}<div>{children}</div></div>
    </div>
  );
}

/* ---------- helpers ---------- */
const hex = (a) => "0x" + a.toString(16).toUpperCase().padStart(4, "0");
function fmtTime(d = new Date()) { return d.toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit", second: "2-digit" }); }
function fmtDur(s) { const m = Math.floor(s / 60), ss = s % 60; return `${m}:${String(ss).padStart(2, "0")}`; }
function fmtClock(s) { if (s == null) return "—"; if (s >= 3600) return Math.floor(s / 3600) + "h " + Math.floor((s % 3600) / 60) + "m"; const m = Math.floor(s / 60), ss = s % 60; return m + ":" + String(ss).padStart(2, "0"); }
function fmtUptime(s) { const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60); return (d ? d + "d " : "") + h + "h " + m + "m"; }

export { Icon, ICON_PATHS, Brandmark, Button, StatusDot, Badge, Checkbox, Toggle, Field, PasswordInput, Modal, Callout, STAT_TONE, hex, fmtTime, fmtDur, fmtClock, fmtUptime };

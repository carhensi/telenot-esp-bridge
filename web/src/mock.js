// Mock data (offline fallback, stands in for /api/* responses). Extracted from main.jsx.

  // Heuristically guess kind from the raw-name prefix (mirrors the firmware heuristic)
  function guessKind(name) {
    const n = name.toLowerCase();
    if (/^mk |fenster|tür|tuer|t\u00fcr|fster/.test(n)) return "magnetkontakt";
    if (/^bm |^im |bewegung|melder innen/.test(n)) return "bewegungsmelder";
    if (/^rm |rauch/.test(n)) return "rauchmelder";
    if (/^wm |wasser/.test(n)) return "wassermelder";
    if (/^\u00fcm |\u00fcberfall|ueberfall|panik/.test(n)) return "ueberfallmelder";
    if (/^sk |riegel|schließ|schliess/.test(n)) return "schliesskontakt";
    if (/sirene|signalgeber|blitz/.test(n)) return "signalgeber";
    if (/sabotage/.test(n)) return "sabotage";
    if (/geh\u00e4use|gehaeuse|deckel/.test(n)) return "gehaeuse";
    if (/sicherung|netzteil|akku/.test(n)) return "sicherung";
    if (/status|system/.test(n)) return "systemstatus";
    return "unbekannt";
  }

  function slug(s) {
    return s.toLowerCase()
      .replace(/\u00e4/g, "ae").replace(/\u00f6/g, "oe").replace(/\u00fc/g, "ue").replace(/\u00df/g, "ss")
      .replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "");
  }

  let addr = 0x0042;
  let twinAddr = 0x0400; // reserved block for tamper twins (never collides with the base range)
  const sensors = [];

  // sab = true → also creates a tamper twin at twinAddr (same name, sabotage kind)
  function add(name, room, { sab = false, pol = "active_low", unconfPol = false } = {}) {
    const a = addr; addr += 1;
    const kind = guessKind(name);
    sensors.push({
      address: a, name, name_ha: name, kind,
      topic: slug(name), location: room,
      polarity: unconfPol ? "unconfirmed" : pol,
      confirmed: false, status: "unconfirmed", include: true,
      raw_name: name, dupOf: null,
    });
    if (sab) {
      const twin = twinAddr++;
      sensors.push({
        address: twin, name: name + " (Sabotage)", name_ha: name + " Sabotage",
        kind: "sabotage", topic: slug(name) + "_sab", location: room,
        polarity: "active_low", confirmed: false, status: "unconfirmed",
        include: true, raw_name: name + " SAB", dupOf: a,
      });
    }
  }

  /* ---- Erdgeschoss ---- */
  add("MK Haustür", "Erdgeschoss", { sab: true });
  add("SK Haustür", "Erdgeschoss");
  add("BM Flur EG", "Erdgeschoss");
  add("RM Flur EG", "Erdgeschoss");
  add("IM Essen EG", "Erdgeschoss");
  add("BM Wohnzimmer", "Erdgeschoss", { unconfPol: true });
  add("MK Fenster Wohnzimmer L", "Erdgeschoss", { sab: true });
  add("MK Fenster Wohnzimmer R", "Erdgeschoss", { sab: true });
  add("GK Wohnzimmer", "Erdgeschoss");
  add("MK Terrassentür", "Erdgeschoss", { sab: true });
  add("SK Terrassentür", "Erdgeschoss");
  add("BM Küche", "Erdgeschoss");
  add("MK Fenster Küche", "Erdgeschoss", { sab: true });
  add("WM unter Spüle Küche", "Erdgeschoss");
  add("RM Küche", "Erdgeschoss");
  add("MK Fenster Gäste-WC", "Erdgeschoss");

  /* ---- Obergeschoss ---- */
  add("BM Flur OG", "Obergeschoss");
  add("RM Flur OG", "Obergeschoss");
  add("MK Fenster Schlafzimmer L", "Obergeschoss", { sab: true });
  add("MK Fenster Schlafzimmer R", "Obergeschoss", { sab: true });
  add("BM Schlafzimmer", "Obergeschoss", { unconfPol: true });
  add("ÜM Schlafzimmer", "Obergeschoss", { pol: "active_high" });
  add("RM Schlafzimmer", "Obergeschoss");
  add("MK Fenster Kind 1", "Obergeschoss", { sab: true });
  add("RM Kind 1", "Obergeschoss");
  add("MK Fenster Kind 2", "Obergeschoss", { sab: true });
  add("RM Kind 2", "Obergeschoss");
  add("WM Bad OG", "Obergeschoss");
  add("RM Bad OG", "Obergeschoss");
  add("MK Fenster Bad OG", "Obergeschoss");

  /* ---- Dachgeschoss ---- */
  add("BM Büro DG", "Dachgeschoss");
  add("MK Fenster Büro DG", "Dachgeschoss", { sab: true });
  add("RM Büro DG", "Dachgeschoss");
  add("MK Dachfenster Nord", "Dachgeschoss");
  add("MK Dachfenster Süd", "Dachgeschoss");
  add("BM Speicher", "Dachgeschoss", { unconfPol: true });

  /* ---- Keller ---- */
  add("BM Heizraum", "Keller");
  add("WM Heizraum", "Keller");
  add("RM Heizraum", "Keller");
  add("BM Hobbyraum", "Keller");
  add("MK Fenster Hobbyraum", "Keller", { sab: true });
  add("MK Kellertür", "Keller", { sab: true });
  add("BM Vorratsraum", "Keller");
  add("WM Waschküche", "Keller");
  add("MK Fenster Waschküche", "Keller");

  /* ---- Garage ---- */
  add("MK Garagentor", "Garage", { sab: true });
  add("SK Garagentor", "Garage");
  add("BM Garage", "Garage", { unconfPol: true });
  add("RM Garage", "Garage");
  add("MK Nebentür Garage", "Garage", { sab: true });

  /* ---- Außen / Garten ---- */
  add("Sirene außen", "Außen", { pol: "active_high" });
  add("Blitzleuchte außen", "Außen", { pol: "active_high" });
  add("BM Einfahrt", "Außen", { unconfPol: true });
  add("BM Terrasse", "Außen");
  add("MK Gartentür", "Außen", { sab: true });
  add("BM Hofeinfahrt", "Außen", { unconfPol: true });
  add("MK Gartentor", "Außen");
  add("MK Geräteschuppen", "Außen", { sab: true });

  /* ---- mehr Erdgeschoss ---- */
  add("MK Fenster Esszimmer L", "Erdgeschoss", { sab: true });
  add("MK Fenster Esszimmer R", "Erdgeschoss", { sab: true });
  add("GK Esszimmer", "Erdgeschoss");
  add("MK Fenster Arbeitszimmer", "Erdgeschoss", { sab: true });
  add("BM Arbeitszimmer", "Erdgeschoss");
  add("RM Arbeitszimmer", "Erdgeschoss");
  add("MK Kellerabgang Tür", "Erdgeschoss");

  /* ---- mehr Obergeschoss ---- */
  add("MK Fenster Gästezimmer", "Obergeschoss", { sab: true });
  add("BM Gästezimmer", "Obergeschoss");
  add("RM Gästezimmer", "Obergeschoss");
  add("MK Fenster Ankleide", "Obergeschoss");
  add("MK Balkontür OG", "Obergeschoss", { sab: true });
  add("BM Treppenhaus OG", "Obergeschoss", { unconfPol: true });

  /* ---- mehr Keller ---- */
  add("RM Hobbyraum", "Keller");
  add("MK Kellerfenster Nord", "Keller", { sab: true });
  add("MK Kellerfenster Süd", "Keller", { sab: true });
  add("WM Pumpensumpf", "Keller");
  add("BM Technikraum", "Keller");
  add("MK Außentür Keller", "Keller", { sab: true });

  /* ---- mehr Dachgeschoss ---- */
  add("MK Fenster Spitzboden", "Dachgeschoss");
  add("RM Speicher", "Dachgeschoss");
  add("MK Fenster Atelier", "Dachgeschoss", { sab: true });
  add("BM Atelier", "Dachgeschoss");
  add("GK Atelier", "Dachgeschoss");

  /* ---- mehr Garage ---- */
  add("WM Garage Boden", "Garage");
  add("MK Fenster Garage", "Garage");

  /* ---- System / Ohne Raum ---- */
  add("Innensirene", "");
  add("Zentrale Gehäuse", "");
  add("Sabotage Meldelinie 1", "");
  add("Sabotage Meldelinie 2", "");
  add("Netzteil Sicherung", "");
  add("Akku Spannung", "");
  add("Systemstatus Zentrale", "");
  add("Netzausfall 230V", "");

  // a few encoding/empty edge cases (raw name unclear)
  sensors.push({ address: 0x0190, name: "", name_ha: "", kind: "unbekannt", topic: "addr_0190",
    location: "", polarity: "unconfirmed", confirmed: false, status: "unconfirmed", include: true,
    raw_name: "\uFFFD?\uFFFD ", dupOf: null });
  sensors.push({ address: 0x0191, name: "Mel\uFFFDgr 14", name_ha: "Mel_gr_14", kind: "unbekannt", topic: "melgr_14",
    location: "", polarity: "unconfirmed", confirmed: false, status: "unconfirmed", include: true,
    raw_name: "Mel\uFFFDgr 14", dupOf: null });

  sensors.sort((a, b) => a.address - b.address);

  // known rooms for dropdowns
  const rooms = ["Erdgeschoss", "Obergeschoss", "Dachgeschoss", "Keller", "Garage", "Außen"];

  const mqtt = {
    host: "192.168.1.10", port: 8883, tls: true, username: "telenot-bridge",
    password_set: false, topic_root: "ema/v1", device_id: "a1b2c3", ha_discovery: true,
    homekit_mode: false, homekit_disarm: false,
  };

  const device = {
    model: "EMA-Bridge ESP32", fw: "0.1.5", serial: "EB-2K7F-00194",
    mac: "A0:B7:65:1A:2B:C3", ip: "https://192.168.1.42", schema: 1,
    fingerprint: "9F:2C:1A:D4:88:E0:3B:7A",
  };

  const diagnostics = {
    serial: { status: "ok", last_frame_ms: 240 },
    mqtt: { status: "connecting", reconnects: 0, last_error: null, last_pub: "—" },
    heap: { free: 142336, largest_free_block: 98304, low: 121040 },
    firmware_version: "0.1.5", schema_version: 1, uptime_s: 384, reset_reason: "setup_button",
  };

  const log = [
    { seq: 1230, t_ms: 12010, level: "info", msg: "Setup-Modus aktiviert (Taster beim Boot)" },
    { seq: 1231, t_ms: 12180, level: "info", msg: "HTTPS-Server gestartet auf :443 (self-signed)" },
    { seq: 1232, t_ms: 13040, level: "info", msg: "Serial-Link: erstes Telegramm empfangen" },
    { seq: 1233, t_ms: 240120, level: "info", msg: "Discovery abgeschlossen: 123 belegte Adressen" },
    { seq: 1234, t_ms: 250300, level: "warn", msg: "MQTT reconnect (broker timeout)" },
    { seq: 1235, t_ms: 251010, level: "info", msg: "Config validiert: 12 warnings, 0 errors" },
  ];

  const capture = {
    active: false, mode: "listen", sends: "liest nur — sendet nichts an die Anlage",
    bytes_total: 0, frames_ok: 0, frames_err: 0, rec_types: [], elapsed_s: 0,
    buf_used: 0, buf_cap: 32768,
  };
export const MOCK = { sensors, rooms, mqtt, device, diagnostics, log, capture };

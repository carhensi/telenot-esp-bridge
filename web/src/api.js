// REST client (pure ES module). Extracted from main.jsx.
import { waitForCommit } from "./save-config.js";

  const BASE = "/api/v1";
export const API = { live: false, csrf: null };

  async function req(method, path, body) {
    const opts = { method, credentials: "same-origin", headers: {} };
    if (API.csrf) opts.headers["X-CSRF-Token"] = API.csrf;
    if (body !== undefined) {
      opts.headers["Content-Type"] = "application/json";
      opts.body = JSON.stringify(body);
    }
    const r = await fetch(BASE + path, opts);
    const txt = await r.text();
    const data = txt ? JSON.parse(txt) : null;
    if (!r.ok) {
      const err = new Error((data && data.error && data.error.message) || "HTTP " + r.status);
      err.code = (data && data.error && data.error.code) || "http_" + r.status;
      err.status = r.status;
      throw err;
    }
    return data;
  }

  // Maps an ApiSensor response to the frontend shape (dup_of → dupOf).
  function normSensor(s) {
    return Object.assign({}, s, { dupOf: s.dup_of != null ? s.dup_of : null });
  }

  Object.assign(API, {
    normSensor,
    async detect() {
      try {
        await req("GET", "/device");
        API.live = true;
      } catch (_) {
        API.live = false;
      }
      return API.live;
    },
    getDevice: () => req("GET", "/device"),
    async login(password) {
      const d = await req("POST", "/session", { password });
      API.csrf = d.csrf_token;
      return d;
    },
    // Resume an existing session (cookie) after reload/new tab: fetches a fresh
    // CSRF token. Throws (401) when no valid session exists → normal login flow.
    async resumeSession() {
      const d = await req("GET", "/session");
      API.csrf = d.csrf_token;
      return d;
    },
    setPassword: (new_password) => req("POST", "/session/password", { new_password }),
    getConnection: () => req("GET", "/connection"),
    putConnection: (type, ip, port, extra) =>
      req("PUT", "/connection", {
        type, ip: ip || "", port: Number(port) || 0,
        // Panel/baud selection (S2). Absent fields = firmware keeps current values.
        ...(extra ? {
          baud: Number(extra.baud) || 0,
          panel_kind: extra.panel,
          gms_variant: extra.gms,
          hiplex_cmds_verified: !!extra.cmdsVerified,
        } : {}),
      }),
    checkConnection: () => req("POST", "/connection/check"),
    getConnectionCheck: () => req("GET", "/connection/check"),
    startScan: () => req("POST", "/scan/start"),
    getScan: () => req("GET", "/scan"),
    cancelScan: (keep_partial) => req("POST", "/scan/cancel", { keep_partial: !!keep_partial }),
    // Paginated: the ESP32 returns at most 60 sensors per response (large JSON bodies
    // blew the heap). Reassembled here into the full list — callers see nothing of this.
    getSensors: async () => {
      const first = await req("GET", "/sensors?offset=0&limit=60");
      const out = first.sensors || [];
      const total = first.total != null ? first.total : out.length;
      while (out.length < total) {
        const page = await req("GET", `/sensors?offset=${out.length}&limit=60`);
        if (!page.sensors || !page.sensors.length) break;
        out.push(...page.sensors);
      }
      return { ...first, sensors: out };
    },
    patchSensor: (addr, fields) => req("PATCH", "/sensors/" + addr, fields),
    bulkSensors: (addresses, op, extra) =>
      req("POST", "/sensors/bulk", Object.assign({ addresses, op }, extra || {})),
    getMqtt: () => req("GET", "/mqtt"),
    putMqtt: (m) => req("PUT", "/mqtt", m),
    getHomekit: () => req("GET", "/homekit"),
    applyHomekit: async () => {
      if (API.sensorEditsPending) throw new Error("Melderänderungen werden noch gespeichert. Bitte warten.");
      await req("POST", "/homekit/apply");
      await waitForCommit(API);
    },
    reboot: () => req("POST", "/reboot"),
    testMqtt: () => req("POST", "/mqtt/test"),
    getMqttTest: () => req("GET", "/mqtt/test"),
    pinCert: () => req("POST", "/mqtt/pin"),
    unpinCert: () => req("DELETE", "/mqtt/pin"),
    getSecurity: () => req("GET", "/security"),
    putPin: (pin) => req("PUT", "/security/pin", { pin }),
    putRemoteDisarm: (enabled, acknowledged) =>
      req("PUT", "/security/remote-disarm", { enabled, acknowledged: !!acknowledged }),
    getDiagnostics: () => req("GET", "/diagnostics"),
    extendSession: () => req("POST", "/session/extend"),
    getLog: (since) => req("GET", "/diagnostics/log?since_seq=" + (since || 0)),
    // Raw GMS capture for diagnosing unsupported panels.
    startCapture: (mode) => req("POST", "/debug/capture/start", { mode }),
    stopCapture: () => req("POST", "/debug/capture/stop"),
    getCapture: () => req("GET", "/debug/capture"),
    async downloadCapture(trace = false) {
      const name = trace ? "capture.trace" : "capture.bin";
      const r = await fetch(BASE + "/debug/" + name, { credentials: "same-origin" });
      if (!r.ok) throw new Error("HTTP " + r.status);
      const blob = await r.blob();
      const url = window.URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url; a.download = "telenot-" + name;
      document.body.appendChild(a); a.click(); a.remove();
      window.URL.revokeObjectURL(url);
    },
    getState: () => req("GET", "/state"),
    // Settings backup (no secrets): export to file, import from file.
    async downloadBackup() {
      const d = await req("GET", "/backup");
      const blob = new window.Blob([JSON.stringify(d, null, 2)], { type: "application/json" });
      const url = window.URL.createObjectURL(blob);
      const a = document.createElement("a");
      const ts = new Date().toISOString().slice(0, 10);
      a.href = url; a.download = `telenot-backup-${ts}.json`;
      document.body.appendChild(a); a.click(); a.remove();
      window.URL.revokeObjectURL(url);
    },
    importBackup: (backup) => req("POST", "/backup", backup),
    getOta: () => req("GET", "/ota"),
    putOtaSettings: (update_check) => req("PUT", "/ota/settings", { update_check: !!update_check }),
    // Firmware upload via XHR — fetch cannot report upload progress. SHA-256 is
    // computed client-side and verified by the device after streaming.
    uploadOta(file, shaHex, onProgress) {
      return new Promise((resolve, reject) => {
        const xhr = new window.XMLHttpRequest();
        xhr.open("POST", BASE + "/ota/upload");
        xhr.setRequestHeader("X-CSRF-Token", API.csrf || "");
        xhr.setRequestHeader("X-Expected-Sha256", shaHex);
        xhr.upload.onprogress = (e) => {
          if (e.lengthComputable && onProgress) onProgress(e.loaded, e.total);
        };
        xhr.onload = () => {
          let d = null;
          try { d = JSON.parse(xhr.responseText); } catch (_) { /* empty body */ }
          if (xhr.status >= 200 && xhr.status < 300) resolve(d);
          else reject(Object.assign(new Error("HTTP " + xhr.status), { code: d && d.error && d.error.code }));
        };
        xhr.onerror = () => reject(Object.assign(new Error("network"), { code: "network" }));
        xhr.send(file);
      });
    },
    command: (cmd, pin, extra) =>
      req("POST", "/command", Object.assign({ cmd, pin: pin || undefined }, extra || {})),
    getReview: () => req("GET", "/review"),
    commit: (warnings_acknowledged, expected_sensors, expected_confirmed) => req("POST", "/commit", { warnings_acknowledged: !!warnings_acknowledged, expected_sensors, expected_confirmed }),
    getCommit: () => req("GET", "/commit"),
  });

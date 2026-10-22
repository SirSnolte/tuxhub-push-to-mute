// Bridge of the standalone window. It provides the same `window.tuxhub` API
// as tuxhub, so the interface in ui/ runs unchanged in both places.
(() => {
  if (window.tuxhub) return;
  const pending = new Map();
  const listeners = new Map();
  let next = 0;

  function emit(event, params) {
    for (const cb of listeners.get(event) ?? []) cb(params);
    for (const cb of listeners.get("*") ?? []) cb(event, params);
  }

  // Messages from the service: replies carry the id of the request.
  window.__tuxhubReceive = (m) => {
    if (m.id != null && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      if (m.error != null) p.reject(new Error(m.error.message ?? String(m.error)));
      else p.resolve(m.result);
    } else if (typeof m.method === "string") {
      emit(m.method, m.params);
    }
  };

  function request(op, payload) {
    return new Promise((resolve, reject) => {
      const id = ++next;
      pending.set(id, { resolve, reject });
      window.ipc.postMessage(JSON.stringify({ op, id, ...payload }));
    });
  }

  window.tuxhub = {
    standalone: true,
    call: (method, params = {}) => request("call", { method, params }),
    action: (action) => request("action", { action }),
    getSettings: () => request("getSettings", {}),
    saveSettings: (values) => request("saveSettings", { values }),
    launch: () => Promise.resolve(),
    on(event, cb) {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event).add(cb);
      return () => listeners.get(event).delete(cb);
    },
  };

  // Appearance and language follow the desktop.
  const initial = window.__tuxhubInitial ?? {};
  const media = matchMedia("(prefers-color-scheme: dark)");
  let dark = initial.dark ?? media.matches;
  media.addEventListener("change", (e) => {
    dark = e.matches;
    emit("theme", { dark });
  });
  // After all scripts of the page have registered their listeners.
  document.addEventListener("DOMContentLoaded", () => {
    emit("theme", { dark });
    emit("locale", { locale: initial.locale ?? (navigator.language?.startsWith("de") ? "de" : "en") });
  });
})();

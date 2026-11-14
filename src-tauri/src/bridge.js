// Injected into every page of the Silo window. On the server's own pages it
// hands plain `/watch/:id` playback to the native mpv player and leaves
// everything else (watch parties, shuffles, audiobooks, the reader) to the
// web app. On the bundled launcher page it does nothing.
(() => {
  if (window.__siloNativeBridge) return;
  window.__siloNativeBridge = true;
  if (!/^https?:$/.test(location.protocol) || location.hostname === "tauri.localhost") return;

  const internals = () => window.__TAURI_INTERNALS__;
  const invoke = (cmd, args) =>
    internals() ? internals().invoke(cmd, args) : Promise.reject(new Error("bridge unavailable"));

  // ---- credentials the web app is using -----------------------------------
  // Captured from the app's own requests, never stored. The bearer is only
  // used to approve the player's own device sign-in.
  const auth = { bearer: null, profileId: null, profileToken: null };

  function readHeader(headers, name) {
    if (!headers) return null;
    if (headers instanceof Headers) return headers.get(name);
    if (Array.isArray(headers)) {
      const hit = headers.find(([k]) => k.toLowerCase() === name.toLowerCase());
      return hit ? hit[1] : null;
    }
    for (const k of Object.keys(headers)) if (k.toLowerCase() === name.toLowerCase()) return headers[k];
    return null;
  }

  function capture(input, init) {
    const url = new URL(input instanceof Request ? input.url : String(input), location.href);
    if (url.origin !== location.origin) return url;
    const headers = (init && init.headers) || (input instanceof Request ? input.headers : null);
    const authz = readHeader(headers, "Authorization");
    if (authz && authz.startsWith("Bearer ")) auth.bearer = authz.slice(7);
    const pid = readHeader(headers, "X-Profile-Id");
    if (pid) auth.profileId = pid;
    const ptok = readHeader(headers, "X-Profile-Token");
    if (ptok) auth.profileToken = ptok;
    return url;
  }

  // While the fallback path below is backing out of the web player, its own
  // playback start is held so the server never opens a second session.
  let holdWebStart = false;
  const realFetch = window.fetch.bind(window);
  window.fetch = function (input, init) {
    let url;
    try {
      url = capture(input, init);
    } catch {
      return realFetch(input, init);
    }
    const method = ((init && init.method) || (input instanceof Request ? input.method : "GET")).toUpperCase();
    if (holdWebStart && method === "POST" && url.pathname === "/api/v2/playback/start") {
      return new Promise((_, reject) => {
        const signal = (init && init.signal) || (input instanceof Request ? input.signal : null);
        if (signal) signal.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
      });
    }
    return realFetch(input, init);
  };

  // ---- which navigations go native ----------------------------------------
  let nativeOk = null;
  invoke("native_available").then((ok) => (nativeOk = !!ok), () => (nativeOk = false));

  function watchRequest(href) {
    let url;
    try {
      url = new URL(href, location.href);
    } catch {
      return null;
    }
    if (url.origin !== location.origin) return null;
    const m = url.pathname.match(/^\/watch\/([^/]+)\/?$/);
    if (!m) return null;
    const q = url.searchParams;
    // Watch parties and shuffles need the web player's room and queue logic.
    if (q.has("room_id") || q.has("room_token") || q.has("shuffle")) return null;
    return {
      content_id: decodeURIComponent(m[1]),
      file_id: q.get("fileId") || null,
      library_id: q.get("libraryId") || null,
      restart: q.get("restart") === "1",
    };
  }

  function viewer() {
    let profileId = auth.profileId;
    let profileToken = auth.profileToken;
    try {
      profileId = profileId || localStorage.getItem("profile_id");
      profileToken = profileToken || localStorage.getItem("profile_token");
    } catch {}
    return profileId ? { profile_id: profileId, profile_token: profileToken || null } : null;
  }

  function canGoNative(req) {
    return req && nativeOk === true && auth.bearer && viewer();
  }

  function playNative(req) {
    toast("Opening the player…");
    return invoke("native_play", {
      args: { request: req, viewer: viewer(), bearer: auth.bearer, title: null },
    }).catch((e) => toast(String(e), true));
  }

  // Play buttons and cards are links: take the click before React Router.
  window.addEventListener(
    "click",
    (ev) => {
      if (ev.defaultPrevented || ev.button !== 0 || ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return;
      const a = ev.target instanceof Element ? ev.target.closest("a[href]") : null;
      if (!a || (a.target && a.target !== "_self")) return;
      const req = watchRequest(a.getAttribute("href"));
      if (!canGoNative(req)) return;
      ev.preventDefault();
      ev.stopPropagation();
      playNative(req);
    },
    true,
  );

  // Fallback for programmatic navigation to /watch: let the router land,
  // hold the web player's start, step back, and play natively.
  for (const name of ["pushState", "replaceState"]) {
    const original = history[name].bind(history);
    history[name] = function (state, title, url) {
      const result = original(state, title, url);
      const req = url != null ? watchRequest(url) : null;
      if (canGoNative(req)) {
        holdWebStart = true;
        playNative(req);
        setTimeout(() => {
          if (name === "pushState") history.back();
          else original(null, "", "/");
          setTimeout(() => (holdWebStart = false), 1500);
        }, 0);
      }
      return result;
    };
  }

  // ---- status ---------------------------------------------------------------
  let toastEl = null;
  let toastTimer = 0;
  function toast(text, isError = false) {
    if (!document.body) return;
    if (!toastEl) {
      toastEl = document.createElement("div");
      toastEl.setAttribute("role", "status");
      Object.assign(toastEl.style, {
        position: "fixed", left: "50%", bottom: "32px", transform: "translateX(-50%)",
        maxWidth: "min(560px, calc(100vw - 32px))", padding: "12px 18px", borderRadius: "10px",
        font: "500 14px/1.4 system-ui, sans-serif", color: "#fff", zIndex: "2147483647",
        boxShadow: "0 8px 30px rgba(0,0,0,.35)", transition: "opacity .2s", pointerEvents: "none",
      });
      document.body.appendChild(toastEl);
    }
    toastEl.textContent = text;
    toastEl.style.background = isError ? "#8f1d1d" : "rgba(20,20,24,.92)";
    toastEl.style.opacity = "1";
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toastEl.style.opacity = "0"), isError ? 8000 : 3000);
  }

  function nudgeRefresh() {
    // The web app refetches on focus, which brings Continue Watching and
    // progress bars up to date after mpv closes.
    window.dispatchEvent(new Event("focus"));
    document.dispatchEvent(new Event("visibilitychange"));
  }

  function listen() {
    const ev = window.__TAURI__ && window.__TAURI__.event;
    if (!ev) return setTimeout(listen, 200);
    ev.listen("silo-native://state", ({ payload }) => {
      if (payload.state === "playing") toast("Playing in mpv");
      if (payload.state === "ended") nudgeRefresh();
      if (payload.state === "error") {
        toast(payload.message || "Playback failed", true);
        nudgeRefresh();
      }
    });
  }
  listen();

  // Ctrl+Shift+O: pick another server.
  window.addEventListener("keydown", (ev) => {
    if (ev.ctrlKey && ev.shiftKey && ev.key.toLowerCase() === "o") invoke("change_server");
  });
})();

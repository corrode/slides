const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};
const settle = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};
const waitFor = async (condition) => {
  const deadline = Date.now() + 4_000;
  while (!(await condition())) {
    if (Date.now() > deadline)
      throw new Error("Timed out waiting for browser state");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
};
const fragment = (index) =>
  `<main id="live-view" class="audience-shell" data-slide-index="${index}"><span data-live-status>Live</span><span class="nav-position">${index + 1}/5</span></main>`;
const ended =
  '<main id="live-view" class="audience-shell"><section class="session-complete-state">Session ended</section></main>';

const assertErrorTarget = (f) => {
  assert(
    f.d.querySelectorAll("#live-error").length === 1,
    "Expected exactly one live error target",
  );
  const target = f.d.querySelector(".live-notices > #live-error");
  assert(
    target && !f.view.contains(target),
    "Live error target must be in the notice stack outside the live view",
  );
  return target;
};

async function fixture(options = {}) {
  const frame = document.createElement("iframe");
  const query = new URLSearchParams({
    view: options.view || "audience",
    case: options.case || "fake",
  });
  if (!options.native) query.set("manual", "1");
  if (options.ended) query.set("ended", "1");
  frame.src = `/fixture?${query}`;
  const loaded = new Promise((resolve) => {
    frame.onload = resolve;
  });
  document.body.append(frame);
  await loaded;
  const w = frame.contentWindow;
  const d = w.document;
  const errors = [];
  w.addEventListener("error", (event) => errors.push(event.message));
  w.addEventListener("unhandledrejection", (event) =>
    errors.push(String(event.reason)),
  );
  const sources = [];
  const timers = new Map();
  const fetches = [];
  const globalListeners = [];
  let now = 0;
  let timerId = 0;
  let hidden = false;
  let online = true;
  let swapCount = 0;
  d.addEventListener("htmx:after:swap", () => {
    swapCount += 1;
  });
  const inject = () =>
    new Promise((resolve, reject) => {
      const script = d.createElement("script");
      script.src = "/assets/live.js";
      script.onload = resolve;
      script.onerror = reject;
      d.head.append(script);
    });
  if (!options.native) {
    for (const target of [w, d]) {
      const add = target.addEventListener.bind(target);
      target.addEventListener = (type, listener, options) => {
        if (
          [
            "online",
            "offline",
            "visibilitychange",
            "pagehide",
            "pageshow",
            "DOMContentLoaded",
          ].includes(type)
        ) {
          globalListeners.push(type);
        }
        return add(type, listener, options);
      };
    }
    w.Date.now = () => now;
    w.Math.random = () => 0.5;
    w.setTimeout = (callback, delay = 0) => {
      const id = ++timerId;
      timers.set(id, { callback, at: now + delay });
      return id;
    };
    w.clearTimeout = (id) => timers.delete(id);
    Object.defineProperty(d, "hidden", { get: () => hidden });
    Object.defineProperty(w.navigator, "onLine", { get: () => online });
    w.EventSource = class extends w.EventTarget {
      constructor(url) {
        super();
        this.url = url;
        this.closed = false;
        sources.push(this);
        assert(
          sources.filter((source) => !source.closed).length === 1,
          "More than one active EventSource",
        );
      }
      close() {
        this.closed = true;
      }
      emit(type, data = "") {
        this.dispatchEvent(new w.MessageEvent(type, { data }));
      }
    };
    w.fetch = async (url, options) => {
      fetches.push({ url, options });
      return new w.Response(null, { status: 204 });
    };
    if (options.unsupported) w.EventSource = undefined;
    await inject();
  }
  const api = {
    w,
    d,
    sources,
    timers,
    fetches,
    globalListeners,
    errors,
    inject,
    get source() {
      return sources.at(-1);
    },
    get state() {
      return d.body.dataset.liveConnection;
    },
    get view() {
      return d.getElementById("live-view");
    },
    get swapCount() {
      return swapCount;
    },
    nextDelay() {
      return Math.min(...[...timers.values()].map((timer) => timer.at - now));
    },
    tick(ms) {
      const end = now + ms;
      for (;;) {
        const next = [...timers.entries()]
          .filter(([, timer]) => timer.at <= end)
          .sort((a, b) => a[1].at - b[1].at)[0];
        if (!next) break;
        timers.delete(next[0]);
        now = next[1].at;
        next[1].callback();
      }
      now = end;
    },
    elapse(ms) {
      now += ms;
    },
    visibility(value) {
      hidden = value;
      d.dispatchEvent(new w.Event("visibilitychange"));
    },
    network(value) {
      online = value;
      w.dispatchEvent(new w.Event(value ? "online" : "offline"));
    },
    async message(html) {
      api.source.emit("message", html);
      await settle();
    },
    async restoreHistory(body) {
      let restored = false;
      const onSwap = (event) => {
        const context = event.detail.ctx;
        if (!context.request?.headers?.["HX-History-Restore-Request"]) return;
        assert(
          context.swap === "outerSync" && context.target === d.body,
          "History did not use the vendored body restoration path",
        );
        restored = true;
      };
      d.addEventListener("htmx:after:swap", onSwap);
      const fetch = w.fetch;
      w.fetch = async (url, options) => {
        fetches.push({ url, options });
        assert(
          options.method === "GET" &&
            options.headers["HX-History-Restore-Request"] === "true",
          "Unexpected history request",
        );
        return new w.Response(
          `<!doctype html><html><head><title>Restored</title></head>${body}</html>`,
          { headers: { "Content-Type": "text/html" } },
        );
      };
      try {
        w.history.pushState({ htmx: true }, "", "/history-away");
        w.history.back();
        await waitFor(async () => {
          // The real history handler starts with htmx.timeout(1).
          api.tick(1);
          await settle();
          return restored;
        });
        assert(
          frame.contentWindow === w && frame.contentDocument === d,
          "History reloaded the document instead of restoring its body",
        );
        await settle();
      } finally {
        w.fetch = fetch;
        d.removeEventListener("htmx:after:swap", onSwap);
      }
    },
    dispose() {
      w.dispatchEvent(new w.Event("pagehide"));
      frame.remove();
    },
  };
  return api;
}

export async function run() {
  const results = [];
  const test = async (name, options, body) => {
    let f;
    try {
      f = await fixture(options);
      assertErrorTarget(f);
      await body(f);
      assertErrorTarget(f);
      assert(
        f.errors.length === 0,
        `Uncaught browser errors: ${f.errors.join("; ")}`,
      );
      results.push({ name });
    } catch (error) {
      results.push({ name, error: error.stack || String(error) });
    } finally {
      f?.dispose();
    }
  };

  for (const view of ["presenter", "audience"]) {
    await test(
      `${view} template: single owner, real outerMorph, no mutation retries`,
      { view },
      async (f) => {
        assert(f.sources.length === 1, "Template opened duplicate streams");
        assert(
          !f.d.querySelector('script[src*="hx-sse"]'),
          "Legacy SSE extension still loaded",
        );
        assert(
          f.source.url ===
            (view === "presenter"
              ? "/sessions/fake/events?view=presenter"
              : "/test-events?case=fake&slide=1&presenter_revision=7"),
          "Event URL/query changed",
        );
        f.d.querySelector('[data-nav="next"]').click();
        await settle();
        assert(
          f.fetches.length === 1 && f.fetches[0].options.method === "POST",
          "Fixture mutation not issued exactly once",
        );
        const original = f.view;
        await f.message(fragment(2));
        assert(f.state === "connected", "Not connected after snapshot");
        assert(
          f.view === original && f.view.dataset.slideIndex === "2",
          "outerMorph did not preserve target identity/update content",
        );
        await f.inject();
        assert(
          f.sources.length === 1,
          "Loading manager twice created a second owner",
        );
        f.source.emit("error");
        f.tick(f.nextDelay());
        await f.message(fragment(3));
        assert(f.fetches.length === 1, "SSE recovery retried a mutation POST");
        assert(
          f.view.dataset.slideIndex === "3",
          "Reconnect did not resolve current swap target",
        );
      },
    );
  }

  await test(
    "real htmx history restoration replaces owner, rejects stale work, and keeps global listeners singular",
    {},
    async (f) => {
      await f.message(fragment(2));
      f.tick(2_000);
      const listeners = f.globalListeners.length;
      const old = f.source;
      const oldTransport = f.d.querySelector("[data-live-events]");
      const body = f.d.body;
      const restoredBody = (name, index) =>
        `<body class="live-page"><div data-live-transport data-live-events="/test-events?case=${name}"></div><div class="live-notices"><div id="live-transport-error" role="alert" aria-live="assertive"></div><div id="live-error" role="alert" aria-live="assertive"></div></div>${fragment(index)}</body>`;
      const queueOld = (event) => {
        if (event.detail.ctx.request?.headers?.["HX-History-Restore-Request"])
          old.emit("message", fragment(98));
      };
      const emitDetached = (event) => {
        if (event.target !== body || oldTransport.isConnected) return;
        // This fires synchronously before the DOM observer reconciles owners.
        old.emit("message", fragment(99));
        old.emit("heartbeat", "stale");
      };
      f.d.addEventListener("htmx:before:swap", queueOld);
      f.d.addEventListener("htmx:after:process", emitDetached);
      await f.restoreHistory(restoredBody("restored-a", 3));
      assertErrorTarget(f);
      f.d.removeEventListener("htmx:before:swap", queueOld);
      f.d.removeEventListener("htmx:after:process", emitDetached);
      assert(
        f.d.body === body && !oldTransport.isConnected,
        "Fixture did not replace body children",
      );
      assert(
        old.closed && f.sources.length === 2,
        "History restoration did not replace the transport owner",
      );
      assert(
        f.source.url === "/test-events?case=restored-a",
        "Restored owner used the old URL",
      );
      assert(
        f.view.dataset.slideIndex === "3",
        "Detached owner overwrote restored view",
      );
      old.emit("error");
      assert(!f.source.closed, "Old owner error closed the restored connection");
      await f.message(fragment(4));
      f.tick(2_000);
      assert(
        f.state === "connected" && f.timers.size === 1,
        "Old owner left a watchdog/retry behind",
      );

      const second = f.source;
      await f.restoreHistory(restoredBody("restored-b", 5));
      assertErrorTarget(f);
      assert(
        second.closed && f.sources.length === 3,
        "Repeated history restoration leaked an owner",
      );
      await f.message(fragment(6));
      f.tick(2_000);
      await f.restoreHistory(
        '<body><main id="not-live">Another page</main></body>',
      );
      assert(
        f.source.closed && f.timers.size === 0,
        "Leaving live view leaked transport work",
      );
      const count = f.sources.length;
      f.network(true);
      f.visibility(false);
      assert(
        f.sources.length === count,
        "Global recovery revived an owner on a non-live page",
      );
      await f.restoreHistory(restoredBody("restored-c", 7));
      assertErrorTarget(f);
      assert(
        f.sources.length === count + 1,
        "Returning to live view did not initialize a new owner",
      );
      await f.inject();
      assert(
        f.globalListeners.length === listeners,
        "History/reinjection accumulated global listeners",
      );
      f.source.emit("error");
      f.network(true);
      f.visibility(false);
      assert(
        f.sources.length === count + 2,
        "Global recovery duplicated streams after restoration",
      );
      await f.message(fragment(8));
      assert(
        f.view.dataset.slideIndex === "8" && f.state === "connected",
        "Restored transport failed to apply updates",
      );
    },
  );

  await test(
    "DOM owner removal rejects messages before observer delivery and supports reattachment",
    {},
    async (f) => {
      const transport = f.d.querySelector("[data-live-events]");
      const old = f.source;
      transport.remove();
      old.emit("message", fragment(99));
      await settle();
      assert(
        old.closed && f.view.dataset.slideIndex === "1" && f.timers.size === 0,
        "Detached owner remained active",
      );
      old.emit("error");
      old.emit("heartbeat", "stale");
      assert(f.timers.size === 0, "Detached owner callbacks restarted timers");
      f.d.body.prepend(transport);
      await settle();
      assert(
        f.sources.length === 2,
        "Reattached transport retained a stale initialized marker",
      );
      await f.message(fragment(2));
      assert(f.state === "connected", "Reattached owner did not recover");
    },
  );

  for (const view of ["presenter", "audience"]) {
    await test(
      `${view}: OOB render errors preserve view and heartbeat never swaps`,
      { view },
      async (f) => {
        await f.message(fragment(2));
        const original = f.view;
        await f.message(
          '<div id="live-error" class="notice error" role="alert" hx-swap-oob="outerHTML">Server render failed; next update retries.</div>',
        );
        assert(
          f.view === original && f.view.dataset.slideIndex === "2",
          "OOB-only event erased the view",
        );
        const errorTarget = assertErrorTarget(f);
        assert(
          errorTarget.textContent.includes("Server render failed"),
          "OOB error was not displayed",
        );
        const count = f.swapCount;
        f.source.emit("heartbeat", "alive");
        await settle();
        assert(f.swapCount === count, "Heartbeat triggered a swap");
        assert(
          errorTarget.textContent.includes("Server render failed"),
          "Heartbeat cleared the render error before a successful snapshot",
        );
        await f.message(fragment(3));
        assert(
          f.view === original && f.view.dataset.slideIndex === "3",
          "Successful snapshot did not update the view",
        );
        assert(
          assertErrorTarget(f) === errorTarget,
          "Successful snapshot replaced the external error target",
        );
        assert(
          !errorTarget.hasChildNodes(),
          "Successful snapshot did not clear render error",
        );
      },
    );
  }

  await test(
    "errors close native retries, use capped jitter, ignore stale callbacks",
    {},
    async (f) => {
      for (let i = 0; i < 10; i++) {
        const old = f.source;
        old.emit("error");
        const delay = f.nextDelay();
        assert(
          old.closed && f.timers.size === 1,
          "Failure did not close source/leave exactly one retry",
        );
        assert(
          delay >= 400 && delay <= 5_000,
          "Retry outside bounded jitter range",
        );
        old.emit("error");
        old.emit("message", fragment(99));
        f.tick(delay);
        await settle();
        assert(
          f.view.dataset.slideIndex === "1",
          "Stale message changed the UI",
        );
      }
      await f.message(fragment(2));
      f.source.emit("error");
      assert(f.nextDelay() === 450, "Healthy snapshot did not reset backoff");
      assert(
        f.d
          .getElementById("live-transport-error")
          .textContent.includes("sign in"),
        "No actionable error/auth guidance",
      );
    },
  );

  await test(
    "12s startup deadline includes open-without-snapshot and heartbeat-only",
    {},
    async (f) => {
      const old = f.source;
      old.emit("open");
      f.tick(11_999);
      old.emit("heartbeat", "alive");
      assert(
        !old.closed && f.state === "connecting",
        "Marked snapshot-less connection healthy",
      );
      f.tick(1);
      assert(
        old.closed && f.state === "reconnecting",
        "Startup deadline did not close stalled connection",
      );
      f.tick(f.nextDelay());
      assert(f.sources.length === 2, "Startup failure never retried");
      await f.message(fragment(2));
      assert(f.state === "connected", "Startup retry did not recover");
    },
  );

  await test(
    "15s heartbeats extend 40s watchdog; silent established stream retries",
    {},
    async (f) => {
      await f.message(fragment(2));
      const old = f.source;
      for (let i = 0; i < 4; i++) {
        f.tick(15_000);
        old.emit("heartbeat", "alive");
      }
      assert(
        !old.closed && f.sources.length === 1,
        "Healthy idle stream reconnected",
      );
      f.tick(39_999);
      assert(!old.closed, "Watchdog fired early");
      f.tick(1);
      assert(
        old.closed && f.state === "reconnecting",
        "Silent stream never reconnected",
      );
      assert(f.view.dataset.slideIndex === "2", "Watchdog removed last slide");
    },
  );

  await test(
    "online/visible recovery is immediate, deduplicated, and never pauses healthy background stream",
    {},
    async (f) => {
      await f.message(fragment(2));
      // Finish app.js's slide-change cue before counting transport timers.
      f.tick(2_000);
      const old = f.source;
      f.visibility(true);
      assert(!old.closed, "Backgrounding closed healthy stream");
      f.elapse(40_001);
      f.visibility(false);
      f.network(true);
      assert(
        old.closed && f.sources.length === 2,
        "Stale foreground recovery delayed/duplicated",
      );
      f.source.emit("error");
      f.network(true);
      f.visibility(false);
      assert(
        f.sources.length === 3 && f.timers.size === 1,
        "Recovery retained a retry timer or duplicate stream",
      );
      await f.message(fragment(3));
      f.tick(2_000);
      f.network(false);
      assert(
        f.source.closed && f.state === "offline" && f.timers.size === 0,
        "Offline did not cancel transport work",
      );
      f.tick(120_000);
      assert(f.sources.length === 3, "Retried while offline");
      f.network(true);
      assert(f.sources.length === 4, "Online recovery not immediate");
    },
  );

  await test(
    "pagehide closes transport; bfcache pageshow restarts only once",
    {},
    async (f) => {
      await f.message(fragment(2));
      f.tick(2_000);
      f.w.dispatchEvent(new f.w.Event("pagehide"));
      assert(
        f.source.closed && f.timers.size === 0,
        "Pagehide leaked connection/timer",
      );
      f.network(true);
      f.visibility(false);
      assert(f.sources.length === 1, "Suspended page reconnected");
      f.w.dispatchEvent(new f.w.Event("pageshow"));
      f.w.dispatchEvent(new f.w.Event("pageshow"));
      assert(f.sources.length === 2, "bfcache restore failed/duplicated");
    },
  );

  await test(
    "invalid SSE fragments preserve last UI and trigger recovery",
    {},
    async (f) => {
      for (const html of [
        "",
        "plain error",
        "<main>Login</main>",
        '<div id="live-error">Not an OOB error</div>',
        fragment(2) + fragment(3),
      ]) {
        await f.message(html);
        assert(
          f.state === "reconnecting" && f.source.closed,
          "Invalid frame accepted",
        );
        assert(
          f.view.dataset.slideIndex === "1",
          "Invalid frame replaced last view",
        );
        f.network(true);
      }
      await f.message(fragment(4));
      assert(
        f.state === "connected",
        "Valid data did not recover after invalid data",
      );
    },
  );

  await test(
    "swap failures are caught and later reconnect can apply updates",
    {},
    async (f) => {
      const swap = f.w.htmx.swap.bind(f.w.htmx);
      f.w.htmx.swap = async () => {
        throw new Error("Synthetic swap failure");
      };
      await f.message(fragment(2));
      assert(
        f.state === "reconnecting" && f.view.dataset.slideIndex === "1",
        "Swap failure was not contained",
      );
      f.w.htmx.swap = swap;
      f.network(true);
      await f.message(fragment(3));
      assert(
        f.view.dataset.slideIndex === "3" && f.state === "connected",
        "Swap queue stayed rejected",
      );
    },
  );

  await test(
    "swap queue is serialized and discards old-generation queued snapshots",
    {},
    async (f) => {
      const swap = f.w.htmx.swap.bind(f.w.htmx);
      let release;
      const gate = new Promise((resolve) => {
        release = resolve;
      });
      let calls = 0;
      f.w.htmx.swap = async (context) => {
        calls += 1;
        if (calls === 1) await gate;
        return swap(context);
      };
      f.source.emit("message", fragment(2));
      f.source.emit("message", fragment(99));
      await settle();
      assert(calls === 1, "Swaps ran concurrently");
      f.source.emit("error");
      f.network(true);
      f.source.emit("message", fragment(3));
      release();
      await settle();
      assert(
        calls === 2 && f.view.dataset.slideIndex === "3",
        "Old queued snapshot applied after reconnect",
      );
    },
  );

  for (const initial of [false, true]) {
    await test(
      `session-ended ${initial ? "initial view" : "snapshot"} permanently stops reconnects`,
      { ended: initial },
      async (f) => {
        if (!initial) await f.message(ended);
        assert(
          f.state === "ended" && f.timers.size === 0,
          "Session-ended state did not stop timers",
        );
        const count = f.sources.length;
        if (initial) assert(count === 0, "Ended initial view opened a stream");
        else assert(f.source.closed, "Ended snapshot left stream open");
        f.network(true);
        f.visibility(false);
        f.w.dispatchEvent(new f.w.Event("pagehide"));
        f.w.dispatchEvent(new f.w.Event("pageshow"));
        f.tick(120_000);
        assert(f.sources.length === count, "Ended session reconnected");
      },
    );
  }

  await test(
    "unsupported browser shows actionable terminal state",
    { unsupported: true },
    async (f) => {
      assert(
        f.state === "disconnected" && f.timers.size === 0,
        "Unsupported EventSource was retried",
      );
      assert(
        f.d
          .getElementById("live-transport-error")
          .textContent.includes("browser"),
        "Missing browser guidance",
      );
    },
  );

  document.cookie = "transport_test=same-origin; SameSite=Lax; path=/";
  for (const status of ["401", "403", "500", "html"]) {
    const name = `native-${status}`;
    await test(
      `native EventSource ${status} preserves UI, then recovers with cookies`,
      { native: true, case: name },
      async (f) => {
        await waitFor(() => f.state === "reconnecting");
        assert(
          f.view?.dataset.slideIndex === "1",
          "HTTP/auth response replaced view",
        );
        await fetch(`/test-recover?case=${name}`);
        f.w.dispatchEvent(new f.w.Event("online"));
        await waitFor(
          () => f.state === "connected" && f.view?.dataset.slideIndex === "2",
        );
        const state = await (await fetch(`/test-state?case=${name}`)).json();
        assert(
          state.requests >= 2 && state.maxActive === 1,
          "Native recovery duplicated streams",
        );
        assert(
          state.cookie.includes("transport_test=same-origin"),
          "Native EventSource lost same-origin credentials",
        );
      },
    );
  }

  await test(
    "native parser handles CRLF split across socket writes",
    { native: true, case: "native-crlf" },
    async (f) => {
      await waitFor(
        () => f.state === "connected" && f.view?.dataset.slideIndex === "2",
      );
      assert(
        f.view.textContent.includes("Complete frame"),
        "Native parser split a valid multiline frame",
      );
      assert(
        f.d.querySelectorAll("#live-view").length === 1,
        "CRLF created duplicate targets",
      );
    },
  );
  return results;
}

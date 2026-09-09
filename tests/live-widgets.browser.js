const assert = (condition, message) => { if (!condition) throw new Error(message); };
const pause = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));
const waitFor = async (condition, timeout = 5_000) => {
  const deadline = performance.now() + timeout;
  while (!condition()) {
    if (performance.now() > deadline) throw new Error("Timed out waiting for browser state");
    await pause(10);
  }
};
const escape = (text) => text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll('"', "&quot;");
let navigation;
const snapshot = ({ index = 1, source = "flowchart TD\nA[Start] --> B[Finish]", code = 'fn main() { println!("hello"); }', ide = "zed://file/main.rs", count = 0, accent = "", widgets = true } = {}) =>
  `<main id="live-view" class="presenter-shell" data-slide-index="${index}"><div id="live-error" role="alert"></div><span class="nav-position">${index + 1}/3</span><button data-color-scheme-toggle>Theme</button><div class="slide-stage"><article class="slide active"><div class="slide-content" style="${accent ? `--highlight:${accent}` : ""}">${widgets ? `<figure class="mermaid-diagram" data-mermaid-diagram><pre class="mermaid-source" data-mermaid-source><code>${escape(source)}</code></pre><div class="mermaid-output" data-mermaid-output hidden></div><p class="mermaid-error" data-mermaid-error role="status" hidden>Could not render this diagram. Check the Mermaid syntax.</p></figure><div class="rust-code" data-rust-code data-code-ide-url="${escape(ide)}"><pre><code>${escape(code)}</code></pre></div>` : "<p>No widgets</p>"}<div class="interaction-body"><span data-count>${count}</span><button aria-pressed="${count > 0}">Vote</button><span data-hands>${count}</span><ol><li>Order ${count}</li></ol></div></div></article></div>${navigation.replaceAll("{first_disabled}", index === 0 ? " disabled" : "").replaceAll("{previous_disabled}", index === 0 ? " disabled" : "").replaceAll("{next_disabled}", index === 2 ? " disabled" : "")}</main>`;

async function fixture(options = {}) {
  const frame = document.createElement("iframe");
  frame.src = "/fixture";
  const loaded = new Promise((resolve) => { frame.onload = resolve; });
  document.body.append(frame);
  await loaded;
  const w = frame.contentWindow;
  const d = w.document;
  const errors = [];
  w.addEventListener("error", (event) => { if (event.message) errors.push(event.message); });
  w.addEventListener("unhandledrejection", (event) => errors.push(String(event.reason)));
  const inject = (src) => new Promise((resolve, reject) => {
    const script = d.createElement("script");
    script.src = src;
    script.onload = resolve;
    script.onerror = () => reject(new Error(`Could not load ${src}`));
    d.head.append(script);
  });
  if (options.baseline) {
    const config = d.createElement("meta");
    config.name = "htmx-config";
    config.content = JSON.stringify({ extensions: "baseline-without-widget-preservation" });
    d.head.append(config);
  }
  let renderCount = 0;
  let releaseRender;
  const renderGate = options.holdRender ? new Promise((resolve) => { releaseRender = resolve; }) : null;
  const boundSources = [];
  if (options.realMermaid) {
    await inject("/assets/vendor/mermaid/mermaid.min.js");
    const render = w.mermaid.render.bind(w.mermaid);
    w.mermaid.render = async (...args) => { renderCount++; return render(...args); };
  } else {
    w.mermaid = {
      initialize() {},
      async render(id, source) {
        renderCount++;
        await renderGate;
        return {
          svg: `<svg xmlns="http://www.w3.org/2000/svg" id="${id}"><text>${escape(source)}</text></svg>`,
          bindFunctions() { boundSources.push(source); },
        };
      },
    };
  }
  const calls = [];
  let finishedRequests = 0;
  d.addEventListener("htmx:finally:request", () => finishedRequests++);
  w.fetch = (url, request) => new Promise((resolve, reject) => {
    const call = {
      url, request, aborted: false,
      resolve(body = "", status = 204) { resolve(new w.Response(status === 204 ? null : body, { status })); },
      reject() { reject(new TypeError("Test network failure")); },
    };
    calls.push(call);
    request.signal?.addEventListener("abort", () => {
      call.aborted = true;
      reject(new w.DOMException("Test request aborted", "AbortError"));
    }, { once: true });
  });
  d.body.insertAdjacentHTML("beforeend", snapshot(options));
  await inject("/assets/htmx.min.js");
  await inject("/assets/app.js");
  d.dispatchEvent(new w.Event("DOMContentLoaded"));
  await waitFor(() => renderCount === 1);
  const api = {
    w, d, calls, errors, boundSources,
    get renderCount() { return renderCount; },
    get finishedRequests() { return finishedRequests; },
    releaseRender,
    query(selector) { return d.querySelector(selector); },
    nav(action) { return d.querySelector(`[data-nav="${action}"]`); },
    async morph(update = {}) {
      await w.htmx.swap({ target: d.querySelector("#live-view"), swap: "outerMorph", text: snapshot(update) });
    },
    async ready() { await waitFor(() => d.querySelector('[data-mermaid-state="ready"]')); },
    dispose() { frame.remove(); },
  };
  if (!options.holdRender) await api.ready();
  return api;
}

export async function run() {
  navigation = await (await fetch("/navigation")).json();
  const results = [];
  const test = async (name, action, options = {}) => {
    let f;
    try {
      f = await fixture(options);
      const detail = await action(f);
      assert(f.errors.length === 0, f.errors.join("\n"));
      results.push({ name, detail });
    } catch (error) { results.push({ name, error: error.stack || String(error) }); }
    finally { f?.dispose(); }
  };

  await test("same-slide snapshots retain SVG/run output/focus while votes, hands and ordering morph", async (f) => {
    const svg = f.query("[data-mermaid-output] svg");
    const run = f.query("[data-playground-run]");
    run.click();
    assert(f.calls.length === 1, "Run was not issued");
    f.calls[0].resolve(JSON.stringify({ success: true, stdout: "hello", stderr: "" }), 200);
    await waitFor(() => f.query('[data-playground-result][data-state="success"]'));
    const output = f.query("[data-playground-output]");
    output.focus();
    for (let count = 1; count <= 10; count++) await f.morph({ count });
    await pause(20);
    assert(f.renderCount === 1, `Rendered ${f.renderCount} times instead of once`);
    assert(f.query("[data-mermaid-output] svg") === svg, "SVG replaced");
    assert(f.query("[data-playground-run]") === run, "Run button replaced");
    assert(f.query("[data-playground-output]") === output && output.textContent === "hello" && !output.hidden, "Run output lost");
    assert(f.d.activeElement === output, "Output focus lost");
    assert(f.d.querySelectorAll(".playground-toolbar").length === 1, "Toolbar duplicated");
    assert(f.query("[data-count]").textContent === "10", "Vote count frozen");
    assert(f.query("[data-hands]").textContent === "10", "Hand count frozen");
    assert(f.query(".interaction-body button").getAttribute("aria-pressed") === "true", "Vote state frozen");
    assert(f.query(".interaction-body li").textContent === "Order 10", "Ordering frozen");
    return "10 snapshots, 0 extra Mermaid renders";
  });

  await test("same-slide snapshot retains in-flight Mermaid and Playground state", async (f) => {
    const run = f.query("[data-playground-run]");
    run.click();
    await f.morph({ count: 2 });
    assert(f.query("[data-playground-run]") === run && run.disabled, "Pending run lost");
    assert(f.renderCount === 1, "Pending Mermaid duplicated");
    f.releaseRender();
    f.calls[0].resolve(JSON.stringify({ success: true, stdout: "completed", stderr: "" }), 200);
    await f.ready();
    await waitFor(() => f.query("[data-playground-output]").textContent === "completed");
    assert(!run.disabled && f.renderCount === 1, "Pending state failed to complete");
  }, { holdRender: true });

  await test("source and IDE metadata changes invalidate only the changed widgets", async (f) => {
    const svg = f.query("[data-mermaid-output] svg");
    const run = f.query("[data-playground-run]");
    const source = "flowchart TD\nC[Changed] --> D[Source]";
    const code = "fn main() {}";
    await f.morph({ source, code });
    await f.ready();
    assert(f.renderCount === 2 && f.query("[data-mermaid-output] svg") !== svg, "Changed Mermaid retained");
    assert(f.query("[data-playground-run]") !== run, "Changed code retained runtime state");
    assert(f.query("[data-playground-result]").hidden, "Changed code did not reset output");
    const changedRun = f.query("[data-playground-run]");
    await f.morph({ source, code, ide: "zed://file/other.rs" });
    assert(f.query("[data-playground-run]") !== changedRun, "Changed IDE metadata retained");
    assert(f.query(".playground-toolbar a").getAttribute("href") === "zed://file/other.rs", "Stale IDE link");
    assert(f.renderCount === 2, "Unchanged Mermaid rerendered with code metadata");
  });

  await test("identical source on another slide resets widgets and updates boundary navigation", async (f) => {
    const svg = f.query("[data-mermaid-output] svg");
    const run = f.query("[data-playground-run]");
    await f.morph({ index: 2 });
    await f.ready();
    assert(f.renderCount === 2 && f.query("[data-mermaid-output] svg") !== svg, "Slide change preserved old diagram");
    assert(f.query("[data-playground-run]") !== run, "Slide change preserved run state");
    assert(f.nav("next").disabled, "Last-slide next is enabled");
    await f.morph({ index: 0 });
    await f.ready();
    assert(f.renderCount === 3 && f.nav("first").disabled && f.nav("previous").disabled, "First-slide state is stale");
  });

  await test("color-scheme toggle and inherited theme changes rerender, then preserve the new theme", async (f) => {
    f.query("[data-color-scheme-toggle]").click();
    await waitFor(() => f.renderCount === 2);
    await f.ready();
    await f.morph();
    await pause(20);
    assert(f.renderCount === 2, "Snapshot rerendered the newly themed diagram");
    await f.morph({ accent: "#123456" });
    await f.ready();
    assert(f.renderCount === 3, "Inherited theme change retained stale SVG");
    await f.morph({ accent: "#123456" });
    assert(f.renderCount === 3, "Updated theme not preserved");
  });

  await test("obsolete async Mermaid completion cannot paint changed source", async (f) => {
    const source = "flowchart TD\nNew --> Content";
    await f.morph({ source });
    f.releaseRender();
    await f.ready();
    assert(f.renderCount === 2, "Changed source was not rendered");
    assert(f.boundSources.length === 1 && f.boundSources[0] === source, "Obsolete diagram was applied");
    assert(f.query("[data-mermaid-output]").textContent === source, "Old render overwrote new source");
  }, { holdRender: true });

  await test("removed widgets are not retained and interaction updates continue", async (f) => {
    await f.morph({ widgets: false, count: 7 });
    assert(!f.query("[data-mermaid-diagram]") && !f.query("[data-playground-ready]"), "Removed widgets preserved");
    assert(f.query("[data-count]").textContent === "7", "Non-widget content frozen");
  });

  await test("all navigation shares body:drop across morphs, without replaying rapid clicks", async (f) => {
    f.nav("next").click();
    assert(f.calls.length === 1 && f.calls[0].request.timeout === "8s", "Navigation request config not applied");
    assert(!f.nav("next").disabled, "Request-driven disabling remains");
    for (const action of ["first", "previous", "current", "next"]) f.nav(action).click();
    await f.morph();
    f.d.dispatchEvent(new f.w.KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    f.nav("previous").click();
    assert(f.calls.length === 1, "Navigation overlapped during morph");
    f.calls[0].resolve();
    await waitFor(() => f.finishedRequests === 1);
    await pause(30);
    assert(f.calls.length === 1, "Dropped relative POST was queued/retried");
    f.nav("previous").click();
    assert(f.calls.length === 2, "Fresh click not accepted immediately after completion");
    f.calls[1].resolve();
    await waitFor(() => f.finishedRequests === 2);
  });

  await test("request completion never re-enables server-disabled first/previous/next", async (f) => {
    f.nav("next").click();
    await f.morph({ index: 2 });
    assert(f.nav("next").disabled, "Last-slide boundary not applied");
    f.calls[0].resolve();
    await waitFor(() => f.finishedRequests === 1);
    assert(f.nav("next").disabled, "Request cleanup re-enabled last-slide next");
    f.nav("first").click();
    await f.morph({ index: 0 });
    f.calls[1].resolve();
    await waitFor(() => f.finishedRequests === 2);
    assert(f.nav("first").disabled && f.nav("previous").disabled, "Request cleanup re-enabled first-slide controls");
  });

  await test("body gate survives replacing the navigation source node", async (f) => {
    const original = f.nav("next");
    original.click();
    await f.w.htmx.swap({ target: f.query("#live-view"), swap: "outerHTML", text: snapshot() });
    assert(f.nav("next") !== original, "Fixture did not replace navigation");
    f.nav("previous").click();
    assert(f.calls.length === 1, "New source bypassed the stable body gate");
    f.calls[0].resolve();
    await waitFor(() => f.finishedRequests === 1);
    f.nav("next").click();
    assert(f.calls.length === 2, "Body gate did not release after source removal");
    f.calls[1].resolve();
    await waitFor(() => f.finishedRequests === 2);
  });

  await test("hung navigation aborts after 8s, reports failure, releases gate and never retries", async (f) => {
    const start = performance.now();
    f.nav("next").click();
    f.nav("previous").click();
    await waitFor(() => f.finishedRequests === 1, 10_000);
    const elapsed = performance.now() - start;
    assert(f.calls[0].aborted && elapsed >= 7_900 && elapsed < 10_000, `Wrong timeout: ${elapsed}ms`);
    assert(f.query("#live-error").textContent.includes("try again"), "Timeout not reported");
    await pause(50);
    assert(f.calls.length === 1, "Timed-out relative POST retried");
    f.nav("current").click();
    assert(f.calls.length === 2, "Timeout left navigation busy");
    f.calls[1].resolve();
    await waitFor(() => f.finishedRequests === 2);
    return `abort/gate release in ${Math.round(elapsed)}ms; 0 retries`;
  });

  await test("HTTP and network failures release navigation without retrying mutations", async (f) => {
    f.nav("next").click();
    f.calls[0].resolve('<div role="alert">Unavailable</div>', 503);
    await waitFor(() => f.finishedRequests === 1);
    assert(f.query("#live-error").textContent === "Unavailable", "HTTP failure hidden");
    f.nav("previous").click();
    f.calls[1].reject();
    await waitFor(() => f.finishedRequests === 2);
    assert(f.query("#live-error").textContent.includes("try again"), "Network failure hidden");
    await pause(30);
    assert(f.calls.length === 2, "Failed navigation retried");
    f.nav("current").click();
    assert(f.calls.length === 3, "Failed request retained gate");
    f.calls[2].resolve();
    await waitFor(() => f.finishedRequests === 3);
  });

  for (const baseline of [true, false]) {
    await test(`real Mermaid: ${baseline ? "baseline without preservation" : "unchanged-widget preservation"}`, async (f) => {
      const start = performance.now();
      for (let count = 1; count <= 5; count++) {
        await f.morph({ count });
        await f.ready();
      }
      const elapsed = performance.now() - start;
      assert(f.renderCount === (baseline ? 6 : 1), `Unexpected render count ${f.renderCount}`);
      return `5 snapshots: ${f.renderCount - 1} extra renders, ${elapsed.toFixed(1)}ms through widget readiness`;
    }, { realMermaid: true, baseline });
  }
  return results;
}

// Run: node tests/live-transport.mjs [path/to/chrome]
// Requires Node >= 22 (built-in WebSocket) and local Chrome/Chromium. No npm packages.
import { createServer } from "node:http";
import { readFile, mkdtemp, rm, access } from "node:fs/promises";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { once } from "node:events";

const root = fileURLToPath(new URL("../", import.meta.url));
const candidates = [
  process.argv[2],
  process.env.CHROME_BIN,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
].filter(Boolean);
let chrome;
for (const candidate of candidates) {
  try {
    await access(candidate);
    chrome = candidate;
    break;
  } catch {}
}
if (!chrome)
  throw new Error("Pass a Chrome executable path or set CHROME_BIN.");

const cases = new Map();
const stats = (name) => {
  if (!cases.has(name))
    cases.set(name, { requests: 0, active: 0, maxActive: 0, recovered: false });
  return cases.get(name);
};
const snapshot = (index = 1, ended = false, presenter = false) =>
  ended
    ? '<main id="live-view" class="audience-shell"><section class="session-complete-state">Session ended</section></main>'
    : `<main id="live-view" class="${presenter ? "presenter" : "audience"}-shell" data-slide-index="${index}"><span data-live-status data-live-label="Live">Live</span><span class="nav-position">${index + 1}/5</span><div class="slide-stage">Slide ${index}</div><button data-nav="next" hx-post="/test-mutation" hx-swap="none">Next</button></main>`;
const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    const send = (body, type = "text/html", status = 200) => {
      res.writeHead(status, {
        "Content-Type": `${type}; charset=utf-8`,
        "Cache-Control": "no-store",
      });
      res.end(body);
    };
    if (url.pathname === "/") {
      return send(
        '<!doctype html><script type="module">window.transportTests = import("/tests/live-transport.browser.js").then(m => m.run());</script>',
      );
    }
    if (url.pathname === "/fixture") {
      const presenter = url.searchParams.get("view") === "presenter";
      let html = await readFile(
        join(root, "templates", presenter ? "presenter.html" : "audience.html"),
        "utf8",
      );
      const name = url.searchParams.get("case") || "fake";
      html = html
        .replace(/{% if has_mermaid %}[\s\S]*?{% endif %}/, "")
        .replace(/<script src="\/assets\/theme.js"[^>]*><\/script>/, "")
        .replace(/<link rel="stylesheet"[^>]*>/, "")
        .replaceAll("{{ title }}", "Transport test")
        .replaceAll("{{ code }}", name)
        .replaceAll(
          "{{ events_url }}",
          `/test-events?case=${name}&slide=1&presenter_revision=7`,
        )
        .replaceAll("{{ theme_style }}", "")
        .replace(
          "{{ initial_live|safe }}",
          snapshot(1, url.searchParams.has("ended"), presenter),
        );
      if (url.searchParams.has("manual"))
        html = html.replace(
          /<script src="\/assets\/live.js"[^>]*><\/script>/,
          "",
        );
      return send(html);
    }
    if (url.pathname === "/test-state")
      return send(
        JSON.stringify(stats(url.searchParams.get("case"))),
        "application/json",
      );
    if (url.pathname === "/test-recover") {
      stats(url.searchParams.get("case")).recovered = true;
      return send("ok", "text/plain");
    }
    if (url.pathname === "/test-mutation")
      throw new Error(
        "Transport tests must never send a real mutation request",
      );
    if (
      url.pathname === "/test-events" ||
      /^\/sessions\/[^/]+\/events$/.test(url.pathname)
    ) {
      const name = url.searchParams.get("case") || url.pathname.split("/")[2];
      const state = stats(name);
      state.requests += 1;
      state.cookie = req.headers.cookie || "";
      if (!state.recovered && /^native-(401|403|500|html)$/.test(name)) {
        const status = name === "native-html" ? 200 : Number(name.slice(7));
        return send(
          "<main>Authentication or HTTP failure: keep the last slide</main>",
          "text/html",
          status,
        );
      }
      state.active += 1;
      state.maxActive = Math.max(state.maxActive, state.active);
      res.writeHead(200, {
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache",
      });
      res.flushHeaders();
      let timer;
      if (name === "native-crlf") {
        res.write(
          'data: <main id="live-view" class="audience-shell" data-slide-index="2">\r',
        );
        timer = setTimeout(
          () =>
            res.write(
              '\ndata: Complete frame</main>\r\n\r\n',
            ),
          20,
        );
      } else {
        res.write(`data: ${snapshot(2)}\n\n`);
        timer = setTimeout(
          () => res.write("event: heartbeat\ndata: alive\n\n"),
          20,
        );
      }
      res.on("close", () => {
        clearTimeout(timer);
        state.active -= 1;
      });
      return;
    }
    const files = new Map([
      ["/assets/htmx.min.js", "assets/htmx.min.js"],
      ["/assets/app.js", "assets/app.js"],
      ["/assets/live.js", "assets/live.js"],
      ["/tests/live-transport.browser.js", "tests/live-transport.browser.js"],
    ]);
    if (files.has(url.pathname))
      return send(
        await readFile(join(root, files.get(url.pathname))),
        "text/javascript",
      );
    send("Not found", "text/plain", 404);
  } catch (error) {
    console.error(error);
    res.writeHead(500);
    res.end("Test fixture failure");
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const profile = await mkdtemp(join(tmpdir(), "slides-transport-test-"));
const child = spawn(
  chrome,
  [
    "--headless=new",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-networking",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "about:blank",
  ],
  { stdio: ["ignore", "ignore", "pipe"] },
);
let socket;
try {
  const debuggerUrl = await new Promise((resolve, reject) => {
    let output = "";
    const timeout = setTimeout(
      () => reject(new Error(`Chrome startup timed out: ${output}`)),
      10_000,
    );
    child.on("error", reject);
    child.stderr.on("data", (chunk) => {
      output += chunk;
      const match = output.match(/DevTools listening on (ws:\/\/\S+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    });
    child.on("exit", (code) => {
      clearTimeout(timeout);
      reject(new Error(`Chrome exited ${code}: ${output}`));
    });
  });
  socket = new WebSocket(debuggerUrl);
  await once(socket, "open");
  let id = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    const request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id);
    if (message.error) request.reject(new Error(JSON.stringify(message.error)));
    else request.resolve(message.result);
  });
  const call = (method, params = {}, sessionId) =>
    new Promise((resolve, reject) => {
      pending.set(++id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params, sessionId }));
    });
  const { targetId } = await call("Target.createTarget", {
    url: "about:blank",
  });
  const { sessionId } = await call("Target.attachToTarget", {
    targetId,
    flatten: true,
  });
  const evaluate = async (expression, awaitPromise = false) => {
    const result = await call(
      "Runtime.evaluate",
      { expression, awaitPromise, returnByValue: true },
      sessionId,
    );
    if (result.exceptionDetails)
      throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  await call("Page.navigate", { url: origin }, sessionId);
  const deadline = Date.now() + 10_000;
  while (!(await evaluate("Boolean(window.transportTests)"))) {
    if (Date.now() > deadline)
      throw new Error("Browser test module did not load");
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  let timeout;
  const results = await Promise.race([
    evaluate("window.transportTests", true),
    new Promise((_, reject) => {
      timeout = setTimeout(
        () => reject(new Error("Browser tests timed out")),
        45_000,
      );
    }),
  ]).finally(() => clearTimeout(timeout));
  for (const result of results)
    console.log(
      `${result.error ? "FAIL" : "PASS"} ${result.name}${result.error ? `\n${result.error}` : ""}`,
    );
  const failed = results.filter((result) => result.error).length;
  console.log(
    `${results.length - failed}/${results.length} transport browser tests passed`,
  );
  if (failed) process.exitCode = 1;
} finally {
  socket?.close();
  if (child.exitCode === null) {
    const exited = once(child, "exit");
    child.kill();
    await exited;
  }
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  await rm(profile, {
    recursive: true,
    force: true,
    maxRetries: 5,
    retryDelay: 100,
  });
}

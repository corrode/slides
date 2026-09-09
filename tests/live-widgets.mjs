// Run: node tests/live-widgets.mjs [path/to/chrome]
// Node >= 22 and local Chrome/Chromium; no npm packages or running app required.
import { createServer } from "node:http";
import { readFile, mkdtemp, rm, access } from "node:fs/promises";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { once } from "node:events";

const root = fileURLToPath(new URL("../", import.meta.url));
const candidates = [
  process.argv[2], process.env.CHROME_BIN,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser",
].filter(Boolean);
let chrome;
for (const candidate of candidates) {
  try { await access(candidate); chrome = candidate; break; } catch {}
}
if (!chrome) throw new Error("Pass a Chrome executable path or set CHROME_BIN.");

// Exercise the actual Rust navigation format string, not a separately maintained
// copy of its htmx attributes. Rust unit tests also check slide-boundary flags.
const render = await readFile(join(root, "src/web/render.rs"), "utf8");
const attributes = render.match(/const NAVIGATION_REQUEST_ATTRIBUTES: &str = r#"(.*?)"#;/)?.[1];
const navigation = [...render.matchAll(/<nav class=\\"presentation-navigation\\"[\s\S]*?<\/nav>/g)]
  .map((match) => match[0]).find((html) => html.includes("{navigation_request}"))
  ?.replaceAll('\\"', '"').replaceAll("{navigation_request}", attributes)
  .replaceAll("{code}", "test").replace(/\{\w+_icon\}/g, "");
if (!attributes || !navigation) throw new Error("Could not extract rendered navigation fixture");

const files = new Map([
  ["/assets/htmx.min.js", "assets/htmx.min.js"],
  ["/assets/app.js", "assets/app.js"],
  ["/assets/app.css", "assets/app.css"],
  ["/assets/vendor/mermaid/mermaid.min.js", "assets/vendor/mermaid/mermaid.min.js"],
  ["/tests/live-widgets.browser.js", "tests/live-widgets.browser.js"],
]);
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://localhost");
  const send = (body, type = "text/html", status = 200) => {
    res.writeHead(status, { "Content-Type": `${type}; charset=utf-8`, "Cache-Control": "no-store" });
    res.end(body);
  };
  try {
    if (url.pathname === "/") return send('<!doctype html><script type="module">window.widgetTests = import("/tests/live-widgets.browser.js").then(m => m.run());</script>');
    if (url.pathname === "/fixture") return send('<!doctype html><html data-color-scheme="dark"><head><link rel="stylesheet" href="/assets/app.css"></head><body class="live-page" data-presentation-theme><div id="live-announcer"></div></body></html>');
    if (url.pathname === "/navigation") return send(JSON.stringify(navigation), "application/json");
    if (files.has(url.pathname)) return send(await readFile(join(root, files.get(url.pathname))), url.pathname.endsWith(".css") ? "text/css" : "text/javascript");
    // Browser tests intercept all mutation requests; none may escape the fixture.
    if (req.method === "POST") throw new Error(`Unexpected real POST: ${req.url}`);
    send("Not found", "text/plain", 404);
  } catch (error) {
    console.error(error);
    send("Test fixture failure", "text/plain", 500);
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const profile = await mkdtemp(join(tmpdir(), "slides-widgets-test-"));
const child = spawn(chrome, [
  "--headless=new", "--no-first-run", "--no-default-browser-check",
  "--disable-background-networking", "--remote-debugging-port=0",
  `--user-data-dir=${profile}`, "about:blank",
], { stdio: ["ignore", "ignore", "pipe"] });
let socket;
try {
  const debuggerUrl = await new Promise((resolve, reject) => {
    let output = "";
    const timeout = setTimeout(() => reject(new Error(`Chrome startup timed out: ${output}`)), 10_000);
    child.on("error", reject);
    child.stderr.on("data", (chunk) => {
      output += chunk;
      const match = output.match(/DevTools listening on (ws:\/\/\S+)/);
      if (match) { clearTimeout(timeout); resolve(match[1]); }
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
  const call = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    pending.set(++id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params, sessionId }));
  });
  const { targetId } = await call("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await call("Target.attachToTarget", { targetId, flatten: true });
  const evaluate = async (expression, awaitPromise = false) => {
    const result = await call("Runtime.evaluate", { expression, awaitPromise, returnByValue: true }, sessionId);
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  await call("Page.navigate", { url: origin }, sessionId);
  const deadline = Date.now() + 10_000;
  while (!(await evaluate("Boolean(window.widgetTests)"))) {
    if (Date.now() > deadline) throw new Error("Browser test module did not load");
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  let timeout;
  const results = await Promise.race([
    evaluate("window.widgetTests", true),
    new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error("Browser tests timed out")), 60_000); }),
  ]).finally(() => clearTimeout(timeout));
  for (const result of results) console.log(`${result.error ? "FAIL" : "PASS"} ${result.name}${result.detail ? ` (${result.detail})` : ""}${result.error ? `\n${result.error}` : ""}`);
  const failed = results.filter((result) => result.error).length;
  console.log(`${results.length - failed}/${results.length} widget/navigation browser tests passed`);
  if (failed) process.exitCode = 1;
} finally {
  socket?.close();
  if (child.exitCode === null) { const exited = once(child, "exit"); child.kill(); await exited; }
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

(() => {
  const managerKey = Symbol.for("slides.liveTransport");
  if (document[managerKey]) return;
  document[managerKey] = true;

  const STARTUP_TIMEOUT = 12_000;
  // The server must send a named, data-bearing heartbeat every 15 seconds.
  // SSE comments are invisible to EventSource and cannot refresh this watchdog.
  const IDLE_TIMEOUT = 40_000;
  const MAX_RETRY_DELAY = 5_000;

  let owner = null;
  let suspended = false;

  function initializeLiveTransport() {
    const transport = document.querySelector("[data-live-events]");
    if (owner?.transport === transport) return;
    owner?.dispose();
    owner = transport ? createLiveTransport(transport) : null;
  }

  function createLiveTransport(transport) {
    let source = null;
    let retryTimer = null;
    let watchdogTimer = null;
    let attempt = 0;
    let generation = 0;
    let receivedMessage = false;
    let lastActivity = 0;
    let stopped = false;
    let swaps = Promise.resolve();

    function setState(state, message = "") {
      transport.dispatchEvent(
        new CustomEvent("slides:live:connection", {
          bubbles: true,
          detail: { state, message },
        }),
      );
    }

    function sessionEnded() {
      return Boolean(
        document.querySelector("#live-view > .session-complete-state"),
      );
    }

    function closeConnection() {
      generation += 1;
      clearTimeout(retryTimer);
      clearTimeout(watchdogTimer);
      retryTimer = null;
      watchdogTimer = null;
      source?.close();
      source = null;
    }

    function stop(state, message) {
      stopped = true;
      closeConnection();
      setState(state, message);
    }

    function offline() {
      if (stopped || suspended) return;
      closeConnection();
      setState(
        "offline",
        "You are offline. The last slide is still shown. Live updates will reconnect when your connection returns.",
      );
    }

    function retry(message) {
      closeConnection();
      if (stopped || suspended) return;
      if (!navigator.onLine) {
        offline();
        return;
      }
      setState("reconnecting", message);
      const delay =
        Math.min(MAX_RETRY_DELAY, 500 * 2 ** attempt) *
        (0.8 + Math.random() * 0.2);
      attempt = Math.min(attempt + 1, 4);
      retryTimer = window.setTimeout(connect, delay);
    }

    function armWatchdog(timeout) {
      clearTimeout(watchdogTimer);
      watchdogTimer = window.setTimeout(() => {
        retry(
          "Live updates stopped responding. Reconnecting automatically; the last slide is still shown.",
        );
      }, timeout);
    }

    function validFragment(html) {
      const template = document.createElement("template");
      template.innerHTML = html;
      const root = template.content.firstElementChild;
      return (
        template.content.childElementCount === 1 &&
        ![...template.content.childNodes].some(
          (node) => node.nodeType === Node.TEXT_NODE && node.textContent.trim(),
        ) &&
        (root.matches("main#live-view") ||
          root.matches('#live-error[hx-swap-oob="outerHTML"]'))
      );
    }

    function connect() {
      if (stopped || suspended) return;
      closeConnection();
      if (!transport.isConnected) {
        stop("disconnected");
        return;
      }
      if (sessionEnded()) {
        stop("ended");
        return;
      }
      if (!navigator.onLine) {
        offline();
        return;
      }
      if (!window.EventSource) {
        stop(
          "disconnected",
          "This browser cannot receive live updates. Open the presentation in a browser with EventSource support.",
        );
        return;
      }

      const currentGeneration = generation;
      const isCurrent = () =>
        generation === currentGeneration &&
        transport.isConnected &&
        !stopped &&
        !suspended;
      receivedMessage = false;
      lastActivity = Date.now();
      setState(
        attempt ? "reconnecting" : "connecting",
        attempt
          ? "Reconnecting to live updates. If this continues, check your connection or reload the page; presenters may need to sign in again."
          : "",
      );
      // Bound time to the first fragment, not just to successful response headers.
      armWatchdog(STARTUP_TIMEOUT);
      try {
        source = new EventSource(transport.dataset.liveEvents);
      } catch {
        retry(
          "Could not open live updates. Retrying automatically. If this continues, reload the page.",
        );
        return;
      }

      source.addEventListener("error", () => {
        if (!isCurrent()) return;
        // Close immediately: only this manager, not EventSource's retry loop,
        // owns reconnect timing. Native errors do not expose HTTP status codes.
        retry(
          "Live updates are unavailable. Retrying automatically. Check your connection; if this continues, reload the page. Presenters may need to sign in again.",
        );
      });
      source.addEventListener("heartbeat", () => {
        if (!isCurrent() || !receivedMessage) return;
        lastActivity = Date.now();
        armWatchdog(IDLE_TIMEOUT);
      });
      source.addEventListener("message", (event) => {
        if (!isCurrent()) return;
        if (!validFragment(event.data)) {
          retry(
            "An invalid live update was received. The last slide is still shown. Reconnecting automatically.",
          );
          return;
        }
        receivedMessage = true;
        lastActivity = Date.now();
        armWatchdog(IDLE_TIMEOUT);
        // Serialize swaps and discard queued work from a closed connection.
        swaps = swaps
          .then(async () => {
            if (!isCurrent()) return;
            const target = document.getElementById("live-view");
            if (!target) throw new Error("The live view is missing");
            await htmx.swap({
              sourceElement: transport,
              target,
              swap: "outerMorph swapEmpty:false",
              text: event.data,
            });
            if (!isCurrent()) return;
            attempt = 0;
            if (sessionEnded()) stop("ended");
            else setState("connected");
          })
          .catch((error) => {
            if (!isCurrent()) return;
            console.warn("Could not apply live update", error);
            retry(
              "Could not apply a live update. Reconnecting automatically. Reload the page if this continues.",
            );
          });
      });
    }

    function recover() {
      if (stopped || suspended) return;
      // Visibility/online events may arrive together. Keep a healthy connection
      // and only replace a stale one (including after throttled background timers).
      const timeout = receivedMessage ? IDLE_TIMEOUT : STARTUP_TIMEOUT;
      if (source && Date.now() - lastActivity < timeout) return;
      attempt = 0;
      connect();
    }

    connect();
    return {
      transport,
      recover,
      offline,
      closeConnection,
      dispose() {
        stopped = true;
        closeConnection();
      },
    };
  }

  function recoverLiveTransport() {
    initializeLiveTransport();
    owner?.recover();
  }

  // htmx history uses outerSync on body: the body survives, its transport does
  // not. Observe identity changes, not just htmx cleanup (plain divs may have
  // no htmx metadata). Unrelated slide mutations keep the existing owner.
  new MutationObserver(initializeLiveTransport).observe(document, {
    childList: true,
    subtree: true,
  });
  window.addEventListener("online", recoverLiveTransport);
  window.addEventListener("offline", () => owner?.offline());
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) recoverLiveTransport();
  });
  window.addEventListener("pagehide", () => {
    suspended = true;
    owner?.closeConnection();
  });
  window.addEventListener("pageshow", () => {
    if (!suspended) return;
    suspended = false;
    recoverLiveTransport();
  });

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", initializeLiveTransport, {
      once: true,
    });
  } else {
    initializeLiveTransport();
  }
})();

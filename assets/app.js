(() => {
  const previousBarValues = new Map();
  const reactionCounts = new Map();
  let slideBeforeSwap = null;
  let previewSlideBeforeSwap = 0;
  let pointerCard = null;
  let editorSplitPointer = null;
  let presenterNotesOpen = true;
  let presenterQuestionsOpen = false;
  let mermaidDiagramId = 0;
  let mermaidLoadPromise = null;
  let mermaidRenderPromise = Promise.resolve();
  const COLOR_SCHEME_STORAGE = "slides-color-scheme";

  function currentColorScheme() {
    return document.documentElement.dataset.colorScheme === "light" ? "light" : "dark";
  }

  function storedColorScheme() {
    try {
      const saved = window.localStorage.getItem(COLOR_SCHEME_STORAGE);
      return saved === "light" || saved === "dark" ? saved : null;
    } catch {
      return null;
    }
  }

  function updateColorSchemeControls() {
    const current = currentColorScheme();
    const next = current === "dark" ? "light" : "dark";
    document.querySelectorAll("[data-color-scheme-toggle]").forEach((button) => {
      button.setAttribute("aria-label", `Use ${next} mode`);
      button.setAttribute("title", `Use ${next} mode`);

      const icon = button.querySelector("[data-color-scheme-icon]");
      const label = button.querySelector("[data-color-scheme-label]");
      if (icon) icon.textContent = next === "light" ? "☀" : "☾";
      if (label) label.textContent = `${next[0].toUpperCase()}${next.slice(1)} mode`;
    });
  }

  function rerenderMermaidDiagrams() {
    mermaidRenderPromise = mermaidRenderPromise
      .then(() => new Promise((resolve) => window.requestAnimationFrame(resolve)))
      .then(() => {
        document.querySelectorAll("[data-mermaid-diagram]").forEach((diagram) => {
          const source = diagram.querySelector("[data-mermaid-source]");
          const output = diagram.querySelector("[data-mermaid-output]");
          const error = diagram.querySelector("[data-mermaid-error]");
          delete diagram.dataset.mermaidState;
          if (source) source.hidden = false;
          if (output) {
            output.replaceChildren();
            output.hidden = true;
          }
          if (error) error.hidden = true;
        });
        return renderMermaidDiagrams(document);
      });
  }

  function applyColorScheme(colorScheme, persist = false) {
    document.documentElement.dataset.colorScheme = colorScheme;
    if (persist) {
      try {
        window.localStorage.setItem(COLOR_SCHEME_STORAGE, colorScheme);
      } catch {
        // The toggle remains usable when storage is unavailable.
      }
    }
    updateColorSchemeControls();
    rerenderMermaidDiagrams();
  }

  function initializeColorScheme() {
    updateColorSchemeControls();
    const preference = window.matchMedia?.("(prefers-color-scheme: dark)");
    preference?.addEventListener("change", (event) => {
      if (!storedColorScheme()) applyColorScheme(event.matches ? "dark" : "light");
    });
  }

  function rememberBars() {
    previousBarValues.clear();
    document.querySelectorAll("[data-live-bar]").forEach((bar) => {
      previousBarValues.set(bar.dataset.liveBar, bar.dataset.barValue || "0");
    });
  }

  function animateBars() {
    document.querySelectorAll("[data-live-bar]").forEach((bar) => {
      const target = bar.dataset.barValue || "0";
      const previous = previousBarValues.get(bar.dataset.liveBar) || "0";
      bar.style.setProperty("transition", "none");
      bar.style.setProperty("--value", `${previous}%`);
      bar.getBoundingClientRect();
      requestAnimationFrame(() => {
        bar.style.removeProperty("transition");
        requestAnimationFrame(() => bar.style.setProperty("--value", `${target}%`));
      });
    });
  }

  function followPresenter() {
    const liveView = document.querySelector(
      '#live-view[data-following-presenter="true"][data-follow-url]',
    );
    const followUrl = liveView?.dataset.followUrl;
    if (followUrl && `${window.location.pathname}${window.location.search}` !== followUrl) {
      window.history.replaceState(window.history.state, "", followUrl);
    }
  }

  function rememberSlide() {
    slideBeforeSwap = document.querySelector("#live-view")?.dataset.slideIndex ?? null;
  }

  function indicateSlideChange() {
    const liveView = document.querySelector("#live-view");
    const currentSlide = liveView?.dataset.slideIndex ?? null;
    if (slideBeforeSwap !== null && currentSlide !== null && currentSlide !== slideBeforeSwap) {
      const slide = liveView.querySelector(".slide-stage, .audience-slide");
      slide?.classList.add("slide-changed");
      window.setTimeout(() => slide?.classList.remove("slide-changed"), 1200);
    }
    slideBeforeSwap = currentSlide;
  }

  function spawnReaction(symbol) {
    const feed = document.querySelector("#reaction-feed");
    if (!feed) return;

    const reaction = document.createElement("span");
    reaction.className = "flying-reaction";
    reaction.textContent = symbol;
    reaction.style.setProperty("--drift", `${Math.round(Math.random() * 80 - 40)}px`);
    reaction.style.setProperty("--start", `${Math.round(Math.random() * 70 - 35)}px`);
    feed.append(reaction);
    window.setTimeout(() => reaction.remove(), 3200);
  }

  function updateReactionFeed(animate) {
    document.querySelectorAll("[data-reaction-key]").forEach((reaction) => {
      const key = reaction.dataset.reactionKey;
      const count = Number.parseInt(reaction.dataset.reactionCount || "0", 10);
      const previous = reactionCounts.get(key);
      if (animate && previous !== undefined && count > previous) {
        const additions = Math.min(count - previous, 6);
        for (let index = 0; index < additions; index += 1) {
          window.setTimeout(
            () => spawnReaction(reaction.dataset.reactionSymbol || "?"),
            index * 90,
          );
        }
      }
      reactionCounts.set(key, count);
    });
  }

  function previewIndexFromUrl() {
    const match = window.location.hash.match(/^#slide-(\d+)$/);
    if (!match) return null;
    const slideNumber = Number.parseInt(match[1], 10);
    return Number.isNaN(slideNumber) ? null : slideNumber - 1;
  }

  function updatePreviewUrl(index) {
    const hash = `#slide-${index + 1}`;
    if (window.location.hash === hash) return;
    window.history.replaceState(
      window.history.state,
      "",
      `${window.location.pathname}${window.location.search}${hash}`,
    );
  }

  function showPreviewSlide(deck, requestedIndex, updateUrl = true) {
    const slides = [...deck.querySelectorAll("[data-preview-slide]")];
    if (slides.length === 0) return;
    const index = Math.max(0, Math.min(requestedIndex, slides.length - 1));
    slides.forEach((slide, slideIndex) => {
      const active = slideIndex === index;
      slide.classList.toggle("active", active);
      slide.setAttribute("aria-current", active ? "true" : "false");
    });
    deck.dataset.slideIndex = `${index}`;
    const previous = deck.querySelector('[data-preview-nav="previous"]');
    const next = deck.querySelector('[data-preview-nav="next"]');
    if (previous) previous.disabled = index === 0;
    if (next) next.disabled = index + 1 === slides.length;
    const position = deck.querySelector("[data-preview-position]");
    if (position) position.textContent = `Slide ${index + 1} of ${slides.length}`;
    if (updateUrl) updatePreviewUrl(index);
  }

  function rememberPreviewSlide() {
    const deck = document.querySelector("[data-preview-deck]");
    previewSlideBeforeSwap = Number.parseInt(deck?.dataset.slideIndex || "0", 10);
  }

  function restorePreviewSlide() {
    const deck = document.querySelector("[data-preview-deck]");
    if (!deck) return;
    showPreviewSlide(deck, previewIndexFromUrl() ?? previewSlideBeforeSwap);
  }

  function initializeMarkdownEditor() {
    const textarea = document.querySelector("[data-markdown-editor]");
    if (!textarea || typeof window.CodeMirror !== "function") return;

    const editor = window.CodeMirror.fromTextArea(textarea, {
      mode: "markdown",
      inputStyle: "contenteditable",
      lineNumbers: true,
      lineWrapping: true,
      fixedGutter: false,
      viewportMargin: 20,
    });
    editor.setSize("100%", "100%");
    const input = editor.getInputField();
    input.setAttribute("aria-label", "Presentation Markdown");
    input.setAttribute("aria-multiline", "true");
    editor.on("change", () => {
      editor.save();
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    });
    window.requestAnimationFrame(() => editor.refresh());
  }

  const EDITOR_SPLIT_MIN = 25;
  const EDITOR_SPLIT_MAX = 70;
  const EDITOR_SPLIT_STORAGE = "slides-editor-pane-width";
  const PRESENTER_NOTES_STORAGE = "slides-presenter-notes-open";
  const PRESENTER_QUESTIONS_STORAGE = "slides-presenter-questions-open";

  function setEditorSplit(layout, requested) {
    const position = Math.max(EDITOR_SPLIT_MIN, Math.min(EDITOR_SPLIT_MAX, requested));
    layout.style.setProperty("--editor-pane-width", `${position}%`);
    layout.querySelector("[data-editor-divider]")?.setAttribute("aria-valuenow", `${Math.round(position)}`);
    return position;
  }

  function restoreEditorSplit() {
    const layout = document.querySelector("[data-editor-split]");
    if (!layout) return;
    try {
      const saved = Number.parseFloat(window.localStorage.getItem(EDITOR_SPLIT_STORAGE) || "42");
      if (!Number.isNaN(saved)) setEditorSplit(layout, saved);
    } catch {
      setEditorSplit(layout, 42);
    }
  }

  function persistEditorSplit(divider) {
    const position = divider.getAttribute("aria-valuenow");
    if (!position) return;
    try {
      window.localStorage.setItem(EDITOR_SPLIT_STORAGE, position);
    } catch {
      // The splitter remains usable when storage is unavailable.
    }
  }

  function initializePresenterNotes() {
    try {
      const saved = window.localStorage.getItem(PRESENTER_NOTES_STORAGE);
      if (saved !== null) presenterNotesOpen = saved === "open";
    } catch {
      presenterNotesOpen = true;
    }
    restorePresenterNotes();
  }

  function restorePresenterNotes() {
    const notes = document.querySelector("[data-presenter-notes]");
    if (notes instanceof HTMLDetailsElement) notes.open = presenterNotesOpen;
  }

  function rememberPresenterNotes(details) {
    presenterNotesOpen = details.open;
    try {
      window.localStorage.setItem(PRESENTER_NOTES_STORAGE, details.open ? "open" : "closed");
    } catch {
      // The notes panel remains usable when storage is unavailable.
    }
  }

  function initializePresenterQuestions() {
    try {
      presenterQuestionsOpen =
        window.localStorage.getItem(PRESENTER_QUESTIONS_STORAGE) === "open";
    } catch {
      presenterQuestionsOpen = false;
    }
    restorePresenterQuestions();
  }

  function restorePresenterQuestions() {
    const questions = document.querySelector("[data-presenter-questions]");
    if (questions instanceof HTMLDetailsElement) questions.open = presenterQuestionsOpen;
  }

  function rememberPresenterQuestions(details) {
    presenterQuestionsOpen = details.open;
    try {
      window.localStorage.setItem(
        PRESENTER_QUESTIONS_STORAGE,
        details.open ? "open" : "closed",
      );
    } catch {
      // The questions panel remains usable when storage is unavailable.
    }
  }

  function resizeEditorFromPointer(event) {
    if (!editorSplitPointer) return;
    const bounds = editorSplitPointer.layout.getBoundingClientRect();
    if (bounds.width === 0) return;
    setEditorSplit(editorSplitPointer.layout, ((event.clientX - bounds.left) / bounds.width) * 100);
    event.preventDefault();
  }

  function finishEditorResize() {
    if (!editorSplitPointer) return;
    persistEditorSplit(editorSplitPointer.divider);
    editorSplitPointer = null;
  }

  function keyboardEditorDivider(event) {
    const divider = event.target.closest?.("[data-editor-divider]");
    const layout = divider?.closest("[data-editor-split]");
    if (!divider || !layout) return false;

    const current = Number.parseFloat(divider.getAttribute("aria-valuenow") || "42");
    let next = null;
    if (event.key === "ArrowLeft") next = current - (event.shiftKey ? 10 : 2);
    if (event.key === "ArrowRight") next = current + (event.shiftKey ? 10 : 2);
    if (event.key === "Home") next = EDITOR_SPLIT_MIN;
    if (event.key === "End") next = EDITOR_SPLIT_MAX;
    if (next === null) return false;

    event.preventDefault();
    setEditorSplit(layout, next);
    persistEditorSplit(divider);
    return true;
  }

  function blocksSlideShortcuts(target) {
    return (
      target instanceof HTMLElement &&
      (target.matches("input, textarea, select") ||
        target.isContentEditable ||
        Boolean(target.closest("[data-ordering-list]")))
    );
  }

  function keyboardAudienceAction(event) {
    const audience = document.querySelector(".audience-shell");
    if (
      !audience ||
      event.defaultPrevented ||
      event.repeat ||
      !event.altKey ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      blocksSlideShortcuts(event.target)
    ) {
      return false;
    }

    const action = {
      KeyH: "hand",
      Digit1: "applause",
      Digit2: "lightbulb",
      Digit3: "question",
    }[event.code];
    if (!action) return false;

    const control = audience.querySelector(
      `[data-audience-shortcut="${action}"]:not([disabled]):not([aria-disabled="true"])`,
    );
    if (!control) return false;
    event.preventDefault();
    control.click();
    return true;
  }

  function activateSlideNavigation(action) {
    const presenter = document.querySelector(".presenter-shell");
    const preview = document.querySelector("[data-preview-deck]");
    if (!presenter && !preview) return false;
    if (preview && !presenter && action === "current") {
      showPreviewSlide(preview, 0);
      return true;
    }

    const selector = presenter ? `[data-nav="${action}"]` : `[data-preview-nav="${action}"]`;
    const control = document.querySelector(
      `${selector}:not([disabled]):not([aria-disabled="true"])`,
    );
    if (!control) return false;
    control.click();
    return true;
  }

  function keyboardNavigation(event) {
    const presenter = document.querySelector(".presenter-shell");
    const preview = document.querySelector("[data-preview-deck]");
    if (!presenter && !preview) return;
    if (event.defaultPrevented || event.repeat || event.metaKey || event.ctrlKey || event.altKey) {
      return;
    }
    if (blocksSlideShortcuts(event.target)) return;
    if (
      event.key === " " &&
      event.target instanceof HTMLElement &&
      event.target.matches("button, a")
    ) {
      return;
    }

    let action = null;
    if (event.key === "ArrowLeft" || event.key === "PageUp") action = "previous";
    if (event.key === "ArrowRight" || event.key === "PageDown") action = "next";
    if (event.key === "Home") action = "current";
    if (event.key === " " && presenter) action = "next";
    if (!action || !activateSlideNavigation(action)) return;
    event.preventDefault();
  }

  function submitDeckForm(button, url, target = "") {
    const sourceForm = document.querySelector(button.dataset.deckForm);
    if (!(sourceForm instanceof HTMLFormElement) || !sourceForm.reportValidity()) return;

    const form = document.createElement("form");
    form.method = "post";
    form.action = url;
    form.target = target;
    form.hidden = true;
    new FormData(sourceForm).forEach((value, name) => {
      if (typeof value !== "string") return;
      const field = document.createElement("input");
      field.type = "hidden";
      field.name = name;
      field.value = value;
      form.append(field);
    });
    document.body.append(form);
    form.submit();
    form.remove();
  }

  async function copyText(value) {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(value);
      return;
    }
    const previousFocus = document.activeElement;
    const input = document.createElement("textarea");
    input.value = value;
    input.readOnly = true;
    input.tabIndex = -1;
    input.setAttribute("aria-hidden", "true");
    input.style.position = "fixed";
    input.style.opacity = "0";
    document.body.append(input);
    try {
      input.select();
      if (!document.execCommand("copy")) throw new Error("Clipboard copy failed");
    } finally {
      input.remove();
      if (previousFocus instanceof HTMLElement) previousFocus.focus();
    }
  }

  async function copyPresentationLink(button) {
    const url = new URL(button.dataset.shareUrl, window.location.origin).href;
    const status = document.querySelector("#share-status");
    try {
      await copyText(url);
      if (status) status.textContent = "Link copied";
    } catch {
      if (status) status.textContent = "Could not copy link";
    }
  }

  async function copyElementValue(button) {
    const target = document.getElementById(button.dataset.copyTarget);
    const status = document.getElementById(button.dataset.copyStatus);
    const value = target instanceof HTMLInputElement ? target.value : target?.textContent;
    if (!value) return;
    try {
      await copyText(value);
      if (status) status.textContent = "Token copied.";
    } catch {
      if (status) status.textContent = "Could not copy the token.";
    }
  }

  function mermaidThemeVariables(diagram) {
    const styles = window.getComputedStyle(diagram);
    const canvas = document.createElement("canvas");
    canvas.width = 1;
    canvas.height = 1;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    const color = (name, fallback) => {
      const value = styles.getPropertyValue(name).trim() || fallback;
      if (!context) return fallback;

      context.clearRect(0, 0, 1, 1);
      context.fillStyle = fallback;
      context.fillStyle = value;
      context.fillRect(0, 0, 1, 1);
      const [red, green, blue, alpha] = context.getImageData(0, 0, 1, 1).data;
      if (alpha === 255) return `rgb(${red}, ${green}, ${blue})`;
      return `rgba(${red}, ${green}, ${blue}, ${(alpha / 255).toFixed(3)})`;
    };
    const background = color("--bg", "#282934");
    const surface = color("--surface-raised", "rgb(255 255 255 / 12%)");
    const text = color("--text", "#e1e1e1");
    const softText = color("--text-soft", "rgb(255 255 255 / 68%)");
    const accent = color("--highlight", "#fc218a");
    const border = color("--border-strong", "rgb(255 255 255 / 32%)");

    return {
      background,
      primaryColor: surface,
      primaryTextColor: text,
      primaryBorderColor: accent,
      secondaryColor: background,
      secondaryTextColor: text,
      secondaryBorderColor: border,
      tertiaryColor: surface,
      tertiaryTextColor: text,
      tertiaryBorderColor: border,
      lineColor: softText,
      textColor: text,
      mainBkg: surface,
      nodeBorder: accent,
      clusterBkg: background,
      clusterBorder: border,
      edgeLabelBackground: background,
      actorBkg: surface,
      actorBorder: accent,
      actorTextColor: text,
      actorLineColor: softText,
      signalColor: softText,
      signalTextColor: text,
      labelBoxBkgColor: surface,
      labelBoxBorderColor: border,
      labelTextColor: text,
      loopTextColor: text,
      noteBkgColor: surface,
      noteBorderColor: accent,
      noteTextColor: text,
    };
  }

  function loadMermaid() {
    if (window.mermaid) return Promise.resolve(true);
    if (mermaidLoadPromise) return mermaidLoadPromise;

    mermaidLoadPromise = new Promise((resolve) => {
      const script = document.createElement("script");
      script.src = "/assets/vendor/mermaid/mermaid.min.js";
      script.onload = () => {
        const loaded = Boolean(window.mermaid);
        if (!loaded) {
          script.remove();
          mermaidLoadPromise = null;
        }
        resolve(loaded);
      };
      script.onerror = () => {
        script.remove();
        mermaidLoadPromise = null;
        resolve(false);
      };
      document.head.append(script);
    });
    return mermaidLoadPromise;
  }

  async function renderMermaidDiagrams(root) {
    const diagrams = root.querySelectorAll(
      "[data-mermaid-diagram]:not([data-mermaid-state])",
    );
    if (diagrams.length === 0) return;

    if (!(await loadMermaid())) {
      diagrams.forEach((diagram) => {
        diagram.dataset.mermaidState = "error";
        const error = diagram.querySelector("[data-mermaid-error]");
        if (error) {
          error.textContent = "Could not load Mermaid. Reload the page to try again.";
          error.hidden = false;
        }
      });
      return;
    }

    for (const diagram of diagrams) {
      const source = diagram.querySelector("[data-mermaid-source]");
      const output = diagram.querySelector("[data-mermaid-output]");
      const error = diagram.querySelector("[data-mermaid-error]");
      if (!source || !output || !error) continue;

      diagram.dataset.mermaidState = "rendering";
      try {
        window.mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          maxTextSize: 50_000,
          theme: "base",
          themeVariables: mermaidThemeVariables(diagram),
        });
        mermaidDiagramId += 1;
        // Keep reduced-motion overrides from changing Mermaid's layout measurements.
        const sandbox = document.createElement("div");
        sandbox.className = "mermaid-render-sandbox";
        document.body.append(sandbox);

        let result;
        try {
          result = await window.mermaid.render(
            `slides-mermaid-${mermaidDiagramId}`,
            source.textContent || "",
            sandbox,
          );
        } finally {
          sandbox.remove();
        }

        output.innerHTML = result.svg;
        result.bindFunctions?.(output);
        source.hidden = true;
        error.hidden = true;
        output.hidden = false;
        diagram.dataset.mermaidState = "ready";
      } catch (renderError) {
        console.warn("Could not render Mermaid diagram", renderError);
        output.replaceChildren();
        output.hidden = true;
        source.hidden = false;
        error.textContent = "Could not render this diagram. Check the Mermaid syntax.";
        error.hidden = false;
        diagram.dataset.mermaidState = "error";
      }
    }
  }

  function initializeMermaidDiagrams(root = document) {
    mermaidRenderPromise = mermaidRenderPromise.then(() => renderMermaidDiagrams(root));
    return mermaidRenderPromise;
  }

  function initializeRustPlaygrounds(root = document) {
    if (document.body.matches("[data-print-deck]")) return;
    root.querySelectorAll("[data-rust-code]:not([data-playground-ready])").forEach((block) => {
      block.dataset.playgroundReady = "true";

      const toolbar = document.createElement("div");
      toolbar.className = "playground-toolbar";
      toolbar.setAttribute("role", "group");
      toolbar.setAttribute("aria-label", "Code actions");

      const copyButton = document.createElement("button");
      copyButton.type = "button";
      copyButton.className = "secondary icon-only";
      copyButton.dataset.playgroundCopy = "";
      copyButton.title = "Copy code";
      copyButton.setAttribute("aria-label", "Copy code");
      copyButton.innerHTML = '<svg class="button-icon" aria-hidden="true" viewBox="0 0 24 24"><rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V7a2 2 0 0 0-2-2H7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h1"/></svg>';

      const runButton = document.createElement("button");
      runButton.type = "button";
      runButton.className = "secondary icon-only";
      runButton.dataset.playgroundRun = "";
      runButton.title = "Run code on play.rust-lang.org";
      runButton.setAttribute("aria-label", "Run code on play.rust-lang.org");
      runButton.innerHTML = '<svg class="button-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="m9 7 8 5-8 5z"/></svg>';
      toolbar.append(copyButton, runButton);

      const copyStatus = document.createElement("span");
      copyStatus.className = "visually-hidden";
      copyStatus.dataset.playgroundCopyStatus = "";
      copyStatus.setAttribute("role", "status");
      copyStatus.setAttribute("aria-live", "polite");

      const result = document.createElement("div");
      result.className = "playground-result";
      result.dataset.playgroundResult = "";
      result.hidden = true;
      const status = document.createElement("p");
      status.className = "playground-status";
      status.dataset.playgroundStatus = "";
      status.setAttribute("role", "status");
      const output = document.createElement("pre");
      output.className = "playground-output";
      output.dataset.playgroundOutput = "";
      output.tabIndex = 0;
      result.append(status, output);

      block.prepend(toolbar, copyStatus);
      block.append(result);
    });
  }

  function rustCodeSource(button) {
    return button
      .closest("[data-rust-code]")
      ?.querySelector(":scope > pre:not([data-playground-output])")?.textContent;
  }

  async function copyRustCode(button) {
    const source = rustCodeSource(button);
    const status = button
      .closest("[data-rust-code]")
      ?.querySelector("[data-playground-copy-status]");
    if (source == null || !status) return;

    try {
      await copyText(source);
      status.textContent = "Code copied.";
    } catch {
      status.textContent = "Could not copy the code.";
    }
  }

  async function runRustCode(button) {
    const block = button.closest("[data-rust-code]");
    const source = rustCodeSource(button);
    const result = block?.querySelector("[data-playground-result]");
    const status = block?.querySelector("[data-playground-status]");
    const output = block?.querySelector("[data-playground-output]");
    if (!block || source == null || !result || !status || !output || button.disabled) return;

    const now = Date.now();
    const lastRunAt = Number.parseInt(button.dataset.lastRunAt || "0", 10);
    if (now - lastRunAt < 750) return;
    button.dataset.lastRunAt = `${now}`;
    button.disabled = true;
    result.hidden = false;
    result.classList.remove("success", "error");
    status.textContent = "Running on play.rust-lang.org…";
    output.textContent = "";

    try {
      const response = await fetch("/api/playground/run", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ code: source }),
      });
      if (!response.ok) {
        if (response.status === 429) {
          status.textContent = "The Playground is rate-limiting requests. Try again shortly.";
        } else if (response.status === 413) {
          status.textContent = "This code block is too large to run.";
        } else {
          status.textContent = "The Playground is unavailable. Try again shortly.";
        }
        result.classList.add("error");
        return;
      }

      const data = await response.json();
      const streams = [data.stdout, data.stderr].filter((value) => value);
      output.textContent = streams.join("\n") || "(no output)";
      status.textContent = data.success ? "Finished." : "Compilation failed.";
      result.classList.add(data.success ? "success" : "error");
    } catch (error) {
      console.error(error);
      status.textContent = "The Playground request failed. Check your connection and try again.";
      result.classList.add("error");
    } finally {
      button.disabled = false;
    }
  }

  function refreshOrderingButtons(list) {
    const cards = [...list.querySelectorAll(".ordering-card")];
    cards.forEach((card, index) => {
      const up = card.querySelector('[data-order-move="up"]');
      const down = card.querySelector('[data-order-move="down"]');
      if (up) up.disabled = index === 0;
      if (down) down.disabled = index + 1 === cards.length;
    });
  }

  function saveOrdering(list) {
    const form = list.closest("form");
    const value = form?.querySelector("[data-order-value]");
    if (!form || !value) return;
    value.value = [...list.querySelectorAll(".ordering-card")]
      .map((card) => card.dataset.orderIndex)
      .join(",");
    refreshOrderingButtons(list);
    form.requestSubmit();
  }

  function moveOrderingCard(button) {
    const card = button.closest(".ordering-card");
    const list = card?.closest("[data-ordering-list]");
    if (!card || !list) return;
    if (button.dataset.orderMove === "up" && card.previousElementSibling) {
      list.insertBefore(card, card.previousElementSibling);
    } else if (button.dataset.orderMove === "down" && card.nextElementSibling) {
      list.insertBefore(card.nextElementSibling, card);
    } else {
      return;
    }
    const cards = [...list.querySelectorAll(".ordering-card")];
    const status = list.closest("form")?.querySelector("[data-order-status]");
    if (status) {
      status.textContent = `${card.dataset.orderLabel} moved to position ${cards.indexOf(card) + 1} of ${cards.length}`;
    }
    saveOrdering(list);
    const sameDirection = card.querySelector(`[data-order-move="${button.dataset.orderMove}"]`);
    const oppositeDirection = card.querySelector(
      `[data-order-move="${button.dataset.orderMove === "up" ? "down" : "up"}"]`,
    );
    const focusTarget = sameDirection?.disabled ? oppositeDirection : sameDirection;
    focusTarget?.focus();
  }

  function formatCreatedAt() {
    document.querySelectorAll("[data-created-at]").forEach((element) => {
      const timestamp = Number.parseInt(element.dataset.createdAt || "", 10);
      if (Number.isNaN(timestamp)) return;
      const createdAt = new Date(timestamp);
      if (Number.isNaN(createdAt.getTime())) return;
      element.dateTime = createdAt.toISOString();
      element.textContent = `Created ${new Intl.DateTimeFormat(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(createdAt)}`;
    });
  }

  document.addEventListener("DOMContentLoaded", () => {
    initializeColorScheme();
    formatCreatedAt();
    initializeMarkdownEditor();
    animateBars();
    followPresenter();
    rememberSlide();
    updateReactionFeed(false);
    restorePreviewSlide();
    restoreEditorSplit();
    initializePresenterNotes();
    initializePresenterQuestions();
    initializeMermaidDiagrams();
    initializeRustPlaygrounds();
  });

  window.addEventListener("load", async () => {
    if (!document.body.matches("[data-print-deck]")) return;
    await initializeMermaidDiagrams();
    await document.fonts?.ready;
    window.setTimeout(() => window.print(), 100);
  });

  document.addEventListener("keydown", (event) => {
    if (keyboardEditorDivider(event)) return;
    if (!keyboardAudienceAction(event)) keyboardNavigation(event);
  });

  window.addEventListener("message", (event) => {
    if (
      event.data?.type !== "slides:navigate" ||
      !["previous", "next", "current"].includes(event.data.action)
    ) {
      return;
    }
    const sourceIsSlideIframe = [...document.querySelectorAll(".slide-iframe")].some(
      (iframe) => iframe.contentWindow === event.source,
    );
    if (sourceIsSlideIframe) activateSlideNavigation(event.data.action);
  });

  window.addEventListener("hashchange", () => {
    const deck = document.querySelector("[data-preview-deck]");
    const index = previewIndexFromUrl();
    if (deck && index !== null) showPreviewSlide(deck, index, false);
  });

  document.addEventListener("input", (event) => {
    if (!event.target.matches?.("#deck-title")) return;
    document.title = `Edit ${event.target.value || "Untitled"} · Slides`;
  });

  document.addEventListener(
    "toggle",
    (event) => {
      if (event.target.matches?.("[data-presenter-notes]")) rememberPresenterNotes(event.target);
      if (event.target.matches?.("[data-presenter-questions]")) {
        rememberPresenterQuestions(event.target);
      }
    },
    true,
  );

  document.addEventListener("submit", (event) => {
    const message = event.target.dataset.confirm;
    if (message && !window.confirm(message)) event.preventDefault();
  });

  document.addEventListener("click", (event) => {
    const colorSchemeToggle = event.target.closest("[data-color-scheme-toggle]");
    if (colorSchemeToggle) {
      applyColorScheme(currentColorScheme() === "dark" ? "light" : "dark", true);
      return;
    }
    const dialogTrigger = event.target.closest("[data-dialog-open]");
    if (dialogTrigger) {
      const dialog = document.getElementById(dialogTrigger.dataset.dialogOpen);
      if (dialog instanceof HTMLDialogElement) {
        dialog.showModal();
        window.requestAnimationFrame(() => dialog.querySelector("[tabindex='-1']")?.focus());
      }
      return;
    }
    const printNow = event.target.closest("[data-print-now]");
    if (printNow) {
      window.print();
      return;
    }
    const openPrint = event.target.closest("[data-print-url]");
    if (openPrint) {
      submitDeckForm(openPrint, openPrint.dataset.printUrl, "_blank");
      return;
    }
    const startPresentation = event.target.closest("[data-present-url]");
    if (startPresentation) {
      submitDeckForm(startPresentation, startPresentation.dataset.presentUrl);
      return;
    }
    const previewControl = event.target.closest("[data-preview-nav]");
    if (previewControl) {
      const deck = previewControl.closest("[data-preview-deck]");
      const current = Number.parseInt(deck?.dataset.slideIndex || "0", 10);
      if (deck) {
        showPreviewSlide(deck, current + (previewControl.dataset.previewNav === "next" ? 1 : -1));
      }
      return;
    }
    const share = event.target.closest("[data-share-url]");
    if (share) {
      copyPresentationLink(share);
      return;
    }
    const copyValue = event.target.closest("[data-copy-target]");
    if (copyValue) {
      copyElementValue(copyValue);
      return;
    }
    const playgroundCopy = event.target.closest("[data-playground-copy]");
    if (playgroundCopy) {
      copyRustCode(playgroundCopy);
      return;
    }
    const playgroundRun = event.target.closest("[data-playground-run]");
    if (playgroundRun) {
      runRustCode(playgroundRun);
      return;
    }
    const move = event.target.closest("[data-order-move]");
    if (move) moveOrderingCard(move);
  });

  document.addEventListener("dragstart", (event) => {
    const card = event.target.closest(".ordering-card[draggable]");
    if (!card) return;
    card.classList.add("dragging");
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", card.dataset.orderIndex);
  });

  document.addEventListener("dragover", (event) => {
    const list = event.target.closest("[data-ordering-list]");
    const dragging = document.querySelector(".ordering-card.dragging");
    if (!list || !dragging) return;
    event.preventDefault();
    const target = event.target.closest(".ordering-card:not(.dragging)");
    if (!target) {
      list.append(dragging);
      return;
    }
    const afterTarget = event.clientY > target.getBoundingClientRect().top + target.offsetHeight / 2;
    list.insertBefore(dragging, afterTarget ? target.nextElementSibling : target);
  });

  document.addEventListener("dragend", (event) => {
    const card = event.target.closest(".ordering-card.dragging");
    if (!card) return;
    const list = card.closest("[data-ordering-list]");
    card.classList.remove("dragging");
    if (list) saveOrdering(list);
  });

  document.addEventListener("pointerdown", (event) => {
    const divider = event.target.closest("[data-editor-divider]");
    const layout = divider?.closest("[data-editor-split]");
    if (divider && layout && event.button === 0) {
      editorSplitPointer = { divider, layout };
      divider.setPointerCapture(event.pointerId);
      resizeEditorFromPointer(event);
      return;
    }
    if (event.pointerType === "mouse") return;
    const handle = event.target.closest(".drag-handle");
    pointerCard = handle?.closest(".ordering-card") || null;
    if (!pointerCard) return;
    pointerCard.classList.add("dragging");
    handle.setPointerCapture(event.pointerId);
    event.preventDefault();
  });

  document.addEventListener("pointermove", (event) => {
    if (editorSplitPointer) {
      resizeEditorFromPointer(event);
      return;
    }
    if (!pointerCard) return;
    const list = pointerCard.closest("[data-ordering-list]");
    const target = document.elementFromPoint(event.clientX, event.clientY)?.closest(
      ".ordering-card:not(.dragging)",
    );
    if (!list || !target || target.parentElement !== list) return;
    const afterTarget = event.clientY > target.getBoundingClientRect().top + target.offsetHeight / 2;
    list.insertBefore(pointerCard, afterTarget ? target.nextElementSibling : target);
    event.preventDefault();
  });

  function finishPointerOrdering() {
    if (!pointerCard) return;
    const card = pointerCard;
    pointerCard = null;
    const list = card.closest("[data-ordering-list]");
    card.classList.remove("dragging");
    if (list) saveOrdering(list);
  }

  document.addEventListener("pointerup", finishEditorResize);
  document.addEventListener("pointercancel", finishEditorResize);
  document.addEventListener("lostpointercapture", finishEditorResize);
  document.addEventListener("pointerup", finishPointerOrdering);
  document.addEventListener("pointercancel", finishPointerOrdering);
  document.addEventListener("lostpointercapture", finishPointerOrdering);

  document.addEventListener("htmx:responseError", (event) => {
    const request = event.detail.elt;
    if (!(request instanceof HTMLElement)) return;
    const questionPanel = request.closest(".question-panel");
    if (questionPanel) {
      const notice = questionPanel.querySelector("[data-question-error]");
      if (notice) notice.innerHTML = event.detail.xhr.responseText;
      return;
    }
    if (!request.matches("#deck-form, [data-deck-action]")) return;
    const notice = document.querySelector("#notice");
    if (!notice) return;
    notice.innerHTML =
      event.detail.xhr.responseText ||
      '<div class="notice error" role="alert">The draft could not be saved.</div>';
  });

  document.addEventListener("htmx:before:swap", () => {
    rememberBars();
    rememberSlide();
    rememberPreviewSlide();
  });

  document.addEventListener("htmx:after:swap", () => {
    animateBars();
    followPresenter();
    indicateSlideChange();
    updateReactionFeed(true);
    restorePreviewSlide();
    restorePresenterNotes();
    restorePresenterQuestions();
    updateColorSchemeControls();
    initializeMermaidDiagrams();
    initializeRustPlaygrounds();
  });
})();

(() => {
  const previousBarValues = new Map();
  const reactionCounts = new Map();
  let slideBeforeSwap = null;
  let previewSlideBeforeSwap = 0;
  let pointerCard = null;

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

  function blocksSlideShortcuts(target) {
    return (
      target instanceof HTMLElement &&
      (target.matches("input, textarea, select") ||
        target.isContentEditable ||
        Boolean(target.closest("[data-ordering-list]")))
    );
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
    if (!action) return;
    if (preview && !presenter && action === "current") {
      event.preventDefault();
      showPreviewSlide(preview, 0);
      return;
    }

    const selector = presenter ? `[data-nav="${action}"]` : `[data-preview-nav="${action}"]`;
    const control = document.querySelector(
      `${selector}:not([disabled]):not([aria-disabled="true"])`,
    );
    if (!control) return;
    event.preventDefault();
    control.click();
  }

  async function copyText(value) {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(value);
      return;
    }
    const input = document.createElement("textarea");
    input.value = value;
    input.style.position = "fixed";
    input.style.opacity = "0";
    document.body.append(input);
    try {
      input.select();
      if (!document.execCommand("copy")) throw new Error("Clipboard copy failed");
    } finally {
      input.remove();
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

  document.addEventListener("DOMContentLoaded", () => {
    animateBars();
    followPresenter();
    rememberSlide();
    updateReactionFeed(false);
    restorePreviewSlide();
  });

  document.addEventListener("keydown", keyboardNavigation);

  window.addEventListener("hashchange", () => {
    const deck = document.querySelector("[data-preview-deck]");
    const index = previewIndexFromUrl();
    if (deck && index !== null) showPreviewSlide(deck, index, false);
  });

  document.addEventListener("submit", (event) => {
    const message = event.target.dataset.confirm;
    if (message && !window.confirm(message)) event.preventDefault();
  });

  document.addEventListener("click", (event) => {
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
    if (event.pointerType === "mouse") return;
    const handle = event.target.closest(".drag-handle");
    pointerCard = handle?.closest(".ordering-card") || null;
    if (!pointerCard) return;
    pointerCard.classList.add("dragging");
    handle.setPointerCapture(event.pointerId);
    event.preventDefault();
  });

  document.addEventListener("pointermove", (event) => {
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

  document.addEventListener("pointerup", finishPointerOrdering);
  document.addEventListener("pointercancel", finishPointerOrdering);
  document.addEventListener("lostpointercapture", finishPointerOrdering);

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
  });
})();

(() => {
  const previousValues = new Map();

  function rememberBars() {
    previousValues.clear();
    document.querySelectorAll("[data-live-bar]").forEach((bar) => {
      previousValues.set(bar.dataset.liveBar, bar.dataset.barValue || "0");
    });
  }

  function animateBars() {
    document.querySelectorAll("[data-live-bar]").forEach((bar) => {
      const target = bar.dataset.barValue || "0";
      const previous = previousValues.get(bar.dataset.liveBar) || "0";
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

  document.addEventListener("DOMContentLoaded", () => {
    animateBars();
    followPresenter();
  });
  document.addEventListener("submit", (event) => {
    const message = event.target.dataset.confirm;
    if (message && !window.confirm(message)) event.preventDefault();
  });
  document.addEventListener("htmx:before:swap", rememberBars);
  document.addEventListener("htmx:after:swap", () => {
    animateBars();
    followPresenter();
  });
})();

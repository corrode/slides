(() => {
  const previousValues = new Map();

  function rememberBars() {
    document.querySelectorAll("[data-live-bar]").forEach((bar) => {
      previousValues.set(bar.dataset.liveBar, bar.dataset.barValue || "0");
    });
  }

  function animateBars() {
    document.querySelectorAll("[data-live-bar]").forEach((bar) => {
      const target = bar.dataset.barValue || "0";
      const previous = previousValues.get(bar.dataset.liveBar) || "0";
      bar.style.setProperty("--value", `${previous}%`);
      requestAnimationFrame(() => {
        requestAnimationFrame(() => bar.style.setProperty("--value", `${target}%`));
      });
    });
  }

  document.addEventListener("DOMContentLoaded", animateBars);
  document.addEventListener("submit", (event) => {
    const message = event.target.dataset.confirm;
    if (message && !window.confirm(message)) event.preventDefault();
  });
  document.addEventListener("htmx:before:swap", rememberBars);
  document.addEventListener("htmx:after:swap", animateBars);
})();

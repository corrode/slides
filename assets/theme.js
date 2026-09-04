(() => {
  const storageKey = "slides-color-scheme";
  let saved = null;
  try {
    saved = window.localStorage.getItem(storageKey);
  } catch {
    // System preference remains available when storage is blocked.
  }
  const colorScheme =
    saved === "light" || saved === "dark"
      ? saved
      : window.matchMedia?.("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light";
  document.documentElement.dataset.colorScheme = colorScheme;
})();

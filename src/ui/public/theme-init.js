/* Early theme bootstrapping to avoid FOUC:
   Set data-theme="light|dark" before CSS loads based on user preference.

   This is a file of its own (and not an inline script of index.html) because the
   content security policy of the release build allows no inline scripts, see
   src/platform/csp.rs. It is a classic script in <head>, so it still runs before
   the first paint. */
(function () {
  try {
    var key = "cfc-theme";
    var stored = localStorage.getItem(key);
    var t =
      stored === "light" || stored === "dark"
        ? stored
        : window.matchMedia &&
            window.matchMedia("(prefers-color-scheme: dark)").matches
          ? "dark"
          : "light";
    document.documentElement.setAttribute("data-theme", t);
  } catch {
    // In case storage is blocked, default to dark to match existing visuals
    document.documentElement.setAttribute("data-theme", "dark");
  }
})();

// Switches of the remotes page: the key codes (the choice stays in this browser) and the remove
// mode, which starts off on every load so it is not left on by accident.
(() => {
  const storageKey = "remotes-show-codes";
  const page = document.getElementById("remotes-page");
  const toggle = document.getElementById("show-codes");
  const removeToggle = document.getElementById("remove-keys");
  let show = false;
  try {
    show = localStorage.getItem(storageKey) === "true";
  } catch {}

  function apply() {
    page.dataset.codes = show ? "shown" : "hidden";
    toggle.checked = show;
  }

  toggle.addEventListener("change", () => {
    show = toggle.checked;
    try {
      localStorage.setItem(storageKey, String(show));
    } catch {}
    apply();
  });
  apply();

  function applyRemoving() {
    page.dataset.removing = removeToggle.checked ? "on" : "off";
  }

  removeToggle.addEventListener("change", applyRemoving);
  // Also when the browser brings the page back from its cache (back button).
  window.addEventListener("pageshow", () => {
    removeToggle.checked = false;
    applyRemoving();
  });
})();

// Shows or hides the key codes on the remotes page; the choice stays in this browser.
(() => {
  const storageKey = "remotes-show-codes";
  const page = document.getElementById("remotes-page");
  const toggle = document.getElementById("show-codes");
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
})();

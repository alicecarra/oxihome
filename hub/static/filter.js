// Shows or hides the items of a list by a data attribute; each list's choice stays in this browser.
(() => {
  for (const group of document.querySelectorAll("[data-filter-items]")) {
    const storageKey = `${group.id}-hidden`;
    const attribute = group.dataset.filterAttribute;
    let hidden = [];
    try {
      hidden = JSON.parse(localStorage.getItem(storageKey)) || [];
    } catch {}
    const buttons = group.querySelectorAll("[data-filter]");

    function apply() {
      const items = [
        ...document.querySelectorAll(`${group.dataset.filterItems} [data-${attribute}]`),
      ];
      const valueOf = (item) => item.getAttribute(`data-${attribute}`);
      for (const button of buttons) {
        const value = button.dataset.filter;
        button.setAttribute("aria-pressed", String(!hidden.includes(value)));
        button.querySelector("[data-count]").textContent = items.filter(
          (item) => valueOf(item) === value,
        ).length;
      }
      for (const item of items) {
        item.hidden = hidden.includes(valueOf(item));
      }
      document.getElementById(group.dataset.filterEmpty).hidden =
        items.length === 0 || items.some((item) => !item.hidden);
    }

    for (const button of buttons) {
      button.addEventListener("click", () => {
        const value = button.dataset.filter;
        hidden = hidden.includes(value)
          ? hidden.filter((other) => other !== value)
          : [...hidden, value];
        try {
          localStorage.setItem(storageKey, JSON.stringify(hidden));
        } catch {}
        apply();
      });
    }
    document.body.addEventListener("htmx:afterSettle", apply);
    apply();
  }
})();

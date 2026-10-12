// Shows or hides the node cards on the home page by presence; the choice stays in this browser.
(() => {
  const storageKey = "node-filter-hidden";
  let hidden = [];
  try {
    hidden = JSON.parse(localStorage.getItem(storageKey)) || [];
  } catch {}
  const buttons = document.querySelectorAll("#node-filter [data-filter]");

  function apply() {
    const cards = [...document.querySelectorAll("#nodes [data-presence]")];
    for (const button of buttons) {
      const presence = button.dataset.filter;
      button.setAttribute("aria-pressed", String(!hidden.includes(presence)));
      button.querySelector("[data-count]").textContent = cards.filter(
        (card) => card.dataset.presence === presence,
      ).length;
    }
    for (const card of cards) {
      card.hidden = hidden.includes(card.dataset.presence);
    }
    document.getElementById("nodes-filtered-out").hidden =
      cards.length === 0 || cards.some((card) => !card.hidden);
  }

  for (const button of buttons) {
    button.addEventListener("click", () => {
      const presence = button.dataset.filter;
      hidden = hidden.includes(presence)
        ? hidden.filter((other) => other !== presence)
        : [...hidden, presence];
      try {
        localStorage.setItem(storageKey, JSON.stringify(hidden));
      } catch {}
      apply();
    });
  }
  document.body.addEventListener("htmx:afterSwap", (event) => {
    if (event.detail.target.id === "nodes") apply();
  });
  apply();
})();

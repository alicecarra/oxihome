// Pauses the live traffic list and keeps it to its last rows.
(() => {
  const list = document.getElementById("traffic-list");
  const pause = document.getElementById("traffic-pause");
  const shown = Number(list.dataset.shown);
  let paused = false;
  let missed = 0;

  pause.addEventListener("click", () => {
    paused = !paused;
    missed = 0;
    pause.setAttribute("aria-pressed", String(paused));
    pause.textContent = paused ? "Resume" : "Pause";
  });
  list.addEventListener("htmx:sseBeforeMessage", (event) => {
    if (!paused) return;
    event.preventDefault();
    missed += 1;
    pause.textContent = `Resume (${missed} missed)`;
  });
  list.addEventListener("htmx:sseMessage", () => {
    while (list.children.length > shown) list.lastElementChild.remove();
  });
})();

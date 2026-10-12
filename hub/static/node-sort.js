// Sorts the node table on `/nodes` by the clicked column; the choice stays in this browser.
(() => {
  const storageKey = "node-sort";
  let sort = null;
  try {
    sort = JSON.parse(localStorage.getItem(storageKey));
  } catch {}

  function compare(first, second) {
    const numeric = !isNaN(first) && !isNaN(second);
    return numeric
      ? Number(first) - Number(second)
      : first.localeCompare(second, undefined, { numeric: true });
  }

  function apply() {
    const body = document.querySelector("#nodes tbody");
    if (!body) return;
    const rows = [...body.rows];
    // The server's order, to fall back on and to break ties.
    rows.forEach((row, index) => (row.dataset.order ??= index));
    for (const header of document.querySelectorAll("#nodes [data-sort-column]")) {
      const active = sort?.column === header.dataset.sortColumn;
      header.setAttribute("aria-sort", active ? sort.direction : "none");
    }
    const sign = sort?.direction === "descending" ? -1 : 1;
    rows.sort((first, second) => {
      if (sort) {
        const a = first.getAttribute(`data-sort-${sort.column}`) ?? "";
        const b = second.getAttribute(`data-sort-${sort.column}`) ?? "";
        // Rows without a value go last either way.
        if (a === "" || b === "") {
          if (a !== b) return a === "" ? 1 : -1;
        } else {
          const order = compare(a, b);
          if (order !== 0) return sign * order;
        }
      }
      return first.dataset.order - second.dataset.order;
    });
    body.append(...rows);
  }

  document.addEventListener("click", (event) => {
    const header = event.target.closest("#nodes [data-sort-column] button")?.parentElement;
    if (!header) return;
    const column = header.dataset.sortColumn;
    // Ascending, then descending, then back to the server's order.
    if (sort?.column !== column) sort = { column, direction: "ascending" };
    else if (sort.direction === "ascending") sort = { column, direction: "descending" };
    else sort = null;
    try {
      localStorage.setItem(storageKey, JSON.stringify(sort));
    } catch {}
    apply();
  });
  document.body.addEventListener("htmx:afterSwap", (event) => {
    if (event.detail.target.id === "nodes") apply();
  });
  apply();
})();

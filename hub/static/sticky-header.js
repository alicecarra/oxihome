// Keeps --header-height at the sticky header's height, so a sticky row can sit right under it.
const header = document.querySelector("body > header");
if (header) {
  const update = () =>
    document.documentElement.style.setProperty("--header-height", `${header.offsetHeight}px`);
  new ResizeObserver(update).observe(header);
  update();
}

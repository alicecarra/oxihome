// Caches nothing: the hub shows live data. Only answers page loads when the hub is unreachable.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("fetch", (event) => {
  if (event.request.mode !== "navigate") {
    return;
  }
  event.respondWith(
    fetch(event.request).catch(
      () =>
        new Response(
          `<!DOCTYPE html><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Oxihome</title>
<body style="margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;flex-direction:column;gap:1rem;background:#0c1015;color:#e8eaed;font-family:system-ui,sans-serif">
<p>The hub is unreachable.</p>
<button onclick="location.reload()" style="padding:.5rem 1rem;border-radius:.375rem;border:1px solid #2a2f36;background:#171b20;color:inherit">Try again</button>`,
          { headers: { "Content-Type": "text/html; charset=utf-8" } },
        ),
    ),
  );
});

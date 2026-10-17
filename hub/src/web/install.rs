//! What lets Android install the interface as an app.

use topcoat::{
    Result,
    router::{
        Body,
        content::Js,
        header::{CACHE_CONTROL, CONTENT_TYPE},
        response::IntoResponse,
        route,
    },
    view::{View, component, view},
};

const MANIFEST: &str = include_str!("../../static/manifest.webmanifest");
const SERVICE_WORKER: &str = include_str!("../../static/service-worker.js");
const REGISTRATION: &str = include_str!("../../static/install.js");
const ICON_192: &[u8] = include_bytes!("../../static/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../../static/icon-512.png");

/// The browser fetches these without the session cookie.
pub const PUBLIC_PATHS: [&str; 5] = [
    "/manifest.webmanifest",
    "/service-worker.js",
    "/static/install.js",
    "/static/icon-192.png",
    "/static/icon-512.png",
];

#[component]
pub async fn install_links() -> Result<impl View> {
    Ok(view! {
        <link rel="manifest" href="/manifest.webmanifest">
        <meta name="theme-color" content="#171b20">
        <script src="/static/install.js"></script>
    })
}

#[route(GET "/manifest.webmanifest")]
pub async fn manifest() -> Result<impl IntoResponse> {
    Ok((
        [(CONTENT_TYPE, "application/manifest+json")],
        Body::from(MANIFEST),
    ))
}

/// Served from the root so that it controls every page.
#[route(GET "/service-worker.js")]
pub async fn service_worker() -> Result<impl IntoResponse> {
    Ok(([(CACHE_CONTROL, "no-cache")], Js(SERVICE_WORKER)))
}

#[route(GET "/static/install.js")]
pub async fn registration() -> Result<Js<&'static str>> {
    Ok(Js(REGISTRATION))
}

#[route(GET "/static/icon-192.png")]
pub async fn icon_192() -> Result<impl IntoResponse> {
    Ok(([(CONTENT_TYPE, "image/png")], Body::from(ICON_192)))
}

#[route(GET "/static/icon-512.png")]
pub async fn icon_512() -> Result<impl IntoResponse> {
    Ok(([(CONTENT_TYPE, "image/png")], Body::from(ICON_512)))
}

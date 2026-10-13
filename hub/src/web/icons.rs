//! Icons drawn as inline SVG. The menu icons are from Tabler (MIT license), staged by `build.rs`.

use topcoat::icon::IconData;
use topcoat::icon::iconify::iconify_icon;
use topcoat::view::svg::ViewBox;

const fn stroked(body: &'static str) -> IconData {
    IconData::unescaped_unchecked(ViewBox::new(0.0, 0.0, 24.0, 24.0), body)
}

/// The logo: a six-toothed gear with a house inside.
pub const LOGO: IconData = stroked(
    r#"<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M14.42 4.27 L15.90 2.14 L18.58 3.69 L17.48 6.04 A8.1 8.1 0 0 1 19.90 10.23 L22.49 10.45 L22.49 13.55 L19.90 13.77 A8.1 8.1 0 0 1 17.48 17.96 L18.58 20.31 L15.90 21.86 L14.42 19.73 A8.1 8.1 0 0 1 9.58 19.73 L8.10 21.86 L5.42 20.31 L6.52 17.96 A8.1 8.1 0 0 1 4.10 13.77 L1.51 13.55 L1.51 10.45 L4.10 10.23 A8.1 8.1 0 0 1 6.52 6.04 L5.42 3.69 L8.10 2.14 L9.58 4.27 A8.1 8.1 0 0 1 14.42 4.27Z"/><path d="M8 11 12 6.8l4 4.2V15a.6.6 0 0 1-.6.6H8.6a.6.6 0 0 1-.6-.6z"/></g>"#,
);

/// The logo as a data URL for the favicon, which cannot use the page's colors.
pub const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23ededed' stroke-width='1.75' stroke-linecap='round' stroke-linejoin='round'%3E%3Cstyle%3E@media (prefers-color-scheme: light) { svg { stroke: %23171717 } }%3C/style%3E%3Cpath d='M14.42 4.27 L15.90 2.14 L18.58 3.69 L17.48 6.04 A8.1 8.1 0 0 1 19.90 10.23 L22.49 10.45 L22.49 13.55 L19.90 13.77 A8.1 8.1 0 0 1 17.48 17.96 L18.58 20.31 L15.90 21.86 L14.42 19.73 A8.1 8.1 0 0 1 9.58 19.73 L8.10 21.86 L5.42 20.31 L6.52 17.96 A8.1 8.1 0 0 1 4.10 13.77 L1.51 13.55 L1.51 10.45 L4.10 10.23 A8.1 8.1 0 0 1 6.52 6.04 L5.42 3.69 L8.10 2.14 L9.58 4.27 A8.1 8.1 0 0 1 14.42 4.27Z'/%3E%3Cpath d='M8 11 12 6.8l4 4.2V15a.6.6 0 0 1-.6.6H8.6a.6.6 0 0 1-.6-.6z'/%3E%3C/svg%3E";

pub const DASHBOARD: IconData = iconify_icon!("tabler:layout-dashboard");
pub const NODES: IconData = iconify_icon!("tabler:cpu-2");
pub const PANELS: IconData = iconify_icon!("tabler:square-plus");
pub const RULES: IconData = iconify_icon!("tabler:sitemap");
pub const TIMELINE: IconData = iconify_icon!("tabler:timeline-event");
pub const REMOTES: IconData = iconify_icon!("tabler:device-remote");
pub const SETTINGS: IconData = iconify_icon!("tabler:adjustments-cog");
pub const EVENT: IconData = iconify_icon!("tabler:bolt");
pub const COMMAND: IconData = iconify_icon!("tabler:send");
pub const NOTIFICATION: IconData = iconify_icon!("tabler:bell");
pub const UPDATE: IconData = iconify_icon!("tabler:cloud-download");
pub const SORT: IconData = iconify_icon!("tabler:selector");
pub const SORT_ASCENDING: IconData = iconify_icon!("tabler:chevron-up");
pub const SORT_DESCENDING: IconData = iconify_icon!("tabler:chevron-down");

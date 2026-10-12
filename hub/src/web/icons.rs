//! Icons drawn as inline SVG. The menu icons are from Tabler (MIT license), staged by `build.rs`.

use topcoat::icon::IconData;
use topcoat::icon::iconify::iconify_icon;
use topcoat::view::svg::ViewBox;

const fn stroked(body: &'static str) -> IconData {
    IconData::unescaped_unchecked(ViewBox::new(0.0, 0.0, 24.0, 24.0), body)
}

/// Temporary logo: a gear with a house inside.
pub const LOGO: IconData = stroked(
    r#"<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M8.86 4.42 L10.16 4.01 L10.36 1.63 L13.64 1.63 L13.84 4.01 L15.14 4.42 L16.35 5.05 L18.17 3.51 L20.49 5.83 L18.95 7.65 L19.58 8.86 L19.99 10.16 L22.37 10.36 L22.37 13.64 L19.99 13.84 L19.58 15.14 L18.95 16.35 L20.49 18.17 L18.17 20.49 L16.35 18.95 L15.14 19.58 L13.84 19.99 L13.64 22.37 L10.36 22.37 L10.16 19.99 L8.86 19.58 L7.65 18.95 L5.83 20.49 L3.51 18.17 L5.05 16.35 L4.42 15.14 L4.01 13.84 L1.63 13.64 L1.63 10.36 L4.01 10.16 L4.42 8.86 L5.05 7.65 L3.51 5.83 L5.83 3.51 L7.65 5.05Z"/><path d="M7.8 12.2 12 8.4l4.2 3.8V16a.6.6 0 0 1-.6.6H8.4a.6.6 0 0 1-.6-.6z" stroke-width="1.75"/></g>"#,
);

/// The logo as a data URL for the favicon, which cannot use the page's colors.
pub const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23171717' stroke-width='1.75' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M8.86 4.42 L10.16 4.01 L10.36 1.63 L13.64 1.63 L13.84 4.01 L15.14 4.42 L16.35 5.05 L18.17 3.51 L20.49 5.83 L18.95 7.65 L19.58 8.86 L19.99 10.16 L22.37 10.36 L22.37 13.64 L19.99 13.84 L19.58 15.14 L18.95 16.35 L20.49 18.17 L18.17 20.49 L16.35 18.95 L15.14 19.58 L13.84 19.99 L13.64 22.37 L10.36 22.37 L10.16 19.99 L8.86 19.58 L7.65 18.95 L5.83 20.49 L3.51 18.17 L5.05 16.35 L4.42 15.14 L4.01 13.84 L1.63 13.64 L1.63 10.36 L4.01 10.16 L4.42 8.86 L5.05 7.65 L3.51 5.83 L5.83 3.51 L7.65 5.05Z'/%3E%3Cpath d='M7.8 12.2 12 8.4l4.2 3.8V16a.6.6 0 0 1-.6.6H8.4a.6.6 0 0 1-.6-.6z' stroke-width='1.75'/%3E%3C/svg%3E";

pub const DASHBOARD: IconData = iconify_icon!("tabler:layout-dashboard");
pub const NODES: IconData = iconify_icon!("tabler:cpu-2");
pub const PANEL: IconData = iconify_icon!("tabler:toggle-right");
pub const RULES: IconData = iconify_icon!("tabler:sitemap");
pub const REMOTES: IconData = iconify_icon!("tabler:device-remote");
pub const SETTINGS: IconData = iconify_icon!("tabler:adjustments-cog");

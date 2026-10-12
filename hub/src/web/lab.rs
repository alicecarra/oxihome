//! `/lab`: a page left out of the menu for trying out components and styles side by side.

use topcoat::{
    Result,
    router::page,
    view::{View, view},
};

use super::layout::layout;

#[page("/lab")]
pub async fn lab() -> Result<impl View> {
    Ok(view! {
        layout(
            title: "Oxihome lab",
            <div class="space-y-8">
                <div>
                    <h1 class="text-2xl font-semibold">"Lab"</h1>
                    <p class="text-sm text-muted-foreground">"A page outside the menu for trying things out."</p>
                </div>
            </div>
        )
    })
}

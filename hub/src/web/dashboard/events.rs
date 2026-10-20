use topcoat::{
    Result,
    view::{View, component, view},
};

use crate::web::format::ago_label;
use crate::web::overview::Activity;

#[component]
pub(super) async fn events_block(items: Vec<Activity>) -> Result<impl View> {
    Ok(view! {
        if items.is_empty() {
            <p class="text-sm text-muted-foreground">"Nothing yet."</p>
        }
        <ul class="divide-y divide-border text-sm">
            for item in items {
                <li class="flex justify-between gap-3 py-2 first:pt-0 last:pb-0">
                    <div class="min-w-0">
                        match item.link {
                            Some(link) => <a href=(link) class="font-medium hover:underline">(item.title)</a>,
                            None => <p class="font-medium">(item.title)</p>,
                        }
                        <p class="truncate text-muted-foreground">(item.detail)</p>
                    </div>
                    <span class="shrink-0 text-xs text-muted-foreground">(ago_label(item.ago))</span>
                </li>
            }
        </ul>
    })
}

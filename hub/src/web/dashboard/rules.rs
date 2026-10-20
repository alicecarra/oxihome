use topcoat::{
    Result,
    view::{View, component, view},
};

use crate::web::format::{ago_label, rule_anchor};
use crate::web::overview::RulesSummary;

#[component]
pub(super) async fn rules_block(summary: RulesSummary) -> Result<impl View> {
    Ok(view! {
        <div class="space-y-4 text-sm">
            <div class="space-y-2">
                <p class="font-medium">"Next"</p>
                if summary.upcoming.is_empty() {
                    <p class="text-muted-foreground">"No scheduled rules."</p>
                }
                <ul class="space-y-1">
                    for (name, when) in summary.upcoming {
                        <li class="flex justify-between gap-3">
                            <a href=(format!("/rules#{}", rule_anchor(&name))) class="hover:underline">(name)</a>
                            <span class="shrink-0 text-muted-foreground tabular-nums">(when)</span>
                        </li>
                    }
                </ul>
            </div>
            if !summary.failed.is_empty() {
                <div class="space-y-2">
                    <p class="font-medium text-destructive">"Failed"</p>
                    <ul class="space-y-2">
                        for (name, ago, errors) in summary.failed {
                            <li>
                                <div class="flex justify-between gap-3">
                                    <a href=(format!("/rules#{}", rule_anchor(&name))) class="hover:underline">(name)</a>
                                    <span class="shrink-0 text-muted-foreground">(ago_label(ago))</span>
                                </div>
                                <p class="text-xs text-destructive">(errors.join("; "))</p>
                            </li>
                        }
                    </ul>
                </div>
            }
        </div>
    })
}

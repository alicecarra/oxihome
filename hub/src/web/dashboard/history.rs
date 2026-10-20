use jiff::{SignedDuration, Timestamp};
use topcoat::{
    Result,
    view::{View, attributes, component, view},
};

use super::value::current_value;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::config::dashboard::ChannelReference;
use crate::hub::Hub;
use crate::web::chart::{CHART_BUCKETS, Chart, ChartRange, chart, history_chart, series};
use crate::web::overview::DashboardChannel;

pub(super) async fn history(
    hub: &Hub,
    reference: Option<&ChannelReference>,
    range: ChartRange,
) -> Option<Chart> {
    let reference = reference?;
    let since = Timestamp::now() - SignedDuration::try_from(range.span()).ok()?;
    match hub
        .history(
            &reference.device,
            reference.entity,
            reference.channel,
            since,
            range.bucket_length(),
        )
        .await
    {
        Ok(history) => chart(&series(&history, CHART_BUCKETS)),
        Err(error) => {
            println!("dashboard: history of {}: {error:#}", reference.device);
            None
        }
    }
}

#[component]
pub(super) async fn history_block(
    channel: DashboardChannel,
    chart: Option<Chart>,
    range: ChartRange,
    choices: Vec<(ChartRange, String)>,
) -> Result<impl View> {
    let value = current_value(&channel);
    let unit = channel
        .channel
        .as_ref()
        .ok()
        .and_then(|overview| overview.unit);
    Ok(view! {
        <div class="space-y-3">
            <div class="flex items-baseline justify-between gap-3">
                <p class="text-sm text-muted-foreground">(channel.label)</p>
                <p class="text-xl font-semibold tabular-nums">(value)</p>
            </div>
            <div class="inline-flex gap-0.5 rounded-lg border border-border p-0.5" role="group" aria-label="Chart range">
                for (choice, query) in choices {
                    button(
                        variant: if choice == range { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                        size: ButtonSize::Sm,
                        attrs: attributes! {
                            type="button"
                            aria-pressed=(if choice == range { "true" } else { "false" })
                            hx-get=(format!("/dashboard/fragment?{query}"))
                            hx-target="#dashboard"
                            hx-swap="outerHTML"
                            hx-push-url=(format!("/?{query}"))
                        },
                        (choice.label())
                    )
                }
            </div>
            match chart {
                Some(chart) => history_chart(chart: chart, unit: unit, range: range),
                None => <p class="text-sm text-muted-foreground">(format!("No readings in the last {}.", range.label()))</p>,
            }
        </div>
    })
}

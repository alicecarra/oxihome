use oxihome_protocol::{Unit, Value};
use topcoat::{
    Result,
    view::{View, component, view},
};

use crate::web::format::{bool_label, value_label};
use crate::web::overview::DashboardChannel;

#[component]
pub(super) async fn value_block(channel: DashboardChannel, online: bool) -> Result<impl View> {
    let value = current_value(&channel);
    let percent = percent(&channel);
    Ok(view! {
        <div class=(if online { "space-y-1" } else { "space-y-1 opacity-60" })>
            <p class="text-4xl font-semibold tracking-tight tabular-nums">(value)</p>
            if let Some(percent) = percent {
                percent_bar(percent: percent)
            }
            <p class="text-sm text-muted-foreground">
                (channel.label)
                if !online {
                    " · offline"
                }
            </p>
        </div>
    })
}

/// The channel's current value as text, or why there is none.
pub(super) fn current_value(channel: &DashboardChannel) -> String {
    match &channel.channel {
        Ok(overview) => match &overview.value {
            Some(Value::Bool(on)) => bool_label(*on, overview.switchable).to_string(),
            Some(value) => value_label(value, overview.unit),
            None => "—".to_string(),
        },
        Err(reason) => (*reason).to_string(),
    }
}

/// From 0 to 100, for a number in percent.
pub(super) fn percent(channel: &DashboardChannel) -> Option<f64> {
    let overview = channel.channel.as_ref().ok()?;
    if overview.unit != Some(Unit::Percent) {
        return None;
    }
    match overview.value.as_ref()? {
        Value::Int(number) => Some(f64::from(*number).clamp(0.0, 100.0)),
        Value::Float(number) => Some(f64::from(*number).clamp(0.0, 100.0)),
        _ => None,
    }
}

#[component]
pub(super) async fn percent_bar(percent: f64) -> Result<impl View> {
    Ok(view! {
        <div class="h-1.5 w-full max-w-48 overflow-hidden rounded-full bg-muted-foreground/20" role="presentation">
            <div class="h-full rounded-full bg-foreground/70" style=(format!("width: {percent:.0}%"))></div>
        </div>
    })
}

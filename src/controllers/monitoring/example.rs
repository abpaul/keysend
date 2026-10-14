use std::collections::BTreeMap;

use loco_rs::prelude::*;
use serde_json::{json, Value};

use super::{chart_pages, hero, open_recommendations};
use crate::{
    language::Text,
    monitoring::{
        campaign_text::Campaigns, charts::TrendMetric, deep_view, example, explain, metrics,
        metrics::Window, tracking, view,
    },
    workspace::history::day_number,
};

pub fn view(text: &Text) -> Result<Value> {
    let campaigns = example::campaigns();
    let names = campaigns
        .iter()
        .map(|campaign| (campaign.id, campaign.name.clone()))
        .collect::<BTreeMap<_, _>>();
    let days = example::days();
    let first_day = days
        .first()
        .map(|day| day.day)
        .ok_or_else(|| Error::Message("The example has no Account history".into()))?;
    let end = metrics::last_shown_day(day_number(example::END), example::today());
    let (_, judged) = tracking::of_days(&days, end, Some(example::CONVERSION_ACTIONS), None);
    let window = Window::SevenDays;
    let source = chart_pages::Source {
        text,
        id: example::AD,
        days: &days,
        from: first_day,
        end,
        first_day,
        judged,
        currency: example::CURRENCY,
        window,
    };
    let unlinked = Campaigns::new(
        campaigns
            .iter()
            .map(|campaign| (campaign.id, campaign.name.as_str())),
        None,
        |_| String::new(),
    );
    let metric = TrendMetric::chosen(None, judged);
    let shown = view::metrics(
        text,
        &metrics::window_metrics(&days, end, window, None, judged),
        example::CURRENCY,
    )?;
    let changes = source.changes(
        &source.readable_changes(&example::changes(), &names, None)?,
        unlinked.translations(),
    )?;
    let search_share = source.search_share_of(
        &source.search_campaigns(&campaigns, None),
        &example::search_share_totals(end - window.days() + 1, end),
        |_| None,
    )?;
    let audits = example::audits();
    let audit = audits
        .get(&example::EMERGENCY)
        .ok_or_else(|| Error::Message("The example has no Deep check of its Campaign".into()))?;
    Ok(json!({
        "example": true,
        "update": hero(
            text,
            &example::update(&days, 0),
            example::today(),
            &names,
            &unlinked,
        )?,
        "window": chart_pages::window_view(text, "", window, end, metric)?,
        "metric_links": vec![Value::Null; shown.len()],
        "metrics": shown,
        "metric_sparklines": source.metric_sparklines(None)?,
        "explain": explain::charts(text, judged),
        "trend": source.trend(None, metric, &changes)?,
        "campaign_shares": source.campaign_shares(&campaigns, |_| String::new())?,
        "search_share": search_share,
        "recommendations": open_recommendations(example::recommendations(), None, &unlinked),
        "search_terms": deep_view::search_terms_section(
            text,
            audit,
            example::EMERGENCY,
            example::CURRENCY,
            judged,
        )?,
    }))
}

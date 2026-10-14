use loco_rs::{Error, Result};
use serde::{Deserialize, Serialize};

use super::{
    campaign_settings::Rules,
    campaign_text::Campaigns,
    detection::{mean_deviation, stopped_on, weekday_values, MIN_NORMAL_DAYS},
    email::{self, Message},
    format,
    metrics::{
        self, by_campaign, first_active_day, last_settled_conversion_day, sum, CampaignDay,
        Metrics, CONVERSION_MIN_AGE_DAYS,
    },
};
use crate::{
    language::{say, say_plural, Language},
    workspace::history::{date_of, Campaign},
};

pub const SPEND_MULTIPLIER: f64 = 2.0;
pub const NORMAL_CONVERSIONS: f64 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKey {
    CampaignStopped(i64),
    SpendDoubled,
    ConversionsZero,
}

impl std::fmt::Display for EventKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CampaignStopped(id) => write!(f, "campaign-stopped:{id}"),
            Self::SpendDoubled => f.write_str("spend-doubled"),
            Self::ConversionsZero => f.write_str("conversions-zero"),
        }
    }
}

impl std::str::FromStr for EventKey {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, String> {
        match text {
            "spend-doubled" => Ok(Self::SpendDoubled),
            "conversions-zero" => Ok(Self::ConversionsZero),
            _ => text
                .strip_prefix("campaign-stopped:")
                .and_then(|id| id.parse().ok())
                .map(Self::CampaignStopped)
                .ok_or_else(|| format!("Unknown Alert event {text}")),
        }
    }
}

impl Serialize for EventKey {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for EventKey {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Alert {
    pub day: i64,
    pub event_key: EventKey,
    pub campaign_id: Option<i64>,
    pub name: String,
    pub metrics: Metrics,
    pub normal_value: f64,
    pub body: String,
}

pub fn detect(
    days: &[CampaignDay],
    campaigns: &[Campaign],
    end: i64,
    first: i64,
    rules: impl Rules,
    name: &str,
    currency: &str,
) -> Vec<Alert> {
    let judge = rules.judged();
    let first = first_active_day(days).unwrap_or(end).max(first);
    let data_days = days
        .iter()
        .filter(|row| row.day >= first && row.day < end && row.metrics.serving())
        .map(|row| row.day)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let day = end;
    let date = crate::workspace::history::date_of(day)
        .map(|date| date.to_string())
        .unwrap_or_default();
    let metrics = sum(days, end, end, None);
    let mut alerts = Vec::new();
    let grouped = by_campaign(days);
    for campaign in campaigns {
        let own_days = grouped
            .get(&campaign.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let current = sum(own_days, end, end, None);
        if stopped_on(own_days, end, first, None) {
            alerts.push(Alert {
                day,
                event_key: EventKey::CampaignStopped(campaign.id),
                campaign_id: Some(campaign.id),
                name: campaign.name.clone(),
                metrics: if rules.campaign_judged(campaign.id) {
                    current
                } else {
                    current.without_conversions()
                },
                normal_value: 0.0,
                body: format!("{name}: {} stopped serving on {date}.", campaign.name),
            });
        }
    }
    let spend = weekday_values(days, end, first, None, |metrics| metrics.spend);
    let (normal, _) = mean_deviation(&spend);
    if data_days >= 14
        && spend.len() >= MIN_NORMAL_DAYS
        && normal > 0.0
        && metrics.spend >= normal * SPEND_MULTIPLIER
        && metrics.spend - normal >= normal.max(10.0)
    {
        alerts.push(Alert {
            day,
            event_key: EventKey::SpendDoubled,
            campaign_id: None,
            name: name.into(),
            metrics: if judge {
                metrics.clone()
            } else {
                metrics.clone().without_conversions()
            },
            normal_value: normal,
            body: format!(
                "{name}: spend on {date} was {}, at least twice its normal matching weekday of {}.",
                metrics::english_money(metrics.spend, currency),
                metrics::english_money(normal, currency)
            ),
        });
    }
    if judge {
        let settled = last_settled_conversion_day(end);
        let conversions = weekday_values(days, settled, first, None, |metrics| {
            metrics.conversions.unwrap_or_default()
        });
        let (normal, _) = mean_deviation(&conversions);
        let settled_metrics = sum(days, settled, settled, None);
        if sum(days, settled - 1, settled - 1, None)
            .conversions
            .unwrap_or_default()
            == 0.0
            && conversions.len() >= MIN_NORMAL_DAYS
            && conversions.iter().filter(|value| **value > 0.0).count() >= MIN_NORMAL_DAYS
            && normal >= NORMAL_CONVERSIONS
            && settled_metrics.conversions.unwrap_or_default() == 0.0
        {
            let settled_date = crate::workspace::history::date_of(settled)
                .map(|date| date.to_string())
                .unwrap_or_default();
            alerts.push(Alert {day:settled,event_key:EventKey::ConversionsZero,campaign_id:None,name:name.into(),metrics:settled_metrics,normal_value:normal,body:format!("{name}: conversions were zero on {settled_date} and the day before; its normal matching weekday has {normal:.2} conversions.")});
        }
    }
    alerts
}

pub fn budget_raised(changes: &[crate::workspace::history::AccountChange], end: i64) -> bool {
    changes.iter().any(|change| {
        let date = change
            .changed_at
            .get(..10)
            .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            .map(crate::workspace::history::day_number);
        if !date.is_some_and(|day| (end - 6..=end).contains(&day)) {
            return false;
        }
        let amount = |text: &str| {
            serde_json::from_str::<serde_json::Value>(text)
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/campaignBudget/amountMicros")
                        .and_then(|amount| {
                            amount.as_i64().or_else(|| amount.as_str()?.parse().ok())
                        })
                })
        };
        amount(&change.old_value)
            .zip(amount(&change.new_value))
            .is_some_and(|(old, new)| new > old)
    })
}

#[derive(Serialize)]
struct Fact {
    label: &'static str,
    value: String,
}

pub fn message(
    language: Language,
    alert: &Alert,
    ad_account: &str,
    account: &str,
    currency: &str,
    origin: &str,
) -> Result<Message> {
    let text = language.text()?;
    let written = &text.written;
    let date = date_of(alert.day).ok_or_else(|| Error::Message("Invalid Alert day".into()))?;
    let name = email::display_name(&alert.name);
    let account = email::display_name(account);
    let long = format::long_date(text, date);
    let weekday = text.written.format.weekday_name(date);
    let normal = |value: f64| text.money(value, currency);
    let (headline, lead, action) = match alert.event_key {
        EventKey::CampaignStopped(_) => (
            say!(text, alert_stopped_headline, name = &name)?,
            say!(text, alert_stopped_lead, name = &name, date = &long)?,
            written.alert_stopped_action,
        ),
        EventKey::SpendDoubled => (
            written.alert_spend_headline.to_owned(),
            say!(
                text,
                alert_spend_lead,
                date = &long,
                spend = normal(alert.metrics.spend),
                normal = normal(alert.normal_value),
                weekday = weekday
            )?,
            written.alert_spend_action,
        ),
        EventKey::ConversionsZero => (
            written.alert_conversions_headline.to_owned(),
            say_plural!(
                text,
                alert_conversions_lead,
                u64::try_from(CONVERSION_MIN_AGE_DAYS).unwrap_or_default(),
                date = &long,
                weekday = weekday,
                normal = text.count(alert.normal_value)
            )?,
            written.alert_conversions_action,
        ),
    };
    let named = Campaigns::unlinked(
        matches!(alert.event_key, EventKey::CampaignStopped(_)).then_some(name.as_str()),
    );
    let mut facts = vec![
        Fact {
            label: written.metric_spend,
            value: normal(alert.metrics.spend),
        },
        Fact {
            label: written.metric_clicks,
            value: text.count(alert.metrics.clicks),
        },
        Fact {
            label: written.metric_impressions,
            value: text.count(alert.metrics.impressions),
        },
    ];
    if let Some(conversions) = alert.metrics.conversions {
        facts.push(Fact {
            label: written.metric_conversions,
            value: text.count(conversions),
        });
    }
    let facts_line = say!(
        text,
        alert_facts,
        date = &long,
        facts = facts
            .iter()
            .map(|fact| say!(text, alert_fact, label = fact.label, value = &fact.value))
            .collect::<Result<Vec<_>>>()?
            .join(", ")
    )?;
    let link = email::link(origin, ad_account);
    let subject = say!(
        text,
        alert_subject,
        headline = &headline,
        account = &account
    )?;
    let html = email::render(
        language,
        "email/alert.html",
        &serde_json::json!({
            "title": subject, "preheader": lead, "account": account, "date": long,
            "headline_parts": named.split(&headline, None),
            "lead_parts": named.split(&lead, None), "facts_line": facts_line, "action": action,
            "link": link, "footer": written.email_footer, "note": written.alert_note,
        }),
    )?;
    let body = format!(
        "{}\n{headline}\n\n{lead}\n\n{}\n\n{}\n\n{}\n\n---\n{}\n{}",
        say!(text, alert_title, account = &account, date = &long)?,
        facts_line,
        say!(text, alert_what_to_do, action = action)?,
        say!(text, alert_open, account = &account, link = &link)?,
        written.email_footer,
        written.alert_note,
    );
    Ok(Message {
        subject,
        html,
        text: body,
    })
}

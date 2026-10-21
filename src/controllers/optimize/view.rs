//! The words of Optimize pages: each Proposal as a sentence of its change, and its reasons in
//! the order the User reads them: what KeySend measured, what it might mean, what to expect,
//! what can go wrong, and how KeySend will look at the result.

use loco_rs::{Error, Result};
use serde_json::{json, Value};

use crate::{
    language::{fill, Entry, Text},
    optimize::{
        policy::Refusal,
        propose::{Expected, Hold, Reason, RiskNote},
        signals::{Caveat, Kind, Signal},
        Action, ActionKind, Draft, ProposalState, Risk,
    },
    workspace::{history::date_of, optimization::Event},
};

pub(super) fn say(text: &'static Text, key: &str, values: &[(&str, String)]) -> Result<String> {
    match text.entry(key) {
        Some(Entry::Line(line)) => fill(key, line, values),
        _ => Err(Error::Message(format!("Text `{key}` is not a line"))),
    }
}

pub(super) fn date(text: &Text, day: i64) -> String {
    date_of(day).map_or_else(String::new, |date| text.written.format.date_with_year(date))
}

pub(super) fn timestamp(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|at| at.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

fn campaign_name(signal: &Signal, action: &Action) -> String {
    signal
        .campaign
        .clone()
        .unwrap_or_else(|| action.campaign_id().to_string())
}

/// The change as one sentence: "Cut the daily budget of X from A to B".
pub(super) fn headline(text: &'static Text, draft: &Draft, currency: &str) -> Result<String> {
    let campaign = campaign_name(&draft.signal, &draft.action);
    match &draft.action {
        Action::AdjustCampaignBudget {
            from_micros,
            to_micros,
            ..
        } => say(
            text,
            if to_micros > from_micros {
                "opt_change_raise"
            } else {
                "opt_change_cut"
            },
            &[
                ("campaign", campaign),
                ("from", text.money(*from_micros as f64 / 1e6, currency)),
                ("to", text.money(*to_micros as f64 / 1e6, currency)),
            ],
        ),
        Action::PauseCampaign { .. } => say(text, "opt_change_pause", &[("campaign", campaign)]),
        Action::EnableCampaign { .. } => say(text, "opt_change_enable", &[("campaign", campaign)]),
        Action::AddNegativeKeyword { text: term, .. } => say(
            text,
            "opt_change_negative",
            &[("term", term.clone()), ("campaign", campaign)],
        ),
        _ => say(text, "opt_change_other", &[("campaign", campaign)]),
    }
}

/// The current and the proposed value, as the User compares them.
pub(super) fn values(
    text: &'static Text,
    action: &Action,
    currency: &str,
) -> Result<(String, String)> {
    Ok(match action {
        Action::AdjustCampaignBudget {
            from_micros,
            to_micros,
            ..
        } => {
            let per_day = |micros: i64| {
                say(
                    text,
                    "opt_per_day",
                    &[("amount", text.money(micros as f64 / 1e6, currency))],
                )
            };
            (per_day(*from_micros)?, per_day(*to_micros)?)
        }
        Action::PauseCampaign { .. } => (
            say(text, "opt_status_enabled", &[])?,
            say(text, "opt_status_paused", &[])?,
        ),
        Action::EnableCampaign { .. } => (
            say(text, "opt_status_paused", &[])?,
            say(text, "opt_status_enabled", &[])?,
        ),
        Action::AddNegativeKeyword { text: term, .. } => (
            say(text, "opt_term_allowed", &[("term", term.clone())])?,
            say(text, "opt_term_excluded", &[("term", term.clone())])?,
        ),
        _ => (String::new(), String::new()),
    })
}

pub(super) fn fact(text: &'static Text, signal: &Signal, currency: &str) -> Result<Vec<String>> {
    let money = |value: f64| text.money(value, currency);
    let range = date_range(text, signal.from_day, signal.to_day);
    let target = signal.target.map(money).unwrap_or_default();
    let mut facts = Vec::new();
    match signal.kind {
        Kind::CostAboveTarget | Kind::BudgetLimited => {
            let cost = if signal.conversions > 0.0 {
                money(signal.spend / signal.conversions)
            } else {
                String::new()
            };
            facts.push(say(
                text,
                "opt_fact_cost",
                &[
                    ("days", range),
                    ("spend", money(signal.spend)),
                    ("conversions", text.count(signal.conversions)),
                    ("cost", cost),
                    ("target", target),
                ],
            )?);
        }
        Kind::SpendWithoutConversions => facts.push(say(
            text,
            "opt_fact_no_conversions",
            &[
                ("days", range),
                ("spend", money(signal.spend)),
                ("clicks", text.count(signal.clicks)),
                ("target", target),
            ],
        )?),
        Kind::SearchTermWithoutConversions => facts.push(say(
            text,
            "opt_fact_term",
            &[
                ("term", signal.term.clone().unwrap_or_default()),
                ("spend", money(signal.spend)),
                ("clicks", text.count(signal.clicks)),
                ("target", target),
            ],
        )?),
        Kind::TrackingUnreliable => facts.push(say(text, "opt_fact_tracking", &[])?),
        Kind::ConversionsNotWhole => facts.push(say(
            text,
            "opt_fact_not_whole",
            &[
                ("days", range),
                ("conversions", text.number(signal.conversions, 2)),
            ],
        )?),
    }
    if let Some(lost) = signal.lost_to_budget {
        facts.push(say(
            text,
            "opt_fact_lost",
            &[("percent", text.number(lost * 100.0, 0))],
        )?);
    }
    if let (Some(spend), Some(budget)) = (signal.daily_spend, signal.daily_budget) {
        if signal.kind != Kind::SearchTermWithoutConversions {
            facts.push(say(
                text,
                "opt_fact_pace",
                &[("spend", money(spend)), ("budget", money(budget))],
            )?);
        }
    }
    if let Some(chance) = signal.chance {
        let percent = chance * 100.0;
        facts.push(say(
            text,
            "opt_fact_chance",
            &[(
                "percent",
                if percent < 0.1 {
                    say(text, "opt_less_than_one_in_a_thousand", &[])?
                } else {
                    format!("{}%", text.number(percent, 1))
                },
            )],
        )?);
    }
    Ok(facts)
}

/// The reasons of a Proposal, in groups for the page.
pub(super) fn reasons(text: &'static Text, draft: &Draft, currency: &str) -> Result<Value> {
    let mut facts = Vec::new();
    let mut hypotheses = Vec::new();
    let mut expected = Vec::new();
    let mut risks = Vec::new();
    let mut measure = Vec::new();
    for reason in &draft.reasons {
        match reason {
            Reason::Fact(signal) => facts.extend(fact(text, signal, currency)?),
            Reason::Hypothesis(kind) => hypotheses.push(hypothesis(text, *kind)?),
            Reason::Expected(found) => expected.push(match found {
                Expected::LessSpend { from, to } => say(
                    text,
                    "opt_expect_less",
                    &[("amount", text.money(from - to, currency))],
                )?,
                Expected::MoreSpend { from, to } => say(
                    text,
                    "opt_expect_more",
                    &[("amount", text.money(to - from, currency))],
                )?,
                Expected::NoSpend { daily_spend } => say(
                    text,
                    "opt_expect_none",
                    &[("amount", text.money(*daily_spend, currency))],
                )?,
                Expected::TermExcluded => say(text, "opt_expect_term", &[])?,
            }),
            Reason::Risk(note) => risks.push(say(
                text,
                match note {
                    RiskNote::FewerConversions => "opt_risk_fewer",
                    RiskNote::HigherCost => "opt_risk_higher_cost",
                    RiskNote::BudgetIsNotACeiling => "opt_risk_not_ceiling",
                    RiskNote::UnrecordedValue => "opt_risk_unrecorded",
                },
                &[],
            )?),
            Reason::Caveat(caveat) => risks.push(match caveat {
                Caveat::UnsettledDays { days } => say(
                    text,
                    "opt_caveat_unsettled",
                    &[("count", text.count(*days as f64))],
                )?,
                Caveat::AutomatedBidding { strategy } => say(
                    text,
                    "opt_caveat_bidding",
                    &[("strategy", strategy.replace('_', " ").to_lowercase())],
                )?,
                Caveat::DeepCheckSnapshot { day } => {
                    say(text, "opt_caveat_snapshot", &[("date", date(text, *day))])?
                }
                Caveat::ZeroIsNotProof => say(text, "opt_caveat_zero", &[])?,
            }),
            Reason::Measure { days } => measure.push(say(
                text,
                "opt_measure",
                &[("count", text.count(f64::from(*days)))],
            )?),
        }
    }
    Ok(
        json!({"facts": facts, "hypotheses": hypotheses, "expected": expected, "risks": risks, "measure": measure}),
    )
}

/// A Signal that a Shadow evaluation held, as one sentence.
pub(super) fn held(text: &'static Text, signal: &Signal) -> Result<String> {
    match &signal.campaign {
        Some(campaign) => say(
            text,
            "opt_shadow_hold_headline",
            &[("campaign", campaign.clone())],
        ),
        None => say(text, "opt_shadow_hold_account", &[]),
    }
}

pub(super) fn hypothesis(text: &'static Text, kind: Kind) -> Result<String> {
    say(
        text,
        match kind {
            Kind::CostAboveTarget => "opt_hypothesis_cost",
            Kind::SpendWithoutConversions => "opt_hypothesis_no_conversions",
            Kind::BudgetLimited => "opt_hypothesis_limited",
            Kind::SearchTermWithoutConversions => "opt_hypothesis_term",
            Kind::TrackingUnreliable => "opt_hypothesis_tracking",
            Kind::ConversionsNotWhole => "opt_hypothesis_not_whole",
        },
        &[],
    )
}

pub(super) fn date_range(text: &Text, from: i64, to: i64) -> String {
    date_of(from)
        .zip(date_of(to))
        .map(|(from, to)| text.written.format.date_range(from, to, true))
        .unwrap_or_default()
}

pub(super) fn state(text: &'static Text, state: ProposalState) -> Result<String> {
    say(text, &format!("opt_state_{}", state.name()), &[])
}

pub(super) fn risk(text: &'static Text, risk: Risk) -> Result<String> {
    say(
        text,
        match risk {
            Risk::Medium => "opt_risk_medium",
            Risk::High => "opt_risk_high",
        },
        &[],
    )
}

pub(super) fn hold(text: &'static Text, hold: &str) -> Result<String> {
    if hold == Hold::ConversionsNotWhole.name() {
        return say(text, "opt_hypothesis_not_whole", &[]);
    }
    let known = [
        Hold::Investigate,
        Hold::TrackingUnreliable,
        Hold::StaleData,
        Hold::ShortHistory,
        Hold::NotEligible,
        Hold::Cooldown,
        Hold::Observing,
        Hold::SharedBudget,
        Hold::BudgetUnknown,
        Hold::BudgetNotBinding,
        Hold::BudgetTotalUnknown,
        Hold::Envelope,
        Hold::AlreadyOpen,
        Hold::RecentlyRejected,
        Hold::Queue,
        Hold::Policy,
    ];
    match known.into_iter().find(|known| known.name() == hold) {
        Some(found) => say(text, &format!("opt_hold_{}", found.name()), &[]),
        None => Ok(String::new()),
    }
}

pub(super) fn refusal(text: &'static Text, refusal: &str) -> Result<String> {
    let known = [
        Refusal::Shadow,
        Refusal::Investigate,
        Refusal::Stopped,
        Refusal::Suspended,
        Refusal::NotEligible,
        Refusal::StepTooLarge,
        Refusal::Envelope,
        Refusal::BudgetTotalUnknown,
        Refusal::Economics,
        Refusal::DailyRaise,
        Refusal::DailyChanges,
        Refusal::Cooldown,
        Refusal::Observing,
        Refusal::SharedBudget,
        Refusal::Stale,
        Refusal::Expired,
        Refusal::NotLive,
        Refusal::Fingerprint,
        Refusal::Access,
        Refusal::PolicyChanged,
    ];
    match known.into_iter().find(|known| known.name() == refusal) {
        Some(found) => say(text, &format!("opt_refusal_{}", found.name()), &[]),
        None => say(text, "opt_refusal_other", &[]),
    }
}

/// One line of the history, in words.
pub(super) fn event(text: &'static Text, event: &Event) -> Result<String> {
    let detail = &event.detail;
    Ok(match event.kind.as_str() {
        "proposed" => say(text, "opt_event_proposed", &[])?,
        "approved" => say(text, "opt_event_approved", &[])?,
        "authorized" => say(text, "opt_event_authorized", &[])?,
        "applying" => say(text, "opt_event_applying", &[])?,
        "dispatching" => say(text, "opt_event_dispatching", &[])?,
        "applied" => say(text, "opt_event_applied", &[])?,
        "uncertain" => say(text, "opt_event_uncertain", &[])?,
        "conflict" => say(text, "opt_event_conflict", &[])?,
        "rejected" => {
            let reason = match detail["reason"].as_str().map(str::trim) {
                Some(reason) if !reason.is_empty() => say(
                    text,
                    "opt_reject_reason_given",
                    &[("reason", reason.to_owned())],
                )?,
                _ => say(text, "opt_reject_reason_missing", &[])?,
            };
            format!("{} {reason}", say(text, "opt_event_rejected", &[])?)
        }
        "expired" => say(text, "opt_event_expired", &[])?,
        "superseded" => say(text, "opt_event_superseded", &[])?,
        "failed" => match detail["refusal"].as_str() {
            Some(found) => say(
                text,
                "opt_event_refused",
                &[("reason", refusal(text, found)?)],
            )?,
            None => say(text, "opt_event_failed", &[])?,
        },
        "policy" => say(
            text,
            match detail["standing"].as_str() {
                Some("suspended") => "opt_event_suspended",
                Some("revoked") => "opt_event_revoked",
                _ => "opt_event_policy",
            },
            &[(
                "version",
                detail["version"].as_i64().unwrap_or_default().to_string(),
            )],
        )?,
        "stopped" => say(text, "opt_event_stopped", &[])?,
        "resumed" => say(text, "opt_event_resumed", &[])?,
        "outcome" => say(
            text,
            "opt_event_outcome",
            &[(
                "state",
                say(
                    text,
                    &format!(
                        "opt_outcome_{}",
                        detail["state"].as_str().unwrap_or("inconclusive")
                    ),
                    &[],
                )?,
            )],
        )?,
        _ => String::new(),
    })
}

pub(super) fn kind(text: &'static Text, kind: ActionKind) -> Result<String> {
    say(text, &format!("opt_kind_{}", kind.name()), &[])
}

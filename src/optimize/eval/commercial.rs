//! The independent commercial benchmark: 120 cases that an author wrote from Google Ads
//! guidance and commercial failure modes, without reading the decision code, its thresholds
//! or the other cases. Their expected actions are provisional judgments, not ground truth;
//! the fixture is frozen and this file never changes it.
//!
//! Each case states facts in words. `translations` turns the facts that KeySend's inputs can
//! carry into an Ad Account (daily rows with whole conversions, the profile, budgets, recorded
//! changes) and names the facts that they cannot carry, which stay unknown to the decision.
//! The production decision (`propose::decide`) runs on each one with no model and no store.
//! The benchmark never executes Proposals. A held Signal proposes no mutation. KeySend has no
//! review output, so a case expecting review can at best be held without a mutation.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::super::{
    policy::{Policy, Recent, Standing},
    propose::{self, Budget, Context, Decision, Hold},
    signals, Action, Mode,
};
use crate::{
    agent::{data::AccountData, eval::snapshot},
    monitoring::{
        audit::{Audit, AuditRow, Counts, Data},
        campaign_settings::{AccountDefaults, Resolved},
        deep_check::{self, DeepCheck},
        economics::Economics,
        eval::{self as history, Scenario},
        metrics::{self, CampaignDay, Metrics},
        tracking,
    },
    workspace::{
        history::{AccountChange, Campaign},
        profile::Profile,
    },
};

const FIXTURE: &str = include_str!("../../../tests/fixtures/optimize-commercial-independent.json");
const FIXTURE_SHA256: &str = "19623ad3c73f1790f3d53f59c13191a0f547ed48c467a5e5d5e9960dd8d2da2f";
/// The cost of a click when a case states no clicks.
const CPC: f64 = 2.0;

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Judged>,
}

#[derive(Deserialize)]
struct Judged {
    id: String,
    category: String,
    expected_action: Expected,
    high_risk: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Expected {
    Abstain,
    RecommendReview,
    RecommendBudgetReduction,
    RecommendBudgetIncrease,
    RecommendPause,
}

/// Days of one Campaign, counted back from the last day of data (0): the totals spread
/// evenly so that any run of days holds a whole number of clicks and conversions.
struct Span {
    from: i64,
    to: i64,
    spend: f64,
    clicks: Option<f64>,
    conversions: f64,
    value: f64,
}

pub(crate) struct Line {
    id: i64,
    status: &'static str,
    channel: &'static str,
    bidding: &'static str,
    budget: Option<f64>,
    spans: Vec<Span>,
}

pub(crate) fn campaign(id: i64) -> Line {
    Line {
        id,
        status: "ENABLED",
        channel: "SEARCH",
        bidding: "MAXIMIZE_CONVERSIONS",
        budget: None,
        spans: Vec::new(),
    }
}

impl Line {
    pub(crate) fn span(mut self, from: i64, to: i64, spend: f64, conversions: f64) -> Self {
        self.spans.push(Span {
            from,
            to,
            spend,
            clicks: None,
            conversions,
            value: 0.0,
        });
        self
    }

    /// The latest `days` days, up to the last day of data.
    pub(crate) fn last(self, days: i64, spend: f64, conversions: f64) -> Self {
        self.span(days - 1, 0, spend, conversions)
    }

    /// Days at a daily spend and a cost per conversion, rounded to whole conversions.
    fn cpa(self, from: i64, to: i64, daily: f64, cost: f64) -> Self {
        let spend = daily * (from - to + 1) as f64;
        self.span(from, to, spend, (spend / cost).round())
    }

    fn clicks(mut self, clicks: f64) -> Self {
        self.spans.last_mut().unwrap().clicks = Some(clicks);
        self
    }

    pub(crate) fn value(mut self, value: f64) -> Self {
        self.spans.last_mut().unwrap().value = value;
        self
    }

    fn budget(mut self, daily: f64) -> Self {
        self.budget = Some(daily);
        self
    }

    fn bidding(mut self, strategy: &'static str) -> Self {
        self.bidding = strategy;
        self
    }

    fn channel(mut self, channel: &'static str) -> Self {
        self.channel = channel;
        self
    }
}

pub(crate) struct Account {
    target: Option<f64>,
    economics: Economics,
    lines: Vec<Line>,
    currency: &'static str,
    conversion_actions: u32,
    late: bool,
    missing_days: i64,
    /// Days of Account history before the last day of data.
    history_days: i64,
    changed: Vec<(i64, i64)>,
    shared: Vec<i64>,
    protected: Vec<i64>,
}

pub(crate) fn ad() -> Account {
    Account {
        target: None,
        economics: Economics::default(),
        lines: Vec::new(),
        currency: "USD",
        conversion_actions: 2,
        late: false,
        missing_days: 0,
        history_days: 429,
        changed: Vec::new(),
        shared: Vec::new(),
        protected: Vec::new(),
    }
}

impl Account {
    pub(crate) fn target(mut self, cost: f64) -> Self {
        self.target = Some(cost);
        self
    }

    /// The contribution of one conversion, as the profile's value of a conversion with every
    /// conversion a customer: its break-even cost per conversion.
    pub(crate) fn worth(self, contribution: f64) -> Self {
        self.economics(Some(contribution), Some(1.0), None)
    }

    pub(crate) fn economics(
        mut self,
        customer_value: Option<f64>,
        close_rate: Option<f64>,
        margin: Option<f64>,
    ) -> Self {
        self.economics.customer_value = customer_value;
        self.economics.close_rate = close_rate;
        self.economics.margin = margin;
        self
    }

    fn target_return(mut self, target: f64) -> Self {
        self.economics.target_return = Some(target);
        self
    }

    pub(crate) fn with(mut self, line: Line) -> Self {
        self.lines.push(line);
        self
    }

    /// Most conversions arrive two days or more after the click.
    fn late(mut self) -> Self {
        self.late = true;
        self
    }

    fn missing(mut self, days: i64) -> Self {
        self.missing_days = days;
        self
    }

    fn changed(mut self, campaign: i64, ago: i64) -> Self {
        self.changed.push((campaign, ago));
        self
    }

    fn shared(mut self, campaigns: &[i64]) -> Self {
        self.shared.extend(campaigns);
        self
    }

    fn protect(mut self, campaign: i64) -> Self {
        self.protected.push(campaign);
        self
    }

    fn untracked(mut self) -> Self {
        self.conversion_actions = 0;
        self
    }

    fn currency(mut self, currency: &'static str) -> Self {
        self.currency = currency;
        self
    }

    fn policy(&self) -> Policy {
        Policy {
            mode: Mode::Recommend,
            envelope_micros: Some(10_000_000_000),
            max_daily_increase_micros: 100_000_000,
            protected: self.protected.iter().copied().collect(),
            ..Default::default()
        }
    }
}

/// A whole total spread over `days` so that every run of days holds a whole number.
fn split(total: f64, days: i64, k: i64) -> f64 {
    if total.fract() != 0.0 {
        return total / days as f64;
    }
    let total = total as i64;
    ((k + 1) * total / days - k * total / days) as f64
}

fn rows(line: &Line, end: i64) -> Vec<CampaignDay> {
    let mut rows = Vec::new();
    for span in &line.spans {
        let days = span.from - span.to + 1;
        let clicks = span.clicks.unwrap_or((span.spend / CPC).round());
        for k in 0..days {
            let conversions = split(span.conversions, days, k);
            let value = if span.conversions > 0.0 {
                span.value * conversions / span.conversions
            } else {
                span.value / days as f64
            };
            rows.push(CampaignDay {
                day: end - (span.from - k),
                campaign_id: line.id,
                metrics: Metrics {
                    spend: span.spend / days as f64,
                    clicks: split(clicks, days, k),
                    impressions: split(clicks, days, k) * 20.0,
                    conversions: Some(conversions),
                    conversion_value: Some(value),
                },
            });
        }
    }
    rows
}

fn counts(metrics: &Metrics) -> Counts {
    Counts {
        impressions: metrics.impressions,
        clicks: metrics.clicks,
        conversions: metrics.conversions.unwrap_or_default(),
        cost: metrics.spend,
        value: metrics.conversion_value.unwrap_or_default(),
    }
}

/// The Ad Account as `data::load` would read it: tracking state, findings and Deep check
/// come from the production functions, run on the case's rows.
pub(crate) fn data(account: &Account) -> AccountData {
    let end = history::end();
    let mut days = account
        .lines
        .iter()
        .flat_map(|line| rows(line, end))
        .filter(|day| day.day <= end - account.missing_days)
        .collect::<Vec<_>>();
    days.sort_by_key(|day| (day.day, day.campaign_id));
    let (state, judged) = tracking::of_days(&days, end, Some(account.conversion_actions), None);
    let rules = Resolved::new(
        AccountDefaults {
            state,
            judged,
            target_cost: account.target,
            alerts: true,
        },
        BTreeMap::new(),
    );
    let mut audit = Data {
        currency: account.currency.into(),
        reporting: counts(&metrics::sum(&days, end - 29, end, None)),
        audit: Audit {
            conversion_actions: Some(account.conversion_actions),
            economics: account.economics.clone(),
            ..Default::default()
        },
    };
    let mut by_campaign: BTreeMap<i64, Audit> = BTreeMap::new();
    for line in &account.lines {
        let row = AuditRow {
            name: format!("Campaign {}", line.id),
            id: Some(line.id),
            counts: counts(&metrics::sum(&days, end - 29, end, Some(line.id))),
            ..Default::default()
        };
        audit.audit.campaigns.push(row.clone());
        by_campaign.entry(line.id).or_default().campaigns.push(row);
    }
    deep_check::compare(&mut audit, &mut by_campaign, &days, end);
    let findings = deep_check::detect(&audit, &by_campaign, &rules);
    let skeleton = Scenario {
        business: "Commercial case",
        currency: account.currency,
        goal: "sales",
        conversion_actions: account.conversion_actions,
        ..Default::default()
    };
    let mut data = snapshot(&skeleton, "commercial-0".into());
    data.days = days;
    data.rules = rules;
    data.first_day = Some(end - account.history_days);
    data.conversion_delay = account.late.then_some((100.0, 100.0));
    data.profile = Profile {
        goal: "sales".into(),
        target_cost: account.target,
        note: String::new(),
        economics: account.economics.clone(),
    };
    data.finding_history = findings.iter().cloned().map(|f| (f, true)).collect();
    data.findings = findings.clone();
    data.deep_check = Some(DeepCheck {
        day: end + 1,
        data: audit,
        by_campaign,
        findings,
        judge_by_conversions: judged,
        recommendations_generated: false,
        assessment: None,
        research: None,
    });
    let (last, _) = signals::settled(&data);
    data.campaigns = account
        .lines
        .iter()
        .map(|line| {
            // Unless the case states a budget, the Campaign spends its whole budget.
            let recent = metrics::sum(&data.days, last - 6, last, Some(line.id)).spend / 7.0;
            let fallback = line
                .spans
                .iter()
                .map(|span| span.spend / (span.from - span.to + 1) as f64)
                .fold(0.0, f64::max);
            let budget = line
                .budget
                .unwrap_or(if recent > 0.0 { recent } else { fallback });
            Campaign {
                id: line.id,
                name: format!("Campaign {}", line.id),
                status: line.status.into(),
                channel_type: line.channel.into(),
                bidding_strategy: line.bidding.into(),
                budget_micros: (budget > 0.0).then(|| (budget * 100.0).round() as i64 * 10_000),
            }
        })
        .collect();
    data.changes = account
        .changed
        .iter()
        .map(|(campaign, ago)| AccountChange {
            changed_at: format!(
                "{} 10:00:00",
                crate::workspace::history::date_of(end + 1 - ago).unwrap()
            ),
            resource_name: format!("customers/1/campaigns/{campaign}"),
            resource_type: "CAMPAIGN".into(),
            operation: "UPDATE".into(),
            campaign_id: Some(*campaign),
            changed_fields: "status".into(),
            old_value: String::new(),
            new_value: String::new(),
            change_resource_name: None,
        })
        .collect();
    data
}

pub(crate) fn decide(account: &Account, data: &AccountData) -> Vec<Decision> {
    let policy = account.policy();
    let budgets = data
        .campaigns
        .iter()
        .map(|campaign| {
            let shared = account.shared.contains(&campaign.id);
            (
                campaign.id,
                Budget {
                    budget_id: 1000 + campaign.id,
                    shared,
                    campaigns: if shared {
                        account.shared.len().max(2) as i64
                    } else {
                        1
                    },
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let recent = Recent {
        budgets_micros: data
            .campaigns
            .iter()
            .filter_map(|campaign| campaign.budget_micros)
            .sum(),
        last_change_days: account.changed.iter().copied().collect(),
        ..Default::default()
    };
    propose::decide(
        data,
        &Context {
            policy: &policy,
            version: 1,
            standing: Standing::Active,
            budgets: &budgets,
            recent: &recent,
            open: &BTreeSet::new(),
            rejected: &BTreeSet::new(),
            today: history::end() + 1,
            now: 1_800_000_000,
            currency: account.currency,
        },
    )
}

struct Translation {
    id: &'static str,
    /// How the stated facts became inputs, with every assumed number named.
    inputs: &'static str,
    /// Stated facts that no input of KeySend carries: the decision never sees them.
    unseen: &'static str,
    subject: Vec<i64>,
    account: Account,
}

fn t(
    id: &'static str,
    inputs: &'static str,
    unseen: &'static str,
    account: Account,
) -> Translation {
    Translation {
        id,
        inputs,
        unseen,
        subject: vec![1],
        account,
    }
}

/// What KeySend did for the Campaigns that a case concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Output {
    Cut,
    Pause,
    Raise,
    OtherChange,
    Held(Hold),
    /// No Signal, because no target cost exists to judge against.
    NoTarget,
    /// No Signal: a target exists and no rule fired.
    NoSignal,
}

impl Output {
    fn mutation(self) -> Option<Expected> {
        match self {
            Self::Cut => Some(Expected::RecommendBudgetReduction),
            Self::Pause => Some(Expected::RecommendPause),
            Self::Raise => Some(Expected::RecommendBudgetIncrease),
            Self::OtherChange => Some(Expected::Abstain),
            _ => None,
        }
    }

    fn name(self) -> String {
        match self {
            Self::Cut => "budget cut".into(),
            Self::Pause => "pause".into(),
            Self::Raise => "budget raise".into(),
            Self::OtherChange => "other change".into(),
            Self::Held(hold) => format!("held: {}", hold.name()),
            Self::NoTarget => "no Signal (no target)".into(),
            Self::NoSignal => "no Signal".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    /// The expected budget change or pause was proposed.
    CorrectAction,
    /// A budget change or pause was expected and a different mutation was proposed.
    IncorrectAction,
    /// A budget change or pause was expected and nothing was proposed.
    MissedAction,
    /// No mutation was expected, one was proposed, and the case is high risk.
    UnsafeAction,
    /// No mutation was expected, one was proposed, and the case is not high risk.
    UnnecessaryAction,
    /// Abstention was expected and no mutation was proposed.
    CorrectAbstention,
    /// A review was expected; no mutation, and a held Signal shows the Campaign.
    ReviewHeld,
    /// A review was expected; no mutation and no Signal.
    ReviewSilent,
}

impl Verdict {
    fn name(self) -> &'static str {
        match self {
            Self::CorrectAction => "correct action",
            Self::IncorrectAction => "incorrect action",
            Self::MissedAction => "missed action",
            Self::UnsafeAction => "unsafe action",
            Self::UnnecessaryAction => "unnecessary action",
            Self::CorrectAbstention => "correct abstention",
            Self::ReviewHeld => "review: held without mutation",
            Self::ReviewSilent => "review: silent without mutation",
        }
    }

    fn failure(self) -> bool {
        matches!(
            self,
            Self::IncorrectAction
                | Self::MissedAction
                | Self::UnsafeAction
                | Self::UnnecessaryAction
        )
    }
}

fn output(
    translation: &Translation,
    data: &AccountData,
    decisions: &[Decision],
) -> (Output, Vec<String>) {
    let mut outside = Vec::new();
    let mut proposed = None;
    let mut held = None;
    for decision in decisions {
        match decision {
            Decision::Propose(draft) => {
                let campaign = draft.action.campaign_id();
                let found = match draft.action {
                    Action::AdjustCampaignBudget { .. } if draft.action.may_spend_more() => {
                        Output::Raise
                    }
                    Action::AdjustCampaignBudget { .. } => Output::Cut,
                    Action::PauseCampaign { .. } => Output::Pause,
                    _ => Output::OtherChange,
                };
                if translation.subject.contains(&campaign) {
                    proposed.get_or_insert(found);
                } else {
                    outside.push(format!("{} of Campaign {campaign}", found.name()));
                }
            }
            Decision::Hold(signal, hold) => {
                if signal
                    .campaign_id
                    .is_none_or(|campaign| translation.subject.contains(&campaign))
                {
                    held.get_or_insert(Output::Held(*hold));
                }
            }
        }
    }
    let nothing = if signals::target(data, &translation.account.policy()).is_none() {
        Output::NoTarget
    } else {
        Output::NoSignal
    };
    (proposed.or(held).unwrap_or(nothing), outside)
}

fn verdict(expected: Expected, high_risk: bool, output: Output) -> Verdict {
    match (expected, output.mutation()) {
        (
            Expected::RecommendBudgetReduction
            | Expected::RecommendBudgetIncrease
            | Expected::RecommendPause,
            Some(done),
        ) => {
            if done == expected {
                Verdict::CorrectAction
            } else {
                Verdict::IncorrectAction
            }
        }
        (
            Expected::RecommendBudgetReduction
            | Expected::RecommendBudgetIncrease
            | Expected::RecommendPause,
            None,
        ) => Verdict::MissedAction,
        (_, Some(_)) if high_risk => Verdict::UnsafeAction,
        (_, Some(_)) => Verdict::UnnecessaryAction,
        (Expected::Abstain, None) => Verdict::CorrectAbstention,
        (Expected::RecommendReview, None) if matches!(output, Output::Held(_)) => {
            Verdict::ReviewHeld
        }
        (Expected::RecommendReview, None) => Verdict::ReviewSilent,
    }
}

fn expected_name(expected: Expected) -> &'static str {
    match expected {
        Expected::Abstain => "abstain",
        Expected::RecommendReview => "review",
        Expected::RecommendBudgetReduction => "reduction",
        Expected::RecommendPause => "pause",
        Expected::RecommendBudgetIncrease => "increase",
    }
}

/// The benchmark's report, with the counts that the documentation quotes.
fn report() -> (String, BTreeMap<Verdict, usize>, String) {
    let fixture: Fixture = serde_json::from_str(FIXTURE).unwrap();
    let translations = translations();
    assert_eq!(
        fixture
            .cases
            .iter()
            .map(|case| case.id.as_str())
            .collect::<Vec<_>>(),
        translations.iter().map(|t| t.id).collect::<Vec<_>>(),
        "every fixture case has one translation, in order"
    );
    let mut totals = BTreeMap::<Verdict, usize>::new();
    let mut by_category = BTreeMap::<&str, BTreeMap<Verdict, usize>>::new();
    let mut matrix = BTreeMap::<(Expected, String), usize>::new();
    let mut failures = String::new();
    let mut lines = String::new();
    let mut cases = String::new();
    for (case, translation) in fixture.cases.iter().zip(&translations) {
        let data = data(&translation.account);
        let decisions = decide(&translation.account, &data);
        let (found, outside) = output(translation, &data, &decisions);
        let verdict = verdict(case.expected_action, case.high_risk, found);
        *totals.entry(verdict).or_default() += 1;
        *by_category
            .entry(case.category.as_str())
            .or_default()
            .entry(verdict)
            .or_default() += 1;
        *matrix
            .entry((case.expected_action, found.name()))
            .or_default() += 1;
        let line = format!(
            "{} | {} | {} | {} | {}{}",
            translation.id,
            expected_name(case.expected_action),
            found.name(),
            verdict.name(),
            if case.high_risk {
                "high risk"
            } else {
                "lower risk"
            },
            if outside.is_empty() {
                String::new()
            } else {
                format!(" | outside the case: {}", outside.join(", "))
            }
        );
        let _ = writeln!(lines, "{line}");
        let _ = writeln!(
            cases,
            "{} | {} | {}",
            translation.id,
            found.name(),
            verdict.name()
        );
        if verdict.failure() || !outside.is_empty() {
            let _ = writeln!(
                failures,
                "{line}\n    inputs: {}\n    unseen: {}",
                translation.inputs, translation.unseen
            );
        }
    }
    let mut text = String::new();
    let _ = writeln!(text, "{} cases", fixture.cases.len());
    for (verdict, count) in &totals {
        let _ = writeln!(text, "{}: {count}", verdict.name());
    }
    let _ = writeln!(text, "\nexpected | KeySend output | cases");
    for ((expected, found), count) in &matrix {
        let _ = writeln!(text, "{} | {found} | {count}", expected_name(*expected));
    }
    let _ = writeln!(text, "\ncategory | verdicts");
    for (category, verdicts) in &by_category {
        let verdicts = verdicts
            .iter()
            .map(|(verdict, count)| format!("{} {count}", verdict.name()))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(text, "{category} | {verdicts}");
    }
    let _ = writeln!(text, "\nFailures and actions outside the case:\n{failures}");
    let _ = writeln!(text, "Every case:\n{lines}");
    (text, totals, cases)
}

/// KeySend's output and the verdict of every case, as measured on 2026-10-21 after the
/// fixes that `docs/agent-evaluation.md` describes. A change of the decision changes a line
/// here; the line is updated with the reason, never to raise the score.
const MEASURED: &str = "\
commercial-independent-001-weekly-crm-import | held: tracking_unreliable | correct abstention
commercial-independent-002-long-consideration | held: tracking_unreliable | correct abstention
commercial-independent-003-weekend-appointments | no Signal (no target) | correct abstention
commercial-independent-004-mixed-maturity | no Signal | correct abstention
commercial-independent-005-late-value | no Signal (no target) | correct abstention
commercial-independent-006-settled-loss | budget cut | correct action
commercial-independent-007-unknown-delay | held: tracking_unreliable | review: held without mutation
commercial-independent-008-delay-not-infinite | held: tracking_unreliable | missed action
commercial-independent-009-removed-tag | held: tracking_unreliable | review: held without mutation
commercial-independent-010-double-fire | no Signal (no target) | review: silent without mutation
commercial-independent-011-primary-pageviews | no Signal (no target) | review: silent without mutation
commercial-independent-012-secondary-purchases | held: tracking_unreliable | review: held without mutation
commercial-independent-013-custom-goal-exception | no Signal | review: silent without mutation
commercial-independent-014-device-tag-gap | no Signal (no target) | review: silent without mutation
commercial-independent-015-no-setup | held: tracking_unreliable | review: held without mutation
commercial-independent-016-real-site-failure | no Signal (no target) | missed action
commercial-independent-017-premium-orders | no Signal | correct abstention
commercial-independent-018-qualified-enterprise | no Signal | correct abstention
commercial-independent-019-lifetime-validated | no Signal | correct abstention
commercial-independent-020-lifetime-speculative | budget cut | unsafe action
commercial-independent-021-profit-value | no Signal (no target) | correct abstention
commercial-independent-022-high-revenue-low-margin | budget cut | correct action
commercial-independent-023-customer-mix | no Signal | correct abstention
commercial-independent-024-unknown-value | no Signal (no target) | review: silent without mutation
commercial-independent-025-after-black-friday | no Signal | correct abstention
commercial-independent-026-summer-closure | held: tracking_unreliable | missed action
commercial-independent-027-weather-demand | no Signal | correct abstention
commercial-independent-028-holiday-weekdays | no Signal | correct abstention
commercial-independent-029-sale-calendar-unknown | no Signal (no target) | review: silent without mutation
commercial-independent-030-annual-loss | budget cut | correct action
commercial-independent-031-before-peak | no Signal | correct abstention
commercial-independent-032-ended-ticket-sales | no Signal (no target) | missed action
commercial-independent-033-new-three-days | held: tracking_unreliable | correct abstention
commercial-independent-034-new-high-ticket | held: tracking_unreliable | correct abstention
commercial-independent-035-strategy-transition | no Signal (no target) | correct abstention
commercial-independent-036-composition-change | no Signal (no target) | correct abstention
commercial-independent-037-no-learning-label | no Signal (no target) | correct abstention
commercial-independent-038-learning-budget-breach | held: tracking_unreliable | missed action
commercial-independent-039-new-but-impossible | held: tracking_unreliable | missed action
commercial-independent-040-old-test-label | held: tracking_unreliable | missed action
commercial-independent-041-budget-just-cut | no Signal (no target) | correct abstention
commercial-independent-042-landing-page-fixed | held: tracking_unreliable | correct abstention
commercial-independent-043-multiple-concurrent-edits | no Signal (no target) | review: silent without mutation
commercial-independent-044-user-raised-for-capacity | no Signal | correct abstention
commercial-independent-045-stale-proposed-value | held: cooldown | review: held without mutation
commercial-independent-046-scheduled-experiment | held: tracking_unreliable | correct abstention
commercial-independent-047-manual-cpc-mature-loss | budget cut | correct action
commercial-independent-048-owner-paused-already | no Signal (no target) | correct abstention
commercial-independent-049-profitable-sibling | no Signal | review: silent without mutation
commercial-independent-050-unknown-members | held: shared_budget | review: held without mutation
commercial-independent-051-reallocated-success | no Signal (no target) | correct abstention
commercial-independent-052-wrong-local-budget | no Signal (no target) | review: silent without mutation
commercial-independent-053-bad-member-no-reason | no Signal (no target) | missed action
commercial-independent-054-portfolio-healthy | no Signal | correct abstention
commercial-independent-055-all-members-lose | held: shared_budget | review: held without mutation
commercial-independent-056-brand-protected-member | held: shared_budget | review: held without mutation
commercial-independent-057-brand-not-measured-by-sales | held: tracking_unreliable | correct abstention
commercial-independent-058-brand-high-cpa-unknown-purpose | budget cut | unsafe action
commercial-independent-059-cheap-brand-no-incrementality | no Signal (no target) | review: silent without mutation
commercial-independent-060-brand-controlled-loss | no Signal (no target) | missed action
commercial-independent-061-brand-holdout-profitable | budget cut | unsafe action
commercial-independent-062-upper-funnel-short-window | held: tracking_unreliable | correct abstention
commercial-independent-063-brand-protection | held: not_eligible | review: held without mutation
commercial-independent-064-unpowered-lift | held: tracking_unreliable | correct abstention
commercial-independent-065-fewer-better-leads | no Signal | correct abstention
commercial-independent-066-cheap-spam | no Signal (no target) | missed action
commercial-independent-067-quality-not-sampled | no Signal (no target) | review: silent without mutation
commercial-independent-068-sales-followup-failed | no Signal (no target) | review: silent without mutation
commercial-independent-069-call-duration-proxy | no Signal (no target) | review: silent without mutation
commercial-independent-070-capacity-full | no Signal (no target) | missed action
commercial-independent-071-long-cycle-mature | budget cut | correct action
commercial-independent-072-qualification-unknown | budget cut | unsafe action
commercial-independent-073-returns-reverse-win | no Signal (no target) | missed action
commercial-independent-074-cancelled-whale | no Signal (no target) | review: silent without mutation
commercial-independent-075-gross-margin-not-contribution | no Signal (no target) | review: silent without mutation
commercial-independent-076-profitable-mix | no Signal | correct abstention
commercial-independent-077-currency-mismatch | no Signal (no target) | review: silent without mutation
commercial-independent-078-out-of-stock-all | no Signal (no target) | missed action
commercial-independent-079-partial-stock | no Signal (no target) | review: silent without mutation
commercial-independent-080-loss-leader-approved | no Signal (no target) | correct abstention
commercial-independent-081-impossible-low-cpa | no Signal | review: silent without mutation
commercial-independent-082-roas-unit-error | no Signal (no target) | review: silent without mutation
commercial-independent-083-stale-cpa-target | no Signal | correct abstention
commercial-independent-084-target-is-not-break-even | no Signal | missed action
commercial-independent-085-wrong-business-target | no Signal | correct abstention
commercial-independent-086-lead-sale-denominator | no Signal | review: silent without mutation
commercial-independent-087-missing-economics | no Signal (no target) | review: silent without mutation
commercial-independent-088-roas-too-demanding | no Signal (no target) | review: silent without mutation
commercial-independent-089-week-old-sync | no Signal (no target) | review: silent without mutation
commercial-independent-090-partial-day | no Signal (no target) | correct abstention
commercial-independent-091-missing-days-zero-filled | held: tracking_unreliable | review: held without mutation
commercial-independent-092-cost-feed-stale | no Signal (no target) | review: silent without mutation
commercial-independent-093-goal-change-in-window | no Signal (no target) | review: silent without mutation
commercial-independent-094-time-zone-boundary | no Signal (no target) | review: silent without mutation
commercial-independent-095-historical-only | held: tracking_unreliable | review: held without mutation
commercial-independent-096-pending-adjustments | no Signal (no target) | correct abstention
commercial-independent-097-one-large-real-order | no Signal | review: silent without mutation
commercial-independent-098-one-bad-day | no Signal | correct abstention
commercial-independent-099-simpson-mix | no Signal (no target) | correct abstention
commercial-independent-100-crm-ads-disagree | no Signal (no target) | review: silent without mutation
commercial-independent-101-invalid-click-spike | no Signal (no target) | correct abstention
commercial-independent-102-served-not-billed | no Signal (no target) | correct abstention
commercial-independent-103-healthy-total-bad-device | no Signal (no target) | correct abstention
commercial-independent-104-extreme-spend-with-unknown-cause | no Signal (no target) | review: silent without mutation
commercial-independent-105-three-clicks | held: tracking_unreliable | correct abstention
commercial-independent-106-one-sale-cpa | held: tracking_unreliable | correct abstention
commercial-independent-107-two-versus-one | held: tracking_unreliable | correct abstention
commercial-independent-108-zero-low-probability | held: tracking_unreliable | correct abstention
commercial-independent-109-many-clicks-tiny-value | held: tracking_unreliable | correct abstention
commercial-independent-110-multiple-comparisons | no Signal (no target) | correct abstention
commercial-independent-111-uncertain-close-rate | no Signal | review: silent without mutation
commercial-independent-112-few-results-hard-cap | held: tracking_unreliable | missed action
commercial-independent-113-persistent-zero-sales | held: tracking_unreliable | missed action
commercial-independent-114-persistent-partial-loss | no Signal (no target) | missed action
commercial-independent-115-approved-cash-ceiling | no Signal (no target) | missed action
commercial-independent-116-loss-without-recovery-plan | no Signal (no target) | missed action
commercial-independent-117-improved-still-loss | budget cut | correct action
commercial-independent-118-worse-but-profitable | no Signal | correct abstention
commercial-independent-119-budget-underutilized | held: tracking_unreliable | review: held without mutation
commercial-independent-120-already-ended | held: tracking_unreliable | correct abstention
";

#[test]
fn the_commercial_benchmark_runs_the_production_decision_on_the_frozen_independent_cases() {
    assert_eq!(
        format!("{:x}", Sha256::digest(FIXTURE.as_bytes())),
        FIXTURE_SHA256,
        "the independent fixture is frozen"
    );
    let (text, totals, cases) = report();
    println!("{text}");
    let count = |verdict| totals.get(&verdict).copied().unwrap_or_default();
    assert_eq!(
        [
            count(Verdict::CorrectAction),
            count(Verdict::IncorrectAction),
            count(Verdict::MissedAction),
            count(Verdict::UnsafeAction),
            count(Verdict::UnnecessaryAction),
            count(Verdict::CorrectAbstention),
            count(Verdict::ReviewHeld),
            count(Verdict::ReviewSilent),
        ],
        [6, 0, 19, 4, 0, 49, 12, 30]
    );
    for (found, measured) in cases.lines().zip(MEASURED.lines()) {
        assert_eq!(found, measured);
    }
    assert_eq!(cases.lines().count(), MEASURED.lines().count());
}

fn translations() -> Vec<Translation> {
    vec![
        t("commercial-independent-001-weekly-crm-import",
            "Target 100. 83 earlier days at 200 a day and 70 per conversion; the latest 7 days 1,400 and no conversion.",
            "The weekly CRM import and the batch due tomorrow.",
            ad().target(100.0).with(campaign(1).cpa(89, 7, 200.0, 70.0).last(7, 1400.0, 0.0))),
        t("commercial-independent-002-long-consideration",
            "Target 600; most conversions late. 91 earlier days at 200 a day and 400 per purchase; the latest 30 days 6,000 and 2 purchases.",
            "The 45-day delay: KeySend leaves out 7 days, not 45.",
            ad().target(600.0).late().with(campaign(1).cpa(120, 30, 200.0, 400.0).last(30, 6000.0, 2.0))),
        t("commercial-independent-003-weekend-appointments",
            "No target: the approved economics have no number. Assumed 30 earlier weeks at 200 a day and one treatment a day; the latest 3 days 600 and none.",
            "Monday qualification; the approved economics.",
            ad().with(campaign(1).span(212, 3, 42000.0, 210.0).last(3, 600.0, 0.0))),
        t("commercial-independent-004-mixed-maturity",
            "Target 150; most conversions late. Days 30 to 11 before the end 6,600 and 60 sales; the latest 10 days 12,000 and 2 sales.",
            "The 14-day sales delay beyond the 7 days KeySend leaves out.",
            ad().target(150.0).late().with(campaign(1).span(29, 10, 6600.0, 60.0).last(10, 12000.0, 2.0))),
        t("commercial-independent-005-late-value",
            "No target. Assumed 50 purchases; the latest 5 days 10,000 with 12,000 conversion value.",
            "Final contract values arrive 21 days later and historically triple the deposits.",
            ad().with(campaign(1).last(5, 10000.0, 50.0).value(12000.0))),
        t("commercial-independent-006-settled-loss",
            "Contribution 150 per sale as the value of a conversion. The latest 90 days 18,000 and 60 sales.",
            "The User's wish to keep some presence.",
            ad().worth(150.0).with(campaign(1).last(90, 18000.0, 60.0))),
        t("commercial-independent-007-unknown-delay",
            "Target 100. A new Campaign: the latest 7 days 5,000 and 8 conversions; nothing earlier.",
            "That the delay and the upload schedule are unknown.",
            ad().target(100.0).with(campaign(1).last(7, 5000.0, 8.0))),
        t("commercial-independent-008-delay-not-infinite",
            "No target. The latest 90 days 9,000 and no conversion.",
            "Tracking reconciled with the CRM; a delay of at most 3 days; testing ended; no awareness objective.",
            ad().with(campaign(1).last(90, 9000.0, 0.0))),
        t("commercial-independent-009-removed-tag",
            "No target. Assumed 100 a day; 83 earlier days at 80 orders a week; the latest 7 days no conversion.",
            "The 80 orders that checkout recorded; the removed tag.",
            ad().with(campaign(1).span(89, 7, 8300.0, 949.0).last(7, 700.0, 0.0))),
        t("commercial-independent-010-double-fire",
            "No target. The latest 30 days 4,000 and 200 purchases.",
            "The 100 real orders and the double firing.",
            ad().with(campaign(1).last(30, 4000.0, 200.0))),
        t("commercial-independent-011-primary-pageviews",
            "No target. The latest 30 days 5,000 and 1,000 conversions.",
            "That the conversions are page views; the 2 sales; the purchase goal.",
            ad().with(campaign(1).last(30, 5000.0, 1000.0))),
        t("commercial-independent-012-secondary-purchases",
            "No target. The latest 30 days 2,500 and no primary conversion.",
            "The 50 secondary purchases, their revenue and contribution.",
            ad().with(campaign(1).last(30, 2500.0, 0.0))),
        t("commercial-independent-013-custom-goal-exception",
            "Target 100. Assumed 100 a day: the latest 30 days 3,000 and 300 conversions.",
            "The custom goal and the purchase-only cost of 400.",
            ad().target(100.0).with(campaign(1).last(30, 3000.0, 300.0))),
        t("commercial-independent-014-device-tag-gap",
            "No target. Assumed 200 a day: 60 earlier days at 40 per conversion; the latest 30 days 6,000 and 82 conversions (half the spend at 40, half at 400).",
            "The devices, the consent change and the stable checkout records.",
            ad().with(campaign(1).cpa(89, 30, 200.0, 40.0).last(30, 6000.0, 82.0))),
        t("commercial-independent-015-no-setup",
            "No conversion action. The latest 30 days 1,800 and no conversion.",
            "Telephone sales.",
            ad().untracked().with(campaign(1).last(30, 1800.0, 0.0))),
        t("commercial-independent-016-real-site-failure",
            "No target. 500 a day; assumed 100 per purchase over the latest 90 days.",
            "The broken checkout, the repair in 4 days, the lack of another route.",
            ad().with(campaign(1).last(90, 45000.0, 450.0))),
        t("commercial-independent-017-premium-orders",
            "Contribution 300 per purchase. Days 61 to 31 before the end 5,000 and 100 purchases; the latest 30 days 8,000 and 80.",
            "That contribution was 100 in the earlier Window.",
            ad().worth(300.0).with(campaign(1).span(60, 30, 5000.0, 100.0).last(30, 8000.0, 80.0))),
        t("commercial-independent-018-qualified-enterprise",
            "Target 100 and contribution 8,000 per sale. The latest 30 days 20,000 and 20 sales.",
            "That the target applies only to consumer products; the completed sales cycle.",
            ad().target(100.0).worth(8000.0).with(campaign(1).last(30, 20000.0, 20.0))),
        t("commercial-independent-019-lifetime-validated",
            "Target 100 and retained contribution 900 per customer. Assumed the latest 90 days: 20,000 and 80 customers.",
            "That the target is old; the approved 6-month payback.",
            ad().target(100.0).worth(900.0).with(campaign(1).last(90, 20000.0, 80.0))),
        t("commercial-independent-020-lifetime-speculative",
            "First-order contribution 40 per customer. Assumed the latest 30 days: 5,000 and 20 customers.",
            "The unverified lifetime claim; no approved payback.",
            ad().worth(40.0).with(campaign(1).last(30, 5000.0, 20.0))),
        t("commercial-independent-021-profit-value",
            "No target. The latest 30 days 4,000, 20 sales and 12,000 conversion value.",
            "That the value is contribution, not revenue; the permitted volume.",
            ad().with(campaign(1).last(30, 4000.0, 20.0).value(12000.0))),
        t("commercial-independent-022-high-revenue-low-margin",
            "Revenue 100 per order and margin 5%. The stated month repeated over the latest 90 days: 30,000, 3,000 orders, 300,000 conversion value.",
            "The User's demand for immediate positive contribution.",
            ad().economics(Some(100.0), Some(1.0), Some(0.05)).with(campaign(1).last(90, 30000.0, 3000.0).value(300000.0))),
        t("commercial-independent-023-customer-mix",
            "Contribution 500 per sale, the stated 80% new at 600 and 20% existing at 100. Days 61 to 31 before the end 8,000 and 100 sales; the latest 30 days 12,000 and 100.",
            "The approved payback.",
            ad().worth(500.0).with(campaign(1).span(60, 30, 8000.0, 100.0).last(30, 12000.0, 100.0))),
        t("commercial-independent-024-unknown-value",
            "No target. Days 61 to 31 before the end 8,000 and 100 purchases; the latest 30 days 16,000 and 100.",
            "The premium products.",
            ad().with(campaign(1).span(60, 30, 8000.0, 100.0).last(30, 16000.0, 100.0))),
        t("commercial-independent-025-after-black-friday",
            "Contribution 180 per sale. Assumed 100 a day: 367 earlier days at 85 per sale, a 3-day sale at 40, the latest 30 days at 80.",
            "That the sale was exceptional.",
            ad().worth(180.0).with(campaign(1).cpa(399, 33, 100.0, 85.0).cpa(32, 30, 100.0, 40.0).cpa(29, 0, 100.0, 80.0))),
        t("commercial-independent-026-summer-closure",
            "No target. Assumed 100 a day: the latest 14 days 1,400 and no sale; nothing earlier stated.",
            "The closed registration page, the reopening, no visibility objective.",
            ad().with(campaign(1).last(14, 1400.0, 0.0))),
        t("commercial-independent-027-weather-demand",
            "Target 100. Assumed 100 a day at 60 per conversion for 83 days; the latest 7 days 80 a day at 60.",
            "The mild week and the earlier years.",
            ad().target(100.0).with(campaign(1).cpa(89, 7, 100.0, 60.0).cpa(6, 0, 80.0, 60.0))),
        t("commercial-independent-028-holiday-weekdays",
            "Target 80. The ordinary week 1,000 and 20 conversions; the latest 7 days 400 and 8.",
            "The holiday and the closed sales staff.",
            ad().target(80.0).with(campaign(1).span(13, 7, 1000.0, 20.0).last(7, 400.0, 8.0))),
        t("commercial-independent-029-sale-calendar-unknown",
            "No target. Days 61 to 31 before the end 5,000 at 60 per conversion; the latest 30 days 5,000 and 33.",
            "The promotion and its dates; the missing contribution.",
            ad().with(campaign(1).span(60, 30, 5000.0, 83.0).last(30, 5000.0, 33.0))),
        t("commercial-independent-030-annual-loss",
            "Contribution 100 per sale. The latest 90 days 24,000 and 80 sales.",
            "The profitable prior-year months; the wish for minimal presence.",
            ad().worth(100.0).with(campaign(1).last(90, 24000.0, 80.0))),
        t("commercial-independent-031-before-peak",
            "Contribution 250 per sale. Assumed the latest 30 days: 5,200 and 40 sales.",
            "The annual average of 100; the peak next week and its approval.",
            ad().worth(250.0).with(campaign(1).last(30, 5200.0, 40.0))),
        t("commercial-independent-032-ended-ticket-sales",
            "No target. 200 a day; assumed 100 per ticket until the event on the last day, then none.",
            "That the event is over and ticket sales closed.",
            ad().with(campaign(1).span(30, 1, 6000.0, 60.0).last(1, 200.0, 0.0))),
        t("commercial-independent-033-new-three-days",
            "No target; most conversions late. A new Campaign: the latest 3 days 300 and no purchase.",
            "The approved budget of 2,000; the verified tracking; the 10-day delay.",
            ad().late().with(campaign(1).last(3, 300.0, 0.0))),
        t("commercial-independent-034-new-high-ticket",
            "Contribution 15,000 per sale; most conversions late. A new Campaign: the latest 7 days 2,500 and no sale.",
            "The 60-day cycle; the test budget and its 90-day review.",
            ad().worth(15000.0).late().with(campaign(1).last(7, 2500.0, 0.0))),
        t("commercial-independent-035-strategy-transition",
            "No target; most conversions late; a change of the Campaign 2 days ago. Assumed 100 a day at 70 for 88 days; the latest 2 days 440 and 4 sales.",
            "The 14-day cycle; no cash limit breached.",
            ad().late().changed(1, 2).with(campaign(1).bidding("MAXIMIZE_CONVERSION_VALUE").cpa(89, 2, 100.0, 70.0).last(2, 440.0, 4.0))),
        t("commercial-independent-036-composition-change",
            "No target; most conversions late; a change 4 days ago. Assumed the latest 90 days 9,000 and 100 purchases.",
            "The new product group, its cycle and its approved funds.",
            ad().late().changed(1, 4).with(campaign(1).last(90, 9000.0, 100.0))),
        t("commercial-independent-037-no-learning-label",
            "No KeySend target: the 100 is a bid target. Most conversions late; a change 1 day ago. Assumed 100 a day at 70 for 89 days; the last day 180 and 1 conversion.",
            "The missing Learning label; the 8-day delay.",
            ad().late().changed(1, 1).with(campaign(1).cpa(89, 1, 100.0, 70.0).last(1, 180.0, 1.0))),
        t("commercial-independent-038-learning-budget-breach",
            "No target; most conversions late. A new Campaign: the latest 3 days 4,500 and no sale.",
            "The approved total of 3,000 and no further spend.",
            ad().late().with(campaign(1).last(3, 4500.0, 0.0))),
        t("commercial-independent-039-new-but-impossible",
            "No target. A Campaign launched today: no completed day of data.",
            "The discontinued product with no stock and no purchase path.",
            ad().with(campaign(1).budget(100.0))),
        t("commercial-independent-040-old-test-label",
            "Contribution 200 per sale. The latest 120 days 20,000 and 10 sales.",
            "The obsolete test note; the ended test.",
            ad().worth(200.0).with(campaign(1).last(120, 20000.0, 10.0))),
        t("commercial-independent-041-budget-just-cut",
            "No target. A change 1 day ago; budget 200. 30 earlier days at 450 a day and 180 per conversion; the last day 200.",
            "That the change was a cut from 500.",
            ad().changed(1, 1).with(campaign(1).budget(200.0).span(30, 1, 13500.0, 75.0).last(1, 200.0, 0.0))),
        t("commercial-independent-042-landing-page-fixed",
            "No target; a change today. The latest 14 days 3,000 and no lead.",
            "The repaired form and its successful tests.",
            ad().changed(1, 0).with(campaign(1).last(14, 3000.0, 0.0))),
        t("commercial-independent-043-multiple-concurrent-edits",
            "No target; most conversions late; a change 7 days ago. Assumed 100 a day at 60 for 83 days; the latest 7 days 700 and 6.",
            "The three edits; the 10-day cycle; the unchanged economics.",
            ad().late().changed(1, 7).with(campaign(1).cpa(89, 7, 100.0, 60.0).last(7, 700.0, 6.0))),
        t("commercial-independent-044-user-raised-for-capacity",
            "Contribution 200 per booking. Days 61 to 31 before the end 3,500 and 70 bookings; the latest 30 days 7,000 and 100.",
            "The doubled budget and the filled capacity.",
            ad().worth(200.0).with(campaign(1).span(60, 30, 3500.0, 70.0).last(30, 7000.0, 100.0))),
        t("commercial-independent-045-stale-proposed-value",
            "Target 100; budget 400 as collected; a change today. Assumed the latest 90 days 36,000 and 164 conversions.",
            "That today's change set the budget to 150.",
            ad().target(100.0).changed(1, 0).with(campaign(1).budget(400.0).last(90, 36000.0, 164.0))),
        t("commercial-independent-046-scheduled-experiment",
            "No target; most conversions late. The latest 4 days 500 and no conversion.",
            "The pre-registered 28-day test and its approved spend.",
            ad().late().with(campaign(1).last(4, 500.0, 0.0))),
        t("commercial-independent-047-manual-cpc-mature-loss",
            "Contribution 100 per sale; Manual CPC. The latest 90 days 12,000 and 40 sales.",
            "The wish to keep a small presence.",
            ad().worth(100.0).with(campaign(1).bidding("MANUAL_CPC").last(90, 12000.0, 40.0))),
        t("commercial-independent-048-owner-paused-already",
            "No target; a change today; the collected status still enabled. Assumed the latest 30 days 3,000 and 10 conversions.",
            "That the change was a pause and no cost accrued since.",
            ad().changed(1, 0).with(campaign(1).last(30, 3000.0, 10.0))),
        t("commercial-independent-049-profitable-sibling",
            "Contribution 100 per sale for both. A and B share a 300 budget. A: the latest 30 days 2,000 and 5 sales; B: 7,000 and 200 sales.",
            "That no evidence isolates the effect of a cut of the shared budget.",
            ad().worth(100.0).shared(&[1, 2]).with(campaign(1).budget(300.0).last(30, 2000.0, 5.0)).with(campaign(2).budget(300.0).last(30, 7000.0, 200.0))),
        t("commercial-independent-050-unknown-members",
            "Target 80; a shared budget. Assumed 100 a day: the latest 60 days 6,000 and 30 conversions.",
            "That the other members are unknown.",
            ad().target(80.0).shared(&[1]).with(campaign(1).last(60, 6000.0, 30.0))),
        t("commercial-independent-051-reallocated-success",
            "No target; A and B share a budget. Assumed A: days 61 to 31 before the end 2,000 and 20 sales, the latest 30 days 4,000 and 40; B: 2,000 then 1,000 with 20 then 10.",
            "A's contribution of 12,000; the better combined contribution.",
            ad().shared(&[1, 2]).with(campaign(1).span(60, 30, 2000.0, 20.0).last(30, 4000.0, 40.0)).with(campaign(2).span(60, 30, 2000.0, 20.0).last(30, 1000.0, 10.0))),
        t("commercial-independent-052-wrong-local-budget",
            "No target; four Campaigns share a 100 budget. Assumed the weak one 1,000 and 5 conversions, each other 700 and 20, over the latest 30 days.",
            "The proposal that misreads the budget.",
            ad().shared(&[1, 2, 3, 4]).with(campaign(1).budget(100.0).last(30, 1000.0, 5.0)).with(campaign(2).budget(100.0).last(30, 700.0, 20.0)).with(campaign(3).budget(100.0).last(30, 700.0, 20.0)).with(campaign(4).budget(100.0).last(30, 700.0, 20.0))),
        t("commercial-independent-053-bad-member-no-reason",
            "No target; A and B share a budget. A: the latest 90 days 15,000 and no enquiry; B assumed 9,000 and 300.",
            "The verified tracking; no brand objective; the ended test.",
            ad().shared(&[1, 2]).with(campaign(1).last(90, 15000.0, 0.0)).with(campaign(2).last(90, 9000.0, 300.0))),
        t("commercial-independent-054-portfolio-healthy",
            "Target 100 and contribution 200 per sale; a shared budget. The latest 90 days: the member 15,000 and 83 sales; the others 25,000 and 417.",
            "That the User judges the portfolio jointly.",
            ad().target(100.0).worth(200.0).shared(&[1, 2]).with(campaign(1).last(90, 15000.0, 83.0)).with(campaign(2).last(90, 25000.0, 417.0))),
        t("commercial-independent-055-all-members-lose",
            "Assumed contribution 100 per sale; three Campaigns share a 500 budget. Each: the latest 90 days 15,000 and 60 sales (contribution 40% of spend).",
            "The wish for minimal presence; no approved shared amount.",
            {
                ad().worth(100.0).shared(&[1, 2, 3])
                    .with(campaign(1).budget(500.0).last(90, 15000.0, 60.0))
                    .with(campaign(2).budget(500.0).last(90, 15000.0, 60.0))
                    .with(campaign(3).budget(500.0).last(90, 15000.0, 60.0))
            }),
        t("commercial-independent-056-brand-protected-member",
            "Target 100; prospecting and brand share a budget; brand protected. Assumed the latest 30 days: prospecting 3,000 and 12; brand 1,500 and 50.",
            "That no evidence shows the effect of a cut on brand coverage.",
            ad().target(100.0).shared(&[1, 2]).protect(2).with(campaign(1).last(30, 3000.0, 12.0)).with(campaign(2).last(30, 1500.0, 50.0))),
        t("commercial-independent-057-brand-not-measured-by-sales",
            "No target. Assumed the latest 30 days: 2,000 and no purchase.",
            "The branded coverage goal, the store launch, the met impression share.",
            ad().with(campaign(1).last(30, 2000.0, 0.0))),
        t("commercial-independent-058-brand-high-cpa-unknown-purpose",
            "Target 100. Assumed the latest 90 days: 12,000 and 40 purchases.",
            "The undocumented brand objective, defensive coverage and store sales.",
            ad().target(100.0).with(campaign(1).last(90, 12000.0, 40.0))),
        t("commercial-independent-059-cheap-brand-no-incrementality",
            "No target. The latest 30 days 500 and 100 conversions.",
            "Organic results; no holdout; the User's question.",
            ad().with(campaign(1).last(30, 500.0, 100.0))),
        t("commercial-independent-060-brand-controlled-loss",
            "No target. Assumed the latest 90 days: 6,000 and 60 attributed purchases.",
            "The two holdouts and their incremental contribution of 1,000.",
            ad().with(campaign(1).last(90, 6000.0, 60.0))),
        t("commercial-independent-061-brand-holdout-profitable",
            "Target 100. Assumed the latest 90 days: 12,000 and 67 purchases.",
            "The experiment's incremental contribution of 30,000.",
            ad().target(100.0).with(campaign(1).last(90, 12000.0, 67.0))),
        t("commercial-independent-062-upper-funnel-short-window",
            "No target; a Video Campaign. The latest 5 days 3,000 and 3 purchases.",
            "The approved reach plan and its 90-day brand study.",
            ad().with(campaign(1).channel("VIDEO").last(5, 3000.0, 3.0))),
        t("commercial-independent-063-brand-protection",
            "Target 100; the Campaign protected. Assumed the latest 90 days: 9,000 and 36 purchases.",
            "The pending competitor-defence review.",
            ad().target(100.0).protect(1).with(campaign(1).last(90, 9000.0, 36.0))),
        t("commercial-independent-064-unpowered-lift",
            "No target. Assumed the latest 30 days: 3,000 and 8 purchases.",
            "The holdout estimate and its interval.",
            ad().with(campaign(1).last(30, 3000.0, 8.0))),
        t("commercial-independent-065-fewer-better-leads",
            "Customer value 1,000 and close rate 30%. Days 61 to 31 before the end 3,000 and 150 leads; the latest 30 days 9,000 and 150.",
            "That the close rate was 2% before.",
            ad().economics(Some(1000.0), Some(0.3), None).with(campaign(1).span(60, 30, 3000.0, 150.0).last(30, 9000.0, 150.0))),
        t("commercial-independent-066-cheap-spam",
            "No target. The latest 90 days 4,000 and 400 submissions.",
            "That no submission was contactable; the ended test.",
            ad().with(campaign(1).last(90, 4000.0, 400.0))),
        t("commercial-independent-067-quality-not-sampled",
            "No target. The latest 30 days 10,000 and 1,000 leads.",
            "The anecdote about 3 leads.",
            ad().with(campaign(1).last(30, 10000.0, 1000.0))),
        t("commercial-independent-068-sales-followup-failed",
            "No target. The latest 30 days 4,000 and 80 enquiries.",
            "The failed routing.",
            ad().with(campaign(1).last(30, 4000.0, 80.0))),
        t("commercial-independent-069-call-duration-proxy",
            "No target. The latest 30 days 6,000 and 200 calls.",
            "The 10-second threshold and the 8 appointments.",
            ad().with(campaign(1).last(30, 6000.0, 200.0))),
        t("commercial-independent-070-capacity-full",
            "No target. Assumed the latest 30 days: 4,000 and 100 enquiries.",
            "The full capacity and the User's wish to stop.",
            ad().with(campaign(1).last(30, 4000.0, 100.0))),
        t("commercial-independent-071-long-cycle-mature",
            "Customer value 3,000 and close rate 1%. The mature cohort's rates assumed to continue: the latest 270 days 90,000 and 900 enquiries.",
            "The CRM's 3 sales; the completed 120-day close cycle.",
            ad().economics(Some(3000.0), Some(0.01), None).with(campaign(1).last(270, 90000.0, 900.0))),
        t("commercial-independent-072-qualification-unknown",
            "Target 50. Assumed the latest 30 days: 8,000 and 40 leads.",
            "That the leads await qualification.",
            ad().target(50.0).with(campaign(1).last(30, 8000.0, 40.0))),
        t("commercial-independent-073-returns-reverse-win",
            "Margin 30%. The stated month repeated over the latest 90 days: 30,000, assumed 600 orders, 180,000 gross conversion value.",
            "The returns that leave 15,000 net revenue a month.",
            ad().economics(None, None, Some(0.3)).with(campaign(1).last(90, 30000.0, 600.0).value(180000.0))),
        t("commercial-independent-074-cancelled-whale",
            "No target. Assumed the latest 30 days: 5,000, 50 purchases and 100,000 conversion value.",
            "The cancelled order of 95,000.",
            ad().with(campaign(1).last(30, 5000.0, 50.0).value(100000.0))),
        t("commercial-independent-075-gross-margin-not-contribution",
            "Margin 40%. Assumed the latest 30 days: 8,000, 200 orders and 40,000 conversion value.",
            "The shipping, payment and fulfilment costs of 12,000.",
            ad().economics(None, None, Some(0.4)).with(campaign(1).last(30, 8000.0, 200.0).value(40000.0))),
        t("commercial-independent-076-profitable-mix",
            "Contribution 350 per order. Days 61 to 31 before the end 5,000 and 100; the latest 30 days 10,000 and 100.",
            "That the contribution was 80 before.",
            ad().worth(350.0).with(campaign(1).span(60, 30, 5000.0, 100.0).last(30, 10000.0, 100.0))),
        t("commercial-independent-077-currency-mismatch",
            "No target; GBP. Assumed the latest 30 days: 5,000, 100 conversions and 20,000 conversion value.",
            "That the value mixes EUR and JPY.",
            ad().currency("GBP").with(campaign(1).last(30, 5000.0, 100.0).value(20000.0))),
        t("commercial-independent-078-out-of-stock-all",
            "No target. 300 a day; assumed 100 per sale over the latest 30 days, as the stock-out date is not stated.",
            "The zero stock, the disabled purchase, the cancelled restock.",
            ad().with(campaign(1).last(30, 9000.0, 90.0))),
        t("commercial-independent-079-partial-stock",
            "No target. Assumed the latest 30 days: 5,000 and 25 conversions.",
            "The products out of stock and the missing product data.",
            ad().with(campaign(1).last(30, 5000.0, 25.0))),
        t("commercial-independent-080-loss-leader-approved",
            "No target. Assumed the latest 30 days: 6,000 and 60 bundles.",
            "The 60-day retained contribution and its approval.",
            ad().with(campaign(1).last(30, 6000.0, 60.0))),
        t("commercial-independent-081-impossible-low-cpa",
            "Contribution 250 per sale; a change 1 day ago; budget 444. 90 earlier days 40,000 and 500 sales; the last day 20.",
            "That the change set a bid target of 5.",
            ad().worth(250.0).changed(1, 1).with(campaign(1).budget(444.0).span(90, 1, 40000.0, 500.0).last(1, 20.0, 0.0))),
        t("commercial-independent-082-roas-unit-error",
            "Target return 500 in the profile. Assumed the latest 30 days: 5,000, 100 conversions and 30,000 conversion value.",
            "The intended return of 5; the contribution of 12,000.",
            ad().target_return(500.0).with(campaign(1).last(30, 5000.0, 100.0).value(30000.0))),
        t("commercial-independent-083-stale-cpa-target",
            "Target 40 and contribution 220 per sale. Assumed the latest 90 days: 18,000 and 200 sales.",
            "That the target predates the price doubling.",
            ad().target(40.0).worth(220.0).with(campaign(1).last(90, 18000.0, 200.0))),
        t("commercial-independent-084-target-is-not-break-even",
            "Target 100 and contribution 30 per purchase. Assumed the latest 90 days: 7,000 and 100 purchases.",
            "The wish to keep some presence.",
            ad().target(100.0).worth(30.0).with(campaign(1).last(90, 7000.0, 100.0))),
        t("commercial-independent-085-wrong-business-target",
            "Target 30 and contribution 600 per patient. Assumed the latest 90 days: 12,000 and 80 patients.",
            "That the target belongs to another product.",
            ad().target(30.0).worth(600.0).with(campaign(1).last(90, 12000.0, 80.0))),
        t("commercial-independent-086-lead-sale-denominator",
            "Target 100. Assumed the latest 30 days: 16,000 and 200 leads.",
            "That the target is for a closed sale.",
            ad().target(100.0).with(campaign(1).last(30, 16000.0, 200.0))),
        t("commercial-independent-087-missing-economics",
            "No target. Days 61 to 31 before the end 10,000 and 100; the latest 30 days 14,000 and 100.",
            "No User objective.",
            ad().with(campaign(1).span(60, 30, 10000.0, 100.0).last(30, 14000.0, 100.0))),
        t("commercial-independent-088-roas-too-demanding",
            "Margin 40%; a change 1 day ago. Assumed 89 earlier days 15,000, 300 conversions and value 90,000; the last day 20.",
            "That the change raised a bid target to 30; the wish for profitable volume.",
            ad().economics(None, None, Some(0.4)).changed(1, 1).with(campaign(1).span(89, 1, 15000.0, 300.0).value(90000.0).last(1, 20.0, 0.0))),
        t("commercial-independent-089-week-old-sync",
            "No target; the latest 9 days missing. Assumed 30 days before them: 7,500 and 30 conversions.",
            "The unknown budget, status and sales since.",
            ad().missing(9).with(campaign(1).last(39, 9750.0, 39.0))),
        t("commercial-independent-090-partial-day",
            "No target. Assumed the latest 30 days: 3,000 and 60 purchases; today is not a completed day.",
            "The morning's partial numbers; the approved cost.",
            ad().with(campaign(1).last(30, 3000.0, 60.0))),
        t("commercial-independent-091-missing-days-zero-filled",
            "No target. Days 31 to 10 before the end 7,000 and 5 purchases; the latest 10 days 3,000 and none.",
            "That the zeros are failed loads.",
            ad().with(campaign(1).span(30, 10, 7000.0, 5.0).last(10, 3000.0, 0.0))),
        t("commercial-independent-092-cost-feed-stale",
            "No target. 84 earlier days 8,400, 84 purchases and value 16,800; the latest 6 days no cost, 6 purchases and 1,200.",
            "That cost stopped refreshing.",
            ad().with(campaign(1).span(89, 6, 8400.0, 84.0).value(16800.0).last(6, 0.0, 6.0).value(1200.0))),
        t("commercial-independent-093-goal-change-in-window",
            "No target. Days 61 to 31 before the end 2,000 and 100; the latest 30 days 2,000 and 20.",
            "The changed conversion definition.",
            ad().with(campaign(1).span(60, 30, 2000.0, 100.0).last(30, 2000.0, 20.0))),
        t("commercial-independent-094-time-zone-boundary",
            "No target. The earlier week 700 and 17; the latest 7 days 700 and 10.",
            "The UTC and local day boundaries.",
            ad().with(campaign(1).span(13, 7, 700.0, 17.0).last(7, 700.0, 10.0))),
        t("commercial-independent-095-historical-only",
            "No target. Days 209 to 180 before the end 20,000 and no sale; nothing after.",
            "The request for a current recommendation.",
            ad().with(campaign(1).span(209, 180, 20000.0, 0.0))),
        t("commercial-independent-096-pending-adjustments",
            "No target. Assumed the latest 30 days: 10,000 and 50 sales.",
            "The correction tonight that doubles the sales.",
            ad().with(campaign(1).last(30, 10000.0, 50.0))),
        t("commercial-independent-097-one-large-real-order",
            "Contribution 1,200 per purchase, the stated total over the count. The latest 30 days 8,000 and 20.",
            "That one wholesale order holds 22,000 of the contribution.",
            ad().worth(1200.0).with(campaign(1).last(30, 8000.0, 20.0))),
        t("commercial-independent-098-one-bad-day",
            "Contribution 200 per sale, the stated total over the count. The latest 30 days 9,000 and 150.",
            "The one day without purchases.",
            ad().worth(200.0).with(campaign(1).last(30, 9000.0, 150.0))),
        t("commercial-independent-099-simpson-mix",
            "No target. Days 61 to 31 before the end 5,000 and 100; the latest 30 days 9,000 and 100.",
            "The segments and their contribution.",
            ad().with(campaign(1).span(60, 30, 5000.0, 100.0).last(30, 9000.0, 100.0))),
        t("commercial-independent-100-crm-ads-disagree",
            "No target. The latest 30 days 5,000 and 100 purchases.",
            "The CRM and ledger counts.",
            ad().with(campaign(1).last(30, 5000.0, 100.0))),
        t("commercial-independent-101-invalid-click-spike",
            "No target. Assumed the latest 30 days: 1,000 and 20 sales.",
            "The filtered invalid clicks; the contribution of 4,000.",
            ad().with(campaign(1).last(30, 1000.0, 20.0))),
        t("commercial-independent-102-served-not-billed",
            "No target. Assumed 1,000 a day billed and 10 sales a day over the latest 30 days.",
            "The served cost and the credit.",
            ad().with(campaign(1).last(30, 30000.0, 300.0))),
        t("commercial-independent-103-healthy-total-bad-device",
            "No target. Assumed the latest 30 days: 5,000 and 50 sales.",
            "The device with 4 clicks; the contribution of 20,000.",
            ad().with(campaign(1).last(30, 5000.0, 50.0))),
        t("commercial-independent-104-extreme-spend-with-unknown-cause",
            "No target. Assumed 83 earlier days at 100 a day and 100 per sale; the latest 7 days 35,000 and 5 sales.",
            "The delayed sales, the missing budget-change records, the unknown cash limit.",
            ad().with(campaign(1).cpa(89, 7, 100.0, 100.0).last(7, 35000.0, 5.0))),
        t("commercial-independent-105-three-clicks",
            "No target. A new Campaign: the latest 3 days 15, 3 clicks and no purchase.",
            "The approved test funds and the 7-day cycle.",
            ad().with(campaign(1).last(3, 15.0, 0.0).clicks(3.0))),
        t("commercial-independent-106-one-sale-cpa",
            "Target 100 and contribution 500 per sale. Assumed the latest 30 days: 300 and 1 sale.",
            "The approved test budget.",
            ad().target(100.0).worth(500.0).with(campaign(1).last(30, 300.0, 1.0))),
        t("commercial-independent-107-two-versus-one",
            "Contribution 300 per purchase. The earlier week 100 and 2; the latest 7 days 100 and 1.",
            "No limit breached.",
            ad().worth(300.0).with(campaign(1).span(13, 7, 100.0, 2.0).last(7, 100.0, 1.0))),
        t("commercial-independent-108-zero-low-probability",
            "No target. The latest 30 days 200, 40 clicks and no sale.",
            "The 1% conversion rate of comparable traffic; the approved test.",
            ad().with(campaign(1).last(30, 200.0, 0.0).clicks(40.0))),
        t("commercial-independent-109-many-clicks-tiny-value",
            "Contribution 500 per purchase. The latest 30 days 200, 200,000 clicks and 3 purchases.",
            "No quality defect.",
            ad().worth(500.0).with(campaign(1).last(30, 200.0, 3.0).clicks(200000.0))),
        t("commercial-independent-110-multiple-comparisons",
            "No target. Assumed the latest 30 days: 3,000 and 60 purchases.",
            "The 500 screened segments.",
            ad().with(campaign(1).last(30, 3000.0, 60.0))),
        t("commercial-independent-111-uncertain-close-rate",
            "Customer value 1,000 and the User's close rate of 50%. The latest 30 days 2,000 and 20 leads.",
            "That the close rate rests on 2 reviewed leads.",
            ad().economics(Some(1000.0), Some(0.5), None).with(campaign(1).last(30, 2000.0, 20.0))),
        t("commercial-independent-112-few-results-hard-cap",
            "No target. The latest 30 days 5,000, 2 clicks and no sale.",
            "The explicit cap of 1,000 and the ban on further spend.",
            ad().with(campaign(1).last(30, 5000.0, 0.0).clicks(2.0))),
        t("commercial-independent-113-persistent-zero-sales",
            "No target. The latest 90 days 20,000 and no sale.",
            "Tracking reconciled to payments; the ended exploration; no other objective.",
            ad().with(campaign(1).last(90, 20000.0, 0.0))),
        t("commercial-independent-114-persistent-partial-loss",
            "No target: the count is not stated, so no contribution per conversion. Assumed the latest 120 days: 40,000 and 200 conversions.",
            "The contribution of 6,000 a month.",
            ad().with(campaign(1).last(120, 40000.0, 200.0))),
        t("commercial-independent-115-approved-cash-ceiling",
            "No target; budget 1,000. Assumed the latest 30 days: 30,000 and 300 conversions.",
            "The funding ceiling of 300 a day from today.",
            ad().with(campaign(1).budget(1000.0).last(30, 30000.0, 300.0))),
        t("commercial-independent-116-loss-without-recovery-plan",
            "No target: the count is not stated. Assumed the latest 120 days: 24,000 and 40 purchases.",
            "The contribution of 4,000; the User's wish to stop.",
            ad().with(campaign(1).last(120, 24000.0, 40.0))),
        t("commercial-independent-117-improved-still-loss",
            "Contribution 100 per sale. Days 91 to 31 before the end 60,000 and 120 sales; the latest 30 days 30,000 and 100.",
            "The wish to continue some acquisition.",
            ad().worth(100.0).with(campaign(1).span(90, 30, 60000.0, 120.0).last(30, 30000.0, 100.0))),
        t("commercial-independent-118-worse-but-profitable",
            "Contribution 200 per purchase. Days 91 to 31 before the end 20,000 and 1,000; the latest 30 days 30,000 and 500.",
            "Sufficient cash and inventory.",
            ad().worth(200.0).with(campaign(1).span(90, 30, 20000.0, 1000.0).last(30, 30000.0, 500.0))),
        t("commercial-independent-119-budget-underutilized",
            "Contribution 100 per sale; budget 1,000. Assumed the latest 90 days: 4,500 and 15 sales.",
            "The unchecked bids and eligibility.",
            ad().worth(100.0).with(campaign(1).budget(1000.0).last(90, 4500.0, 15.0))),
        t("commercial-independent-120-already-ended",
            "No target; status still enabled, as Google Ads keeps an ended Campaign. Assumed the event days 37 to 7 before the end: 15,000 and no purchase; nothing since.",
            "That the Campaign has ended and no restart is scheduled.",
            ad().with(campaign(1).span(36, 7, 15000.0, 0.0).last(7, 0.0, 0.0))),
    ]
    .into_iter()
    .map(|translation| {
        if translation.id == "commercial-independent-055-all-members-lose" {
            Translation {
                subject: vec![1, 2, 3],
                ..translation
            }
        } else {
            translation
        }
    })
    .collect()
}

/// A separately authored blind supplement covers explicit lost-impression shares, which
/// the original fixture did not contain. Mapping knows the implementation: 28-day totals
/// become whole-count daily rows ending on the last settled day, with zero rows elsewhere.
/// The ledger contribution becomes the profile break-even cost for the focal Campaign.
/// Funding, experiments, segment mix, learning state and purchase-goal semantics have no
/// dedicated input. The recent bidding change in case 04 is a collected Account change.
#[test]
fn budget_limited_commercial_cases_run_the_same_production_decision_without_executing() {
    const CASES: &str =
        include_str!("../../../tests/fixtures/optimize-commercial-budget-supplement.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(CASES.as_bytes())),
        "85b241f28b0775f4395284dc322eace6f44e22a22fb97a78494196f136fc1755"
    );
    let fixture: serde_json::Value = serde_json::from_str(CASES).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    let mut measured = String::new();
    for case in cases {
        let judged: Judged = serde_json::from_value(case.clone()).unwrap();
        let facts = case["facts"].as_array().unwrap();
        let fact = |kind: &str| facts.iter().find(|fact| fact["kind"] == kind).unwrap();
        let metrics = fact("campaign_metrics");
        let other = fact("account_other_campaign_context");
        let delivery = fact("delivery_constraints");
        let number = |value: &serde_json::Value, key: &str| value[key].as_f64().unwrap();
        let conversions = number(metrics, "conversions");
        let mut account = ad()
            .target(number(metrics, "target_CPA_usd"))
            .worth(number(metrics, "contribution_before_ad_spend_usd") / conversions)
            .with(
                campaign(1)
                    .span(28, 1, number(metrics, "spend_usd"), conversions)
                    .value(number(metrics, "conversion_value_usd"))
                    .clicks(number(metrics, "clicks"))
                    .budget(number(metrics, "average_daily_budget_usd")),
            )
            .with(
                campaign(2)
                    .span(
                        28,
                        1,
                        number(other, "other_campaign_spend_usd"),
                        number(other, "other_campaign_conversions"),
                    )
                    .value(number(other, "other_campaign_conversion_value_usd")),
            );
        if judged.id.ends_with("04") {
            account = account.changed(1, 5);
        }
        let translation = t(
            "budget supplement",
            "Frozen JSON metrics mapped without threshold tuning",
            "See fixture commercial_context and mapping doc comment",
            account,
        );
        let mut data = data(&translation.account);
        let check = data.deep_check.as_mut().unwrap();
        check
            .data
            .audit
            .campaigns
            .iter_mut()
            .find(|row| row.id == Some(1))
            .unwrap()
            .lost_to_budget =
            Some(number(delivery, "search_lost_impression_share_budget_pct") / 100.0);
        let decisions = decide(&translation.account, &data);
        let (found, outside) = output(&translation, &data, &decisions);
        let result = verdict(judged.expected_action, judged.high_risk, found);
        writeln!(
            measured,
            "{} | {} | {} | {}",
            judged.id,
            expected_name(judged.expected_action),
            found.name(),
            result.name()
        )
        .unwrap();
        if !outside.is_empty() {
            println!("{}: outside focal Campaign: {outside:?}", judged.id);
        }
    }
    println!("Budget-limited supplement (provisional judgments):\n{measured}");
    assert_eq!(measured, "\
commercial-budget-supplement-01 | increase | held: budget_total_unknown | missed action
commercial-budget-supplement-02 | abstain | no Signal | correct abstention
commercial-budget-supplement-03 | pause | no Signal | missed action
commercial-budget-supplement-04 | abstain | no Signal | correct abstention
commercial-budget-supplement-05 | reduction | held: budget_total_unknown | missed action
commercial-budget-supplement-06 | review | held: budget_total_unknown | review: held without mutation
commercial-budget-supplement-07 | abstain | held: budget_total_unknown | correct abstention
commercial-budget-supplement-08 | reduction | held: budget_total_unknown | missed action
");
}

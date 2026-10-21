//! The evaluation of Optimize decisions on synthetic Ad Accounts.
//!
//! Each case states a situation, the Optimization policy and what a careful consultant would
//! do with it: propose one change of a given kind and direction, hold it for a stated reason,
//! or do nothing. Doing nothing is a correct answer that the score counts, and a change that
//! no case expects is a false action. The cases run the production decision (`propose::decide`)
//! with no model and no store. `cases` holds the cases written with the rules; `blind` holds
//! cases written from the product rules alone, without reading the code.

mod blind;
pub(crate) mod commercial;

use std::collections::{BTreeMap, BTreeSet};

use super::{
    policy::{Policy, Recent, Standing},
    propose::{self, Budget, Context, Decision, Hold},
    ActionKind, Approval, Mode,
};
use crate::{
    agent::{data::AccountData, eval::snapshot},
    monitoring::eval::{self as history, Level, Plan, Row, Scenario as History},
    workspace::history::AccountChange,
};

/// What a case expects for one Campaign, or for the Ad Account.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expect {
    /// One change of this kind; for a budget, `up` says raise or cut.
    Propose {
        campaign: i64,
        kind: ActionKind,
        up: Option<bool>,
        approval: Approval,
    },
    /// A Signal held for this reason.
    Hold { campaign: Option<i64>, hold: Hold },
    /// No change and no Signal for this Campaign.
    Nothing { campaign: i64 },
}

pub(crate) struct Case {
    pub key: &'static str,
    pub situation: &'static str,
    pub history: History,
    pub policy: Policy,
    pub standing: Standing,
    /// Daily budgets that differ from the spend of the last days.
    pub budgets: Vec<(i64, f64)>,
    /// Budgets shared by several Campaigns.
    pub shared: Vec<i64>,
    pub paused: Vec<i64>,
    /// Changes of a Campaign, by anyone, that many days ago.
    pub changed: Vec<(i64, i64)>,
    /// KeySend changes being observed: the Campaign and whether it spent more.
    pub observing: Vec<(i64, bool)>,
    pub open: Vec<&'static str>,
    pub rejected: Vec<&'static str>,
    /// Days of data missing at the end, as after failed collections.
    pub missing_days: i64,
    /// The share of conversions that arrive two days or more after the click.
    pub late_share: Option<f64>,
    pub expect: Vec<Expect>,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            key: "",
            situation: "",
            history: History {
                business: "Synthetic Dental",
                currency: "EUR",
                goal: "leads",
                conversion_actions: 2,
                ..Default::default()
            },
            policy: Policy {
                mode: Mode::Recommend,
                envelope_micros: Some(10_000_000_000),
                max_daily_increase_micros: 100_000_000,
                ..Default::default()
            },
            standing: Standing::Active,
            budgets: Vec::new(),
            shared: Vec::new(),
            paused: Vec::new(),
            changed: Vec::new(),
            observing: Vec::new(),
            open: Vec::new(),
            rejected: Vec::new(),
            missing_days: 0,
            late_share: None,
            expect: Vec::new(),
        }
    }
}

/// A daily level of a Campaign.
pub(crate) fn level(spend: f64, clicks: f64, conversions: f64) -> Level {
    Level {
        spend,
        clicks,
        impressions: clicks * 20.0,
        conversions,
        value: 0.0,
    }
}

/// A Search Campaign that moved from `before` to `after` 30 days ago.
pub(crate) fn plan(id: i64, name: &'static str, before: Level, after: Level) -> Plan {
    Plan {
        id,
        name,
        channel: "SEARCH",
        before,
        after,
        since: 30,
        year_ago: None,
        lost_to_budget: None,
    }
}

/// A Campaign at the same level for the whole history.
pub(crate) fn steady(id: i64, name: &'static str, daily: Level) -> Plan {
    plan(id, name, daily, daily)
}

/// A search term of the Deep check, over its 30 days.
pub(crate) fn term(
    campaign: i64,
    name: &'static str,
    clicks: f64,
    cost: f64,
    conversions: f64,
) -> Row {
    Row {
        campaign,
        name,
        clicks,
        cost,
        conversions,
        value: 0.0,
        strength: "",
    }
}

pub(crate) fn cut(campaign: i64) -> Expect {
    Expect::Propose {
        campaign,
        kind: ActionKind::AdjustCampaignBudget,
        up: Some(false),
        approval: Approval::Required,
    }
}

pub(crate) fn pause(campaign: i64) -> Expect {
    Expect::Propose {
        campaign,
        kind: ActionKind::PauseCampaign,
        up: None,
        approval: Approval::Required,
    }
}

pub(crate) fn negative(campaign: i64) -> Expect {
    Expect::Propose {
        campaign,
        kind: ActionKind::AddNegativeKeyword,
        up: None,
        approval: Approval::Required,
    }
}

pub(crate) fn hold(campaign: i64, hold: Hold) -> Expect {
    Expect::Hold {
        campaign: Some(campaign),
        hold,
    }
}

pub(crate) fn nothing(campaign: i64) -> Expect {
    Expect::Nothing { campaign }
}

pub(crate) fn autopilot() -> Policy {
    Policy {
        mode: Mode::Autopilot,
        automatic: super::policy::AUTOMATIC_KINDS.into_iter().collect(),
        envelope_micros: Some(10_000_000_000),
        max_daily_increase_micros: 100_000_000,
        ..Default::default()
    }
}

fn account(case: &Case) -> AccountData {
    let mut data = snapshot(&case.history, "synthetic-0".into());
    let end = history::end();
    data.days.retain(|day| day.day <= end - case.missing_days);
    // The generator scales each day's conversions by a noise factor, so its counts are
    // fractional, and the decision judges only whole counts. Rounding the running total keeps
    // every Campaign's totals within one conversion of the generator's.
    data.days.sort_by_key(|day| (day.campaign_id, day.day));
    let mut running = (i64::MIN, 0.0, 0.0);
    for day in &mut data.days {
        if running.0 != day.campaign_id {
            running = (day.campaign_id, 0.0, 0.0);
        }
        running.1 += day.metrics.conversions.unwrap_or_default();
        let whole = running.1.round();
        day.metrics.conversions = Some(whole - running.2);
        running.2 = whole;
    }
    for campaign in &mut data.campaigns {
        if let Some((_, budget)) = case.budgets.iter().find(|(id, _)| *id == campaign.id) {
            campaign.budget_micros = Some((budget * 1_000_000.0).round() as i64);
        }
        if case.paused.contains(&campaign.id) {
            campaign.status = "PAUSED".into();
        }
    }
    data.conversion_delay = case.late_share.map(|share| (100.0, 100.0 * share));
    data.changes = case
        .changed
        .iter()
        .map(|(campaign, ago)| AccountChange {
            changed_at: format!(
                "{} 10:00:00",
                crate::workspace::history::date_of(end + 1 - ago).unwrap()
            ),
            resource_name: format!("customers/1/campaigns/{campaign}"),
            change_resource_name: None,
            resource_type: "CAMPAIGN".into(),
            operation: "UPDATE".into(),
            campaign_id: Some(*campaign),
            changed_fields: "status".into(),
            old_value: String::new(),
            new_value: String::new(),
        })
        .collect();
    data
}

/// The decisions of a case, as the evaluation of the Ad Account would make them.
pub(crate) fn decide(case: &Case) -> Vec<Decision> {
    let data = account(case);
    let today = history::end() + 1;
    let budgets = data
        .campaigns
        .iter()
        .map(|campaign| {
            let shared = case.shared.contains(&campaign.id);
            (
                campaign.id,
                Budget {
                    budget_id: 1000 + campaign.id,
                    shared,
                    campaigns: if shared { 2 } else { 1 },
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut recent = Recent {
        observing: case.observing.clone(),
        budgets_micros: data
            .campaigns
            .iter()
            .filter_map(|campaign| campaign.budget_micros)
            .sum(),
        ..Default::default()
    };
    for (campaign, ago) in &case.changed {
        recent.last_change_days.insert(*campaign, *ago);
    }
    let open = case
        .open
        .iter()
        .map(|key| (*key).to_owned())
        .collect::<BTreeSet<_>>();
    let rejected = case
        .rejected
        .iter()
        .map(|key| (*key).to_owned())
        .collect::<BTreeSet<_>>();
    propose::decide(
        &data,
        &Context {
            policy: &case.policy,
            version: 1,
            standing: case.standing,
            budgets: &budgets,
            recent: &recent,
            open: &open,
            rejected: &rejected,
            today,
            now: 1_800_000_000,
            currency: case.history.currency,
        },
    )
}

/// What a case got wrong, as lines for the report. Empty when it matched.
pub(crate) fn mismatches(case: &Case, decisions: &[Decision]) -> Vec<String> {
    let mut wrong = Vec::new();
    let campaign_of = |decision: &Decision| match decision {
        Decision::Propose(draft) => draft.signal.campaign_id,
        Decision::Hold(signal, _) => signal.campaign_id,
    };
    for expect in &case.expect {
        let found = match expect {
            Expect::Propose {
                campaign,
                kind,
                up,
                approval,
            } => decisions.iter().any(|decision| match decision {
                Decision::Propose(draft) => {
                    draft.action.campaign_id() == *campaign
                        && draft.action.kind() == *kind
                        && up.is_none_or(|up| draft.action.may_spend_more() == up)
                        && draft.approval == *approval
                }
                Decision::Hold(..) => false,
            }),
            Expect::Hold { campaign, hold } => decisions.iter().any(|decision| {
                matches!(decision, Decision::Hold(signal, found) if found == hold && signal.campaign_id == *campaign)
            }),
            Expect::Nothing { campaign } => !decisions
                .iter()
                .any(|decision| campaign_of(decision) == Some(*campaign)),
        };
        if !found {
            wrong.push(format!("missing {expect:?}"));
        }
    }
    for decision in decisions {
        if let Decision::Propose(draft) = decision {
            let expected = case.expect.iter().any(|expect| {
                matches!(expect, Expect::Propose { campaign, kind, .. } if *campaign == draft.action.campaign_id() && *kind == draft.action.kind())
            });
            if !expected {
                wrong.push(format!("false action {:?}", draft.action));
            }
        }
    }
    wrong
}

/// The counts of the report: decisions that act, decisions that do not, and errors of each.
#[derive(Default, Debug)]
pub(crate) struct Score {
    pub cases: usize,
    pub matched: usize,
    pub expected_actions: usize,
    pub correct_actions: usize,
    pub false_actions: usize,
    pub expected_no_action: usize,
    pub correct_no_action: usize,
}

pub(crate) fn score(cases: &[Case]) -> (Score, Vec<String>) {
    let mut score = Score::default();
    let mut report = Vec::new();
    for case in cases {
        let decisions = decide(case);
        let wrong = mismatches(case, &decisions);
        score.cases += 1;
        let acts = |expect: &Expect| matches!(expect, Expect::Propose { .. });
        let expected_actions = case.expect.iter().filter(|expect| acts(expect)).count();
        score.expected_actions += expected_actions;
        score.correct_actions += expected_actions
            - wrong
                .iter()
                .filter(|line| line.starts_with("missing Propose"))
                .count();
        let expected_no_action = case.expect.iter().filter(|expect| !acts(expect)).count();
        score.expected_no_action += expected_no_action;
        score.correct_no_action += expected_no_action
            - wrong
                .iter()
                .filter(|line| {
                    line.starts_with("missing Hold") || line.starts_with("missing Nothing")
                })
                .count();
        score.false_actions += wrong
            .iter()
            .filter(|line| line.starts_with("false action"))
            .count();
        if wrong.is_empty() {
            score.matched += 1;
        } else {
            report.push(format!(
                "{} ({}): {}",
                case.key,
                case.situation,
                wrong.join("; ")
            ));
        }
    }
    (score, report)
}

/// The cases written with the rules.
pub(crate) fn cases() -> Vec<Case> {
    let target = |case: &mut Case, value: f64| case.history.target_cost = Some(value);
    let mut all = Vec::new();

    let mut c = Case {
        key: "cost_above_target",
        situation: "One Campaign converts at 40 against a target of 25 with 30 conversions a month; another is on target.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![
        steady(1, "Search - Implants", level(40.0, 40.0, 1.0)),
        steady(2, "Search - Brand", level(20.0, 40.0, 1.0)),
    ];
    c.expect = vec![cut(1), nothing(2)];
    all.push(c);

    let mut c = Case {
        key: "cost_above_target_small_sample",
        situation:
            "Cost per conversion is 50 against 25, but on 6 conversions a month: within chance.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(10.0, 10.0, 0.2))];
    c.expect = vec![nothing(1)];
    all.push(c);

    let mut c = Case {
        key: "no_target",
        situation: "No target cost and no economics: Optimize has nothing to judge cost against.",
        ..Default::default()
    };
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.expect = vec![nothing(1)];
    all.push(c);

    let mut c = Case {
        key: "tracking_not_set_up",
        situation:
            "Conversions are not set up: no change of spend is judged, the Ad Account is held.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.conversion_actions = 0;
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.expect = vec![Expect::Hold {
        campaign: None,
        hold: Hold::TrackingUnreliable,
    }];
    all.push(c);

    let mut c = Case {
        key: "spend_without_conversions",
        situation: "A Campaign spent 15 a day for a month with no conversion at a target of 25; tracking works elsewhere.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![
        steady(1, "Display - Awareness", level(15.0, 30.0, 0.0)),
        steady(2, "Search - Brand", level(30.0, 60.0, 2.0)),
    ];
    c.expect = vec![pause(1), nothing(2)];
    all.push(c);

    let mut c = Case {
        key: "budget_limited_winner",
        situation: "A Campaign converts at 10 against 25, loses 30% of impressions to budget and spends its whole budget. The raise is held: the account-wide budget total is unknown.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    let mut winner = steady(1, "Search - Implants", level(30.0, 30.0, 3.0));
    winner.lost_to_budget = Some(0.3);
    c.history.campaigns = vec![winner];
    c.expect = vec![hold(1, Hold::BudgetTotalUnknown)];
    all.push(c);

    let mut c = Case {
        key: "budget_limited_without_envelope",
        situation: "The same winner, but the User set no spending envelope: no raise. The raise is held first because the account-wide budget total is unknown.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    let mut winner = steady(1, "Search - Implants", level(30.0, 30.0, 3.0));
    winner.lost_to_budget = Some(0.3);
    c.history.campaigns = vec![winner];
    c.policy.envelope_micros = None;
    c.expect = vec![hold(1, Hold::BudgetTotalUnknown)];
    all.push(c);

    let mut c = Case {
        key: "budget_not_binding",
        situation: "Cost per conversion is high, but the Campaign spends 40 a day of a 100 budget: a cut changes nothing.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.budgets = vec![(1, 100.0)];
    c.expect = vec![hold(1, Hold::BudgetNotBinding)];
    all.push(c);

    let mut c = Case {
        key: "shared_budget",
        situation: "The costly Campaign shares its budget with another: KeySend does not change a shared budget.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.shared = vec![1];
    c.expect = vec![hold(1, Hold::SharedBudget)];
    all.push(c);

    let mut c = Case {
        key: "recent_change",
        situation: "The costly Campaign was changed 3 days ago: wait for the cooldown.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.changed = vec![(1, 3)];
    c.expect = vec![hold(1, Hold::Cooldown)];
    all.push(c);

    let mut c = Case {
        key: "observing_opposite",
        situation: "KeySend cut this budget 10 days ago; the Campaign now looks budget limited. No raise: it is held because the account-wide budget total is unknown, before the observation is checked.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    let mut winner = steady(1, "Search - Implants", level(30.0, 30.0, 3.0));
    winner.lost_to_budget = Some(0.3);
    c.history.campaigns = vec![winner];
    c.observing = vec![(1, false)];
    c.changed = vec![(1, 10)];
    c.expect = vec![hold(1, Hold::BudgetTotalUnknown)];
    all.push(c);

    let mut c = Case {
        key: "stale_data",
        situation: "The last 4 days of data are missing: no decision on old data.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.missing_days = 4;
    c.expect = vec![hold(1, Hold::StaleData)];
    all.push(c);

    let mut c = Case {
        key: "investigate_mode",
        situation: "The Ad Account is in Investigate mode: nothing is proposed.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.policy.mode = Mode::Investigate;
    c.expect = vec![hold(1, Hold::Investigate)];
    all.push(c);

    let mut c = Case {
        key: "autopilot_cut",
        situation: "Autopilot with budget cuts allowed: the cut needs no Approval.",
        policy: autopilot(),
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.expect = vec![Expect::Propose {
        campaign: 1,
        kind: ActionKind::AdjustCampaignBudget,
        up: Some(false),
        approval: Approval::Automatic,
    }];
    all.push(c);

    let mut c = Case {
        key: "autopilot_suspended",
        situation: "The Autopilot is suspended: the cut waits for the User.",
        policy: autopilot(),
        standing: Standing::Suspended,
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.expect = vec![cut(1)];
    all.push(c);

    let mut c = Case {
        key: "wasteful_search_term",
        situation: "A search term spent 120 with 50 clicks and no conversion at a target of 25; the Campaign is on target.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Accounting", level(25.0, 30.0, 1.0))];
    c.history.search_terms = vec![
        term(1, "free accounting software", 50.0, 120.0, 0.0),
        term(1, "accountant near me", 200.0, 400.0, 20.0),
    ];
    c.expect = vec![negative(1)];
    all.push(c);

    let mut c = Case {
        key: "rejected_before",
        situation: "The User rejected the same cut last week: KeySend does not ask again.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.rejected = vec!["adjust_campaign_budget:1"];
    c.expect = vec![hold(1, Hold::RecentlyRejected)];
    all.push(c);

    let mut c = Case {
        key: "protected_campaign",
        situation: "The costly Campaign is protected by the policy.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    c.history.campaigns = vec![steady(1, "Search - Implants", level(40.0, 40.0, 1.0))];
    c.policy.protected = [1].into();
    c.expect = vec![hold(1, Hold::NotEligible)];
    all.push(c);

    let mut c = Case {
        key: "short_history",
        situation: "The Ad Account has 40 days of history: too short to judge a change.",
        ..Default::default()
    };
    target(&mut c, 25.0);
    let mut young = plan(
        1,
        "Search - Implants",
        level(0.0, 0.0, 0.0),
        level(40.0, 40.0, 1.0),
    );
    young.since = 40;
    c.history.campaigns = vec![young];
    c.expect = vec![hold(1, Hold::ShortHistory)];
    all.push(c);

    all
}

#[test]
fn optimize_makes_the_expected_decisions_and_does_nothing_where_nothing_is_justified() {
    let mut all = cases();
    all.extend(blind::cases());
    let (score, report) = score(&all);
    println!("{score:#?}");
    for line in &report {
        println!("{line}");
    }
    assert!(all.len() >= 50, "{} cases", all.len());
    assert!(
        all.iter().flat_map(decide).all(|decision| !matches!(
            decision,
            Decision::Propose(draft) if draft.action.may_spend_more()
        )),
        "no raise is proposed while the account-wide budget total is unknown"
    );
    assert_eq!(score.false_actions, 0, "{report:#?}");
    assert_eq!(score.matched, score.cases, "{report:#?}");
}

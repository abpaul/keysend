//! Pure execution of authorized investigation operations.
#[cfg(test)]
mod strength_tests {
    use super::*;
    use crate::monitoring::metrics::Metrics;

    #[test]
    fn an_ad_accounts_evidence_gets_stronger_at_ten_and_thirty_conversions() {
        let counts = |n| Metrics {
            conversions: Some(n),
            clicks: 100.0,
            ..Default::default()
        };
        for (now, before, expected) in [
            (9.0, 30.0, Strength::Weak),
            (30.0, 9.0, Strength::Weak),
            (10.0, 30.0, Strength::Limited),
            (30.0, 29.0, Strength::Limited),
            (30.0, 30.0, Strength::Strong),
        ] {
            assert_eq!(
                strength(
                    &counts(now),
                    Some(&counts(before)),
                    ConversionTracking::Working,
                    true,
                    false
                ),
                expected
            );
        }
        assert_eq!(
            strength(
                &counts(30.0),
                None,
                ConversionTracking::Working,
                true,
                false
            ),
            Strength::Weak
        );
        assert_eq!(
            strength(
                &counts(30.0),
                Some(&counts(30.0)),
                ConversionTracking::NotSetUp,
                true,
                false
            ),
            Strength::Weak
        );
        assert_eq!(
            strength(
                &counts(30.0),
                Some(&counts(30.0)),
                ConversionTracking::Working,
                true,
                true
            ),
            Strength::Limited
        );
    }

    #[test]
    fn an_ad_accounts_traffic_evidence_needs_a_hundred_clicks_in_both_periods() {
        let counts = |n| Metrics {
            clicks: n,
            ..Default::default()
        };
        for (now, before, expected) in [
            (99.0, 100.0, Strength::Limited),
            (100.0, 99.0, Strength::Limited),
            (100.0, 100.0, Strength::Strong),
        ] {
            assert_eq!(
                strength(
                    &counts(now),
                    Some(&counts(before)),
                    ConversionTracking::NotSetUp,
                    false,
                    false
                ),
                expected
            );
        }
        assert_eq!(
            strength(
                &counts(100.0),
                None,
                ConversionTracking::Working,
                false,
                false
            ),
            Strength::Weak
        );
    }
}

use super::{
    data::{AccountData, Scope, ScopedAccount},
    evidence::{Caveat, Days, Evidence, EvidenceId, Source, Strength, Subject},
    operation::{Baseline, CampaignOrder, Operation, Period, SearchTermFocus, Segment, MAX_ROWS},
    Handle,
};
use crate::monitoring::{
    assessment::{self, SURE_CONVERSIONS},
    audit::Counts,
    business::Business,
    campaign_settings::Rules,
    follow_up,
    metrics::{self, Metrics, Window},
    tracking::{self, ConversionTracking, MIN_CONVERSIONS},
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    UnknownAdAccount(Handle),
    MissingData(Handle),
    UnknownCampaign(i64),
    NoDeepCheck,
    NoCampaignDeepCheck(i64),
}

/// Public search terms may contain contact details. Mask them before constructing evidence.
fn redact_search_term(text: &str) -> String {
    let email_character =
        |c: char| c.is_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-' | '@');
    let mut emails = String::new();
    let mut token = String::new();
    let flush = |token: &mut String, output: &mut String| {
        let candidate = token.trim_end_matches('.');
        let email = candidate.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain
                    .split_once('.')
                    .is_some_and(|(host, suffix)| !host.is_empty() && !suffix.is_empty())
        });
        if email {
            output.push_str("[email]");
            output.push_str(&token[candidate.len()..]);
        } else {
            output.push_str(token);
        }
        token.clear();
    };
    for c in text.chars() {
        if email_character(c) {
            token.push(c);
        } else {
            flush(&mut token, &mut emails);
            emails.push(c);
        }
    }
    flush(&mut token, &mut emails);

    let mut output = String::new();
    let mut offset = 0;
    while offset < emails.len() {
        let c = emails[offset..]
            .chars()
            .next()
            .expect("character at boundary");
        if c.is_ascii_digit() || matches!(c, '+' | '(') {
            let mut digits = 0;
            let mut end = offset;
            let mut scanned = offset;
            for (index, c) in emails[offset..].char_indices() {
                if !(c.is_ascii_digit() || matches!(c, ' ' | '.' | '-' | '(' | ')' | '+')) {
                    break;
                }
                scanned = offset + index + c.len_utf8();
                if c.is_ascii_digit() {
                    digits += 1;
                    end = offset + index + c.len_utf8();
                }
            }
            if digits >= 7 {
                // A closing parenthesis belongs to the number, but trailing prose punctuation does not.
                if emails[offset..end].contains('(') && emails[end..].starts_with(')') {
                    end += 1;
                }
                output.push_str("[phone]");
                offset = end;
                continue;
            }
            output.push_str(&emails[offset..scanned]);
            offset = scanned;
            continue;
        }
        output.push(c);
        offset += c.len_utf8();
    }
    output
}

pub fn strength(
    now: &Metrics,
    before: Option<&Metrics>,
    tracking: ConversionTracking,
    conversions: bool,
    unsettled: bool,
) -> Strength {
    let Some(before) = before else {
        return Strength::Weak;
    };
    if conversions {
        let n = now
            .conversions
            .unwrap_or_default()
            .min(before.conversions.unwrap_or_default());
        if tracking != ConversionTracking::Working || n < MIN_CONVERSIONS {
            Strength::Weak
        } else if n < SURE_CONVERSIONS || unsettled {
            Strength::Limited
        } else {
            Strength::Strong
        }
    } else if now.clicks >= tracking::STOPPED_CLICKS && before.clicks >= tracking::STOPPED_CLICKS {
        Strength::Strong
    } else {
        Strength::Limited
    }
}

fn counts(m: &Metrics) -> Counts {
    Counts {
        cost: m.spend,
        clicks: m.clicks,
        impressions: m.impressions,
        conversions: m.conversions.unwrap_or_default(),
        value: m.conversion_value.unwrap_or_default(),
    }
}

fn metrics_of(c: &Counts) -> Metrics {
    Metrics {
        spend: c.cost,
        clicks: c.clicks,
        impressions: c.impressions,
        conversions: Some(c.conversions),
        conversion_value: Some(c.value),
    }
}

fn window(period: Period) -> Window {
    match period {
        Period::Last7Days => Window::SevenDays,
        Period::Last30Days => Window::ThirtyDays,
    }
}

fn day(day: i64) -> String {
    crate::workspace::history::date_of(day).map_or_else(String::new, |date| {
        crate::language::english().written.format.day_month(date)
    })
}

fn dates(end: i64, length: i64) -> Days {
    Days {
        start: end - length + 1,
        end,
    }
}

struct Builder<'a> {
    account: &'a ScopedAccount,
    data: &'a AccountData,
    period: Days,
    baseline: Option<Days>,
    current: Metrics,
    previous: Option<Metrics>,
    tracking_problem: bool,
}

impl<'a> Builder<'a> {
    fn new(
        account: &'a ScopedAccount,
        data: &'a AccountData,
        period: Period,
        baseline: Baseline,
        campaign: Option<i64>,
    ) -> Self {
        let length = window(period).days();
        let period = dates(data.end_day, length);
        let before = dates(
            match baseline {
                Baseline::PreviousPeriod => period.start - 1,
                Baseline::LastYear => data.end_day - 364,
            },
            length,
        );
        Self::for_days(account, data, period, before, campaign)
    }

    fn for_days(
        account: &'a ScopedAccount,
        data: &'a AccountData,
        period: Days,
        before: Days,
        campaign: Option<i64>,
    ) -> Self {
        let baseline = data
            .first_day
            .filter(|first| *first <= before.start)
            .map(|_| before);
        let current = metrics::sum(&data.days, period.start, period.end, campaign);
        let previous = baseline.map(|b| metrics::sum(&data.days, b.start, b.end, campaign));
        Self {
            account,
            data,
            period,
            baseline,
            current,
            previous,
            tracking_problem: data.rules.account().state == ConversionTracking::NotSetUp
                || tracking::stopped(&data.days, data.end_day).is_some()
                || tracking::spike(&data.days, data.end_day).is_some(),
        }
    }

    fn evidence(
        &self,
        source: Source,
        subject: Subject,
        facts: Value,
        conversions: bool,
    ) -> Evidence {
        let unsettled = self.period.end > metrics::last_settled_conversion_day(self.data.end_day);
        let mut caveats = Vec::new();
        if self.baseline.is_none() {
            caveats.push(Caveat::NoBaseline);
        }
        if conversions {
            if unsettled {
                caveats.push(Caveat::ConversionsNotSettled {
                    days: self.period.end - metrics::last_settled_conversion_day(self.data.end_day),
                });
            }
            if self.tracking_problem {
                caveats.push(Caveat::TrackingNotWorking);
            }
            let current = self.current.conversions.unwrap_or_default();
            let n = self
                .previous
                .as_ref()
                .and_then(|m| m.conversions)
                .map_or(current, |before| current.min(before));
            if n < MIN_CONVERSIONS {
                caveats.push(Caveat::FewConversions {
                    conversions: n.round() as i64,
                });
            }
        }
        let mut row = Evidence {
            id: EvidenceId::new(0, 0),
            account: self.account.handle,
            source,
            subject,
            days: Some(self.period),
            baseline: self.baseline,
            facts,
            strength: strength(
                &self.current,
                self.previous.as_ref(),
                self.reliability_state(),
                conversions,
                unsettled,
            ),
            caveats,
        };
        display_periods(&mut row);
        row
    }

    fn metrics(&self, subject: Subject, judged: bool) -> Evidence {
        let now = counts(&self.current);
        let mut facts = assessment::block(&now, &self.data.currency, judged, now.judged_by_value());
        let displayed = metrics::formatted(&self.current, &self.data.currency);
        facts
            .as_object_mut()
            .unwrap()
            .extend(displayed.as_object().unwrap().clone());
        if let Some(before) = &self.previous {
            facts["Baseline"] = assessment::block(
                &counts(before),
                &self.data.currency,
                judged,
                now.judged_by_value(),
            );
            facts["Baseline"]["Conversions"] =
                metrics::english_count(before.conversions.unwrap_or_default()).into();
            facts["Changes"] =
                assessment::changes(&now, &counts(before), judged, now.judged_by_value());
        }
        facts["Conversions judged"] = judged.into();
        let conversions =
            judged || self.data.rules.account().state == ConversionTracking::TooFewResults;
        self.evidence(Source::AccountHistory, subject, facts, conversions)
    }

    fn reliability_state(&self) -> ConversionTracking {
        if self.tracking_problem {
            ConversionTracking::NotSetUp
        } else {
            self.data.rules.account().state
        }
    }

    fn tracking_state(&self) -> Value {
        if tracking::stopped(&self.data.days, self.data.end_day).is_some() {
            json!("stopped")
        } else if tracking::spike(&self.data.days, self.data.end_day).is_some() {
            json!("spike")
        } else {
            json!(self.data.rules.account().state)
        }
    }

    fn account_metrics(&self) -> Evidence {
        let mut row = self.metrics(Subject::AdAccount, self.data.rules.judged());
        row.facts["Ad Account"] = self.account.name.clone().into();
        row.facts["Conversion tracking"] = self.tracking_state();
        row.facts["Goal"] = self.data.profile.goal.clone().into();
        row.facts["Note"] = self.data.profile.note.clone().into();
        row.facts["Target cost per conversion"] = self
            .data
            .profile
            .target_cost
            .map(|n| metrics::english_money(n, &self.data.currency))
            .into();
        let economics = &self.data.profile.economics;
        for (label, value) in [
            (
                "Target return on ad spend",
                economics
                    .target_return
                    .map(|n| metrics::english_number(n, 2)),
            ),
            (
                "Break-even cost per conversion",
                economics
                    .break_even_cost()
                    .map(|n| metrics::english_money(n, &self.data.currency)),
            ),
            (
                "Break-even return on ad spend",
                economics
                    .break_even_return()
                    .map(|n| metrics::english_number(n, 2)),
            ),
        ] {
            if let Some(value) = value {
                row.facts[label] = value.into();
            }
        }
        row
    }

    fn snapshot(
        &self,
        day: i64,
        subject: Subject,
        facts: Value,
        current: &Metrics,
        previous: Option<&Metrics>,
        conversions: bool,
    ) -> Evidence {
        let mut row = self.evidence(Source::DeepCheck { day }, subject, facts, conversions);
        row.days = Some(dates(day - 1, 30));
        row.baseline = previous.map(|_| dates(day - 31, 30));
        row.caveats.clear();
        row.caveats.push(Caveat::Snapshot { day });
        if previous.is_none() {
            row.caveats.push(Caveat::NoBaseline);
        }
        let unsettled = day - 1 > metrics::last_settled_conversion_day(self.data.end_day);
        if conversions {
            if unsettled {
                row.caveats.push(Caveat::ConversionsNotSettled {
                    days: day - 1 - metrics::last_settled_conversion_day(self.data.end_day),
                });
            }
            if self.tracking_problem {
                row.caveats.push(Caveat::TrackingNotWorking);
            }
            let conversions = current.conversions.unwrap_or_default();
            let n = previous
                .and_then(|m| m.conversions)
                .map_or(conversions, |before| conversions.min(before));
            if n < MIN_CONVERSIONS {
                row.caveats.push(Caveat::FewConversions {
                    conversions: n.round() as i64,
                });
            }
        }
        row.strength = strength(
            current,
            previous,
            self.reliability_state(),
            conversions,
            unsettled,
        );
        display_periods(&mut row);
        row
    }

    fn subject(&self, id: Option<i64>) -> Subject {
        id.map_or(Subject::AdAccount, |id| Subject::Campaign {
            id,
            name: self
                .data
                .campaigns
                .iter()
                .find(|c| c.id == id)
                .map_or_else(|| format!("Campaign {id}"), |c| c.name.clone()),
        })
    }

    fn finding(&self, finding: &crate::monitoring::deep_check::Finding, active: bool) -> Evidence {
        let facts = json!({"Finding":finding.rule.title,"Evidence":finding.rule.detail,"Spend":metrics::english_money(finding.rule.spend,&self.data.currency),"State":if active {"active"} else {"resolved"}});
        let mut row = if let Some(deep) = self.data.deep_check.as_ref().filter(|_| active) {
            let audit = finding.campaign_id.and_then(|id| deep.by_campaign.get(&id));
            let current = audit
                .and_then(|a| a.campaigns.first())
                .map_or(&deep.data.reporting, |r| &r.counts);
            let previous = if finding.campaign_id.is_some() {
                audit
                    .and_then(|a| a.campaigns.first())
                    .and_then(|row| row.previous.as_ref())
            } else {
                deep.data.audit.previous.as_ref()
            };
            self.snapshot(
                deep.day,
                self.subject(finding.campaign_id),
                facts,
                &metrics_of(current),
                previous.map(metrics_of).as_ref(),
                true,
            )
        } else {
            let mut row = self.evidence(
                Source::RecommendationHistory,
                self.subject(finding.campaign_id),
                facts,
                true,
            );
            row.days = None;
            row.baseline = None;
            row.strength = Strength::Weak;
            row.caveats = vec![Caveat::NoBaseline];
            row
        };
        row.source = Source::Finding {
            key: finding.rule.key.clone(),
        };
        if self.tracking_problem && !row.caveats.contains(&Caveat::TrackingNotWorking) {
            row.caveats.push(Caveat::TrackingNotWorking);
        }
        display_periods(&mut row);
        row
    }

    fn findings(&self) -> Vec<Evidence> {
        let mut findings = self.data.findings.iter().collect::<Vec<_>>();
        findings.sort_by(|a, b| b.rule.spend.total_cmp(&a.rule.spend));
        findings
            .into_iter()
            .map(|f| self.finding(f, true))
            .collect()
    }
}

fn display_periods(row: &mut Evidence) {
    row.facts.as_object_mut().unwrap().remove("Period");
    row.facts.as_object_mut().unwrap().remove("Baseline period");
    let display = |days: Days| {
        crate::workspace::history::date_of(days.start)
            .zip(crate::workspace::history::date_of(days.end))
            .map(|(start, end)| {
                crate::language::english()
                    .written
                    .format
                    .date_range(start, end, true)
            })
    };
    if let Some(period) = row.days.and_then(display) {
        row.facts["Period"] = period.into();
    }
    if let Some(period) = row.baseline.and_then(display) {
        row.facts["Baseline period"] = period.into();
    }
}

fn bounded(mut rows: Vec<Evidence>, limit: usize) -> Vec<Evidence> {
    let left = rows.len().saturating_sub(limit);
    rows.truncate(limit);
    if left > 0 {
        for row in &mut rows {
            row.caveats.push(Caveat::RowsLeftOut { rows: left });
        }
    }
    rows
}

fn bound_facts(facts: &mut Value) -> usize {
    match facts {
        Value::Array(rows) => {
            let left = rows.len().saturating_sub(MAX_ROWS as usize);
            rows.truncate(MAX_ROWS as usize);
            left + rows.iter_mut().map(bound_facts).sum::<usize>()
        }
        Value::Object(fields) => fields.values_mut().map(bound_facts).sum(),
        _ => 0,
    }
}

fn authorized<'a>(
    scope: &'a Scope,
    data: &'a BTreeMap<Handle, AccountData>,
    handle: Handle,
) -> Result<(&'a ScopedAccount, &'a AccountData), Refused> {
    let account = scope
        .resolve(handle)
        .ok_or(Refused::UnknownAdAccount(handle))?;
    let snapshot = data
        .get(&handle)
        .filter(|d| d.customer_id == account.customer_id)
        .ok_or(Refused::MissingData(handle))?;
    Ok((account, snapshot))
}

pub fn run(
    op: &Operation,
    step: usize,
    scope: &Scope,
    data: &BTreeMap<Handle, AccountData>,
) -> Result<Vec<Evidence>, Refused> {
    let rows = if let Operation::RankAccounts { period } = op {
        let mut ranked = Vec::new();
        for account in scope.accounts() {
            let (_, d) = authorized(scope, data, account.handle)?;
            let b = Builder::new(account, d, *period, Baseline::PreviousPeriod, None);
            let mut row = b.metrics(Subject::AdAccount, d.rules.judged());
            row.facts["Ad Account"] = account.name.clone().into();
            row.facts["Conversion tracking"] = b.tracking_state();
            let findings = bounded(b.findings(), MAX_ROWS as usize);
            if !findings.is_empty() {
                let reliability =
                    b.evidence(Source::AccountHistory, Subject::AdAccount, json!({}), true);
                row.strength = row.strength.min(reliability.strength);
                for finding in &findings {
                    row.strength = row.strength.min(finding.strength);
                    for caveat in &finding.caveats {
                        if !row.caveats.contains(caveat) {
                            row.caveats.push(caveat.clone());
                        }
                    }
                }
                for caveat in reliability.caveats {
                    if !row.caveats.contains(&caveat) {
                        row.caveats.push(caveat);
                    }
                }
            }
            row.facts["Active Findings"] = json!(findings
                .iter()
                .map(|e| json!({"Subject":e.subject,"Facts":e.facts}))
                .collect::<Vec<_>>());
            if let Some(deep) = d.deep_check.as_ref().filter(|_| !d.findings.is_empty()) {
                row.caveats.push(Caveat::Snapshot { day: deep.day });
            }
            if d.findings.len() > MAX_ROWS as usize {
                row.caveats.push(Caveat::RowsLeftOut {
                    rows: d.findings.len() - MAX_ROWS as usize,
                });
            }
            ranked.push((b.current.spend, row));
        }
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        ranked.into_iter().map(|(_, row)| row).collect()
    } else {
        let (account, d) = authorized(scope, data, op.account().unwrap())?;
        let (period, baseline) = match op {
            Operation::AccountOverview { period, .. }
            | Operation::CompareCampaigns { period, .. }
            | Operation::ComparePeriods { period, .. } => (*period, Baseline::PreviousPeriod),
            _ => (Period::Last30Days, Baseline::PreviousPeriod),
        };
        let b = Builder::new(account, d, period, baseline, None);
        match op {
            Operation::AccountOverview { .. } => {
                let row = b.account_metrics();
                bounded(
                    std::iter::once(row).chain(b.findings()).collect(),
                    MAX_ROWS as usize,
                )
            }
            Operation::ComparePeriods { .. } => {
                let mut row = b.account_metrics();
                row.facts["Comparison"] = "previous period".into();
                // Detection has a fixed, settled seven-day Window; label it independently.
                row.facts["Daily detection window"] =
                    "last 7 days; conversions end on the last settled day".into();
                row.facts["Movements"] = json!(d.detection.movements.iter().map(|m| json!({"Metric":m.metric.as_str(),"Current": if m.metric == crate::monitoring::detection::MetricName::Spend {metrics::english_money(m.current, &d.currency)} else {metrics::english_count(m.current)},"Previous":if m.metric == crate::monitoring::detection::MetricName::Spend {metrics::english_money(m.previous, &d.currency)} else {metrics::english_count(m.previous)}})).collect::<Vec<_>>());
                let yearly = Builder::new(account, d, period, Baseline::LastYear, None);
                let mut last_year = yearly.account_metrics();
                last_year.facts["Comparison"] = "same days last year".into();
                let length = window(period).days();
                let year_end = d.end_day - 364;
                let year = Builder::for_days(
                    account,
                    d,
                    dates(year_end, length),
                    dates(year_end - length, length),
                    None,
                );
                if yearly.baseline.is_some() {
                    let mut history = year.account_metrics();
                    display_periods(&mut history);
                    last_year.facts["Last year against its previous period"] = history.facts;
                }
                let mut rows = vec![row, last_year];
                let mut contributions = d.detection.campaigns.iter().collect::<Vec<_>>();
                contributions.sort_by(|a, b| {
                    (b.metrics.current.spend - b.metrics.previous.spend)
                        .abs()
                        .total_cmp(&(a.metrics.current.spend - a.metrics.previous.spend).abs())
                });
                for campaign in contributions {
                    let cb = Builder::new(
                        account,
                        d,
                        Period::Last7Days,
                        Baseline::PreviousPeriod,
                        Some(campaign.campaign_id),
                    );
                    let mut row = cb.metrics(
                        cb.subject(Some(campaign.campaign_id)),
                        d.rules.campaign_judged(campaign.campaign_id),
                    );
                    row.facts["Role"] = "Contribution, not a Cause".into();
                    row.facts["Window"] =
                        "last 7 days; conversions shown separately on settled days".into();
                    let end = metrics::last_settled_conversion_day(d.end_day);
                    let settled = Builder::for_days(
                        account,
                        d,
                        dates(end, 7),
                        dates(end - 7, 7),
                        Some(campaign.campaign_id),
                    );
                    if d.rules.campaign_judged(campaign.campaign_id) {
                        row.facts["Settled conversion window"] = settled
                            .metrics(cb.subject(Some(campaign.campaign_id)), true)
                            .facts;
                    }
                    rows.push(row);
                }
                let mut candidates = d.detection.causes.iter().collect::<Vec<_>>();
                let contribution = |cause: &crate::monitoring::causes::Candidate| {
                    let metrics = cause
                        .campaign_id
                        .and_then(|id| d.detection.campaigns.iter().find(|c| c.campaign_id == id))
                        .map_or(&d.detection.metrics, |c| &c.metrics);
                    (cause.metric.value(&metrics.current) - cause.metric.value(&metrics.previous))
                        .abs()
                };
                candidates.sort_by(|a, b| contribution(b).total_cmp(&contribution(a)));
                for cause in candidates {
                    let conversions =
                        cause.metric == crate::monitoring::detection::MetricName::Conversions;
                    let end = if conversions {
                        metrics::last_settled_conversion_day(d.end_day)
                    } else {
                        d.end_day
                    };
                    let cb = Builder::for_days(
                        account,
                        d,
                        dates(end, 7),
                        dates(end - 7, 7),
                        cause.campaign_id,
                    );
                    rows.push(cb.evidence(Source::AccountHistory, cb.subject(cause.campaign_id), json!({"Window":"last 7 settled days for conversions; last 7 days for traffic", "Level":cause.level, "Evidence":cause.evidence, "Anchors":cause.anchors}), conversions));
                }
                bounded(rows, MAX_ROWS as usize)
            }
            Operation::CompareCampaigns { order, rows, .. } => {
                let mut ranked = Vec::new();
                for campaign in &d.campaigns {
                    let cb = Builder::new(account, d, period, baseline, Some(campaign.id));
                    let before = cb.previous.as_ref().cloned().unwrap_or_default();
                    let key = match order {
                        CampaignOrder::Spend => cb.current.spend,
                        CampaignOrder::SpendChange => (cb.current.spend - before.spend).abs(),
                        CampaignOrder::ConversionChange => {
                            (cb.current.conversions.unwrap_or_default()
                                - before.conversions.unwrap_or_default())
                            .abs()
                        }
                        CampaignOrder::CostPerConversionChange => metrics::ratio(
                            cb.current.spend,
                            cb.current.conversions.unwrap_or_default(),
                        )
                        .zip(metrics::ratio(
                            before.spend,
                            before.conversions.unwrap_or_default(),
                        ))
                        .map_or(0.0, |(a, b)| (a - b).abs()),
                    };
                    ranked.push((
                        key,
                        cb.metrics(
                            Subject::Campaign {
                                id: campaign.id,
                                name: campaign.name.clone(),
                            },
                            d.rules.campaign_judged(campaign.id),
                        ),
                    ));
                }
                ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
                bounded(
                    std::iter::once(b.account_metrics())
                        .chain(ranked.into_iter().map(|(_, row)| row))
                        .collect(),
                    rows.get(),
                )
            }
            Operation::InvestigateSearchTerms {
                campaign,
                focus,
                rows,
                ..
            } => {
                if let Some(id) = campaign {
                    if !d.campaigns.iter().any(|c| c.id == *id) {
                        return Err(Refused::UnknownCampaign(*id));
                    }
                }
                let deep = d.deep_check.as_ref().ok_or(Refused::NoDeepCheck)?;
                let audit = match campaign {
                    Some(id) => deep
                        .by_campaign
                        .get(id)
                        .ok_or(Refused::NoCampaignDeepCheck(*id))?,
                    None => &deep.data.audit,
                };
                let mut terms = audit
                    .search_terms
                    .iter()
                    .filter(|term| {
                        *focus != SearchTermFocus::SpendWithoutConversions
                            || term.counts.conversions == 0.0
                    })
                    .collect::<Vec<_>>();
                terms.sort_by(|a, b| b.counts.cost.total_cmp(&a.counts.cost));
                let aggregate = *focus == SearchTermFocus::SpendWithoutConversions;
                let available = terms.len();
                let limit = if aggregate {
                    rows.get().saturating_sub(1).max(1)
                } else {
                    rows.get()
                };
                terms.truncate(limit);
                let mut listed = Metrics::default();
                for term in &terms {
                    listed.add(&metrics_of(&term.counts));
                }
                let total_spend = if campaign.is_some() {
                    audit.campaigns.iter().map(|r| r.counts.cost).sum()
                } else {
                    deep.data.reporting.cost
                };
                let coverage = metrics::ratio(listed.spend, total_spend);
                let incomplete = Caveat::SearchTermsIncomplete {
                    shown: terms.len(),
                    of_spend_share: coverage
                        .map(|share| (share * 100.0).round().clamp(0.0, 100.0) as u8),
                };
                let mut result = Vec::new();
                if aggregate {
                    let mut facts = assessment::block(&counts(&listed), &d.currency, true, false);
                    facts["Selection"] = "Listed zero-conversion search terms".into();
                    facts["Share of spend"] = assessment::percent(listed.spend, total_spend).into();
                    facts["Search terms"] = json!(terms.iter().map(|term| json!({"Term":redact_search_term(&term.name),"Spend":metrics::english_money(term.counts.cost, &d.currency),"Clicks":metrics::english_count(term.counts.clicks),"Conversions":"0"})).collect::<Vec<_>>());
                    if let Some(id) = campaign {
                        facts["Campaign"] = json!(b.subject(Some(*id)));
                    }
                    let mut row =
                        b.snapshot(deep.day, b.subject(*campaign), facts, &listed, None, false);
                    row.strength =
                        strength(&listed, Some(&listed), b.reliability_state(), false, false);
                    result.push(row);
                }
                if !aggregate || rows.get() > 1 {
                    for term in &terms {
                        let mut facts = assessment::block(
                            &term.counts,
                            &d.currency,
                            d.rules.judged(),
                            term.counts.judged_by_value(),
                        );
                        facts["Conversions"] =
                            metrics::english_count(term.counts.conversions).into();
                        if let Some(id) = campaign {
                            facts["Campaign"] = json!(b.subject(Some(*id)));
                        }
                        facts["Collection"] = "Top 200 search terms by spend over 30 days".into();
                        let traffic = term.counts.conversions == 0.0
                            && (aggregate || term.counts.clicks >= tracking::STOPPED_CLICKS);
                        let current = metrics_of(&term.counts);
                        let mut row = b.snapshot(
                            deep.day,
                            Subject::SearchTerm {
                                text: redact_search_term(&term.name),
                            },
                            facts,
                            &current,
                            term.previous.as_ref().map(metrics_of).as_ref(),
                            d.rules.judged() && !traffic,
                        );
                        if traffic {
                            row.strength = strength(
                                &current,
                                Some(&current),
                                b.reliability_state(),
                                false,
                                false,
                            );
                        }
                        result.push(row);
                    }
                }
                for row in &mut result {
                    row.caveats.push(incomplete.clone());
                    if available > terms.len() {
                        row.caveats.push(Caveat::RowsLeftOut {
                            rows: available - terms.len(),
                        });
                    }
                }
                bounded(result, rows.get())
            }
            Operation::InvestigateSegments { segment, .. } => segments(&b, *segment)?,
            Operation::ReviewEconomics { .. } => {
                let mut row = b.evidence(
                    Source::AdAccountProfile,
                    Subject::AdAccount,
                    d.profile.economics.facts(&d.currency),
                    false,
                );
                row.days = None;
                row.baseline = None;
                row.caveats.clear();
                row.strength = if d.profile.economics.is_empty() {
                    Strength::Weak
                } else {
                    Strength::Strong
                };
                for (input, value) in [
                    ("margin", d.profile.economics.margin),
                    ("customer_value", d.profile.economics.customer_value),
                    ("close_rate", d.profile.economics.close_rate),
                    ("target_return", d.profile.economics.target_return),
                ] {
                    if value.is_none() {
                        row.caveats.push(Caveat::MissingInput {
                            input: input.into(),
                        });
                    }
                }
                vec![row]
            }
            Operation::BusinessContext { .. } => {
                let deep = d.deep_check.as_ref().ok_or(Refused::NoDeepCheck)?;
                let business = Business {
                    research: deep.research.clone(),
                    ..Default::default()
                };
                let researched_day = business.research.as_ref().map(|r| r.day);
                let mut row = b.evidence(
                    Source::BusinessContext { researched_day },
                    Subject::AdAccount,
                    json!({"Verified facts":business.verified_facts()}),
                    false,
                );
                row.days = None;
                row.baseline = None;
                row.caveats = vec![Caveat::Snapshot { day: deep.day }];
                row.strength = strength(&b.current, None, d.rules.account().state, false, false);
                vec![row]
            }
            Operation::InvestigateConversions { .. } => conversions(&b),
            Operation::ReviewHistory { .. } => history(&b),
            Operation::RankAccounts { .. } => unreachable!(),
        }
    };
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(n, mut row)| {
            row.id = EvidenceId::new(step, n + 1);
            display_periods(&mut row);
            let left = bound_facts(&mut row.facts);
            if left > 0 {
                row.caveats.push(Caveat::RowsLeftOut { rows: left });
            }
            row
        })
        .collect())
}

fn segments(b: &Builder<'_>, segment: Segment) -> Result<Vec<Evidence>, Refused> {
    let d = b.data;
    let mut values: Vec<(String, Metrics, Option<Metrics>)> = match segment {
        Segment::Device => d
            .devices
            .iter()
            .map(|r| {
                (
                    crate::monitoring::causes::device_name(r.device).into(),
                    r.current.clone(),
                    r.previous.clone(),
                )
            })
            .collect(),
        Segment::Location => d
            .locations
            .iter()
            .map(|r| (r.name.clone(), metrics_of(&r.counts), None))
            .collect(),
        Segment::Hour => d
            .hours
            .iter()
            .map(|r| {
                (
                    format!("Weekday {}, hour {}", r.weekday, r.hour),
                    metrics_of(&r.counts),
                    None,
                )
            })
            .collect(),
        Segment::Demographic => d
            .demographics
            .iter()
            .map(|(kind, r)| (format!("{kind}: {}", r.value), metrics_of(&r.counts), None))
            .collect(),
    };
    let deep = if segment == Segment::Device {
        None
    } else {
        Some(d.deep_check.as_ref().ok_or(Refused::NoDeepCheck)?)
    };
    let mut total = Metrics::default();
    let mut before_total = Metrics::default();
    for (_, m, before) in &values {
        total.add(m);
        if let Some(before) = before {
            before_total.add(before);
        }
    }
    let by_value = counts(&b.current).judged_by_value();
    let facts = |m: &Metrics, total: &Metrics| {
        let mut facts = assessment::block(&counts(m), &d.currency, d.rules.judged(), by_value);
        facts["Conversions"] = metrics::english_count(m.conversions.unwrap_or_default()).into();
        facts["Conversions judged"] = d.rules.judged().into();
        facts["Share of spend"] = assessment::percent(m.spend, total.spend).into();
        facts["Share of conversions"] = assessment::percent(
            m.conversions.unwrap_or_default(),
            total.conversions.unwrap_or_default(),
        )
        .into();
        facts
    };
    values.sort_by(|a, b| b.1.spend.total_cmp(&a.1.spend));
    Ok(bounded(
        values
            .into_iter()
            .map(|(name, m, previous)| {
                let mut shown = facts(&m, &total);
                let subject = Subject::Segment { name };
                if let Some(deep) = deep {
                    b.snapshot(deep.day, subject, shown, &m, None, d.rules.judged())
                } else {
                    let baseline = previous.as_ref().and(b.baseline);
                    if let Some(before) = &previous {
                        shown["Baseline"] = facts(before, &before_total);
                        shown["Changes"] = assessment::changes(
                            &counts(&m),
                            &counts(before),
                            d.rules.judged(),
                            by_value,
                        );
                    }
                    let device = Builder {
                        account: b.account,
                        data: d,
                        period: b.period,
                        baseline,
                        current: m,
                        previous,
                        tracking_problem: b.tracking_problem,
                    };
                    device.evidence(
                        Source::AccountHistory,
                        subject,
                        shown,
                        d.rules.judged()
                            || d.rules.account().state == ConversionTracking::TooFewResults,
                    )
                }
            })
            .collect(),
        MAX_ROWS as usize,
    ))
}

fn conversions(b: &Builder<'_>) -> Vec<Evidence> {
    let d = b.data;
    let mut summary = b.evidence(Source::ConversionTracking, Subject::AdAccount, json!({
        "Conversion tracking": b.tracking_state(),
        "Judged by conversions": d.rules.judged(),
        "Enabled actions": d.deep_check.as_ref().and_then(|c| c.data.audit.conversion_actions).map(|n| metrics::english_count(f64::from(n))),
        "Last settled day": day(metrics::last_settled_conversion_day(d.end_day)),
    }), true);
    if let Some(stop) = tracking::stopped(&d.days, d.end_day) {
        summary.facts["Stopped"] = json!({"Since":day(stop.since),"Days":stop.days.to_string(),"Clicks":metrics::english_count(stop.clicks),"Spend":metrics::english_money(stop.cost,&d.currency),"Conversions before":metrics::english_count(stop.before)});
    }
    if let Some(spike) = tracking::spike(&d.days, d.end_day) {
        summary.facts["Spike"] = json!({"Since":day(spike.since),"Days":spike.days.to_string(),"Conversion rate":format!("{}%",metrics::english_number(spike.rate*100.0,2)),"Usual conversion rate":format!("{}%",metrics::english_number(spike.usual*100.0,2))});
    }
    if let Some((total, late)) = d.conversion_delay {
        summary.facts["Conversion delay collection"] = json!({"Conversions":metrics::english_count(total),"Conversions after two days":metrics::english_count(late)});
    }
    if let Some(deep) = &d.deep_check {
        summary.caveats.push(Caveat::Snapshot { day: deep.day });
        if let Some(lag) = tracking::lag(&deep.data.audit.lag) {
            summary.facts["Conversion delay"] = json!({"After a week":format!("{}%",metrics::english_number(lag.late*100.0,0)),"Last 30 days not recorded yet":format!("{}%",metrics::english_number(lag.missing.0*100.0,0)),"Previous 30 days not recorded yet":format!("{}%",metrics::english_number(lag.missing.1*100.0,0))});
        }
    }
    let mut weeks = d.action_weeks.iter().collect::<Vec<_>>();
    weeks.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut rows = vec![summary];
    for (week, name, n) in weeks {
        let mut row = b.evidence(Source::ConversionTracking, Subject::ConversionAction { name:name.clone() }, json!({"Week starting":day(d.end_day-55+week*7),"Conversions":metrics::english_count(*n)}),true);
        row.days = Some(dates((d.end_day - 55 + week * 7 + 6).min(d.end_day), 7));
        row.baseline = None;
        row.strength = Strength::Weak;
        row.caveats.clear();
        row.caveats.push(Caveat::NoBaseline);
        if *n < MIN_CONVERSIONS {
            row.caveats.push(Caveat::FewConversions {
                conversions: n.round() as i64,
            });
        }
        if b.tracking_problem {
            row.caveats.push(Caveat::TrackingNotWorking);
        }
        let unsettled = row.days.unwrap().end - metrics::last_settled_conversion_day(d.end_day);
        if unsettled > 0 {
            row.caveats
                .push(Caveat::ConversionsNotSettled { days: unsettled });
        }
        rows.push(row);
    }
    bounded(rows, MAX_ROWS as usize)
}

fn history(b: &Builder<'_>) -> Vec<Evidence> {
    let d = b.data;
    let mut rows = Vec::new();
    if let Some(deep) = &d.deep_check {
        let mut assessment_data = deep.data.clone();
        assessment_data
            .audit
            .campaigns
            .sort_by(|a, b| b.counts.cost.total_cmp(&a.counts.cost));
        let result = assessment::judge(&deep.data, deep.judge_by_conversions, &deep.findings);
        let target = |id| d.rules.target(id);
        let facts = assessment::facts(&assessment::Evidence {
            data: &assessment_data,
            judged: deep.judge_by_conversions,
            result: &result,
            campaigns: &d.campaigns,
            target: &target,
        });
        let mut row = b.snapshot(
            deep.day,
            Subject::AdAccount,
            facts,
            &metrics_of(&deep.data.reporting),
            deep.data.audit.previous.as_ref().map(metrics_of).as_ref(),
            deep.judge_by_conversions,
        );
        row.source = Source::Assessment { day: deep.day };
        if let Some(assessment) = &deep.assessment {
            row.facts["Assessment"] = json!(assessment);
        }
        rows.push(row);
    }
    let mut findings = d.finding_history.iter().collect::<Vec<_>>();
    findings.sort_by(|a, b| b.0.rule.spend.total_cmp(&a.0.rule.spend));
    for (finding, active) in findings {
        rows.push(b.finding(finding, *active));
    }

    let mut earlier = d.earlier.iter().collect::<Vec<_>>();
    earlier.sort_by(|a, b| {
        let spend = |key: &str| {
            d.recommendations
                .iter()
                .find(|r| r.recommendation.finding_key == key)
                .and_then(|r| r.finding.as_ref())
                .map_or(0.0, |f| f.spend)
        };
        spend(&b.recommendation.finding_key)
            .total_cmp(&spend(&a.recommendation.finding_key))
            .then_with(|| b.opened_day.cmp(&a.opened_day))
    });
    for earlier in earlier {
        let recommendation = &earlier.recommendation;
        let name = b.subject(recommendation.campaign_id);
        let mut row = b.evidence(
            Source::RecommendationHistory,
            Subject::Recommendation {
                finding_key: recommendation.finding_key.clone(),
            },
            earlier.facts(
                &d.days,
                d.end_day,
                name.name().unwrap_or(&d.name),
                &d.currency,
            ),
            true,
        );
        row.facts["Campaign"] = json!(name);
        let last = metrics::last_settled_conversion_day(d.end_day);
        if last - earlier.opened_day + 1 >= crate::monitoring::earlier::FOLLOW_DAYS
            && recommendation.campaign_id.is_some()
        {
            row.days = Some(Days {
                start: earlier.opened_day,
                end: last,
            });
            let before = dates(earlier.opened_day - 1, 30);
            row.baseline = d
                .first_day
                .filter(|first| *first <= before.start)
                .map(|_| before);
            let now = metrics::sum(
                &d.days,
                earlier.opened_day,
                last,
                recommendation.campaign_id,
            );
            let previous = row
                .baseline
                .map(|p| metrics::sum(&d.days, p.start, p.end, recommendation.campaign_id));
            row.strength = strength(&now, previous.as_ref(), b.reliability_state(), true, false);
            row.caveats.clear();
            if row.baseline.is_none() {
                row.caveats.push(Caveat::NoBaseline);
            }
            if b.tracking_problem {
                row.caveats.push(Caveat::TrackingNotWorking);
            }
            let current = now.conversions.unwrap_or_default();
            let n = previous
                .as_ref()
                .and_then(|m| m.conversions)
                .map_or(current, |before| current.min(before));
            if n < MIN_CONVERSIONS {
                row.caveats.push(Caveat::FewConversions {
                    conversions: n.round() as i64,
                });
            }
        } else {
            row.days = None;
            row.baseline = None;
            row.caveats = vec![Caveat::NoBaseline];
            row.strength = Strength::Weak;
        }
        if b.tracking_problem && !row.caveats.contains(&Caveat::TrackingNotWorking) {
            row.caveats.push(Caveat::TrackingNotWorking);
        }
        rows.push(row);
    }
    let names = d.campaigns.iter().map(|c| (c.id, c.name.clone())).collect();
    for follow in follow_up::follow_ups(
        &d.changes,
        &d.days,
        &names,
        &d.rules,
        &d.currency,
        d.end_day + 1,
    ) {
        let mut facts = json!({"Account change":follow.change,"Day":day(follow.change_day),"Follow-up due":day(follow.due_day)});
        if let Some(checked) = &follow.checked {
            facts["Follow-up"] = follow_up::facts(std::slice::from_ref(checked), &d.currency);
        }
        let fb = Builder::for_days(
            b.account,
            d,
            Days {
                start: follow.change_day + 1,
                end: follow.change_day + follow_up::FOLLOW_UP_DAYS,
            },
            dates(follow.change_day - 1, follow_up::FOLLOW_UP_DAYS),
            Some(follow.campaign_id),
        );
        let mut row = fb.evidence(
            Source::AccountChanges,
            b.subject(Some(follow.campaign_id)),
            facts,
            d.rules.campaign_judged(follow.campaign_id),
        );
        if follow.checked.is_none() {
            row.days = None;
            row.baseline = None;
            row.strength = Strength::Weak;
            row.caveats = vec![Caveat::NoBaseline];
        }
        if d.rules.campaign_judged(follow.campaign_id)
            && b.tracking_problem
            && !row.caveats.contains(&Caveat::TrackingNotWorking)
        {
            row.caveats.push(Caveat::TrackingNotWorking);
        }
        rows.push(row);
    }
    for change in &d.changes {
        let mut row=b.evidence(Source::AccountChanges,b.subject(change.campaign_id),json!({"Date":change.changed_at,"Changed fields":change.changed_fields,"Operation":change.operation,"Resource type":change.resource_type}),false);
        row.days = None;
        row.baseline = None;
        row.caveats.clear();
        row.strength = strength(&b.current, None, d.rules.account().state, false, false);
        rows.push(row);
    }
    let spend = |row: &Evidence| {
        let finding_key = match &row.source {
            Source::Finding { key } => Some(key.as_str()),
            _ => match &row.subject {
                Subject::Recommendation { finding_key } => Some(finding_key.as_str()),
                _ => None,
            },
        };
        if let Some(key) = finding_key {
            return d
                .finding_history
                .iter()
                .find(|(f, _)| f.rule.key == key)
                .map_or(0.0, |(f, _)| f.rule.spend);
        }
        let campaign = match row.subject {
            Subject::Campaign { id, .. } => Some(id),
            _ => None,
        };
        metrics::sum(&d.days, d.end_day - 29, d.end_day, campaign).spend
    };
    rows.sort_by(|a, b| spend(b).total_cmp(&spend(a)));
    bounded(rows, MAX_ROWS as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::{
        campaign_settings::{AccountDefaults, Resolved},
        earlier::Earlier,
        example,
    };
    use crate::workspace::{
        monitoring::{DemographicTotal, HourTotal, LocationTotal},
        profile::Profile,
    };

    fn fixture() -> (Scope, BTreeMap<Handle, AccountData>) {
        let end = example::today() - 1;
        let days = example::days();
        let campaigns = example::campaigns();
        let changes = example::changes();
        let rules = Resolved::new(
            AccountDefaults {
                state: ConversionTracking::Working,
                judged: true,
                target_cost: Some(50.0),
                alerts: true,
            },
            BTreeMap::new(),
        );
        let (mut deep, recommendations) = example::deep_check(end + 1);
        crate::monitoring::deep_check::compare(&mut deep.data, &mut deep.by_campaign, &days, end);
        let findings = deep.findings.clone();
        let detection = crate::monitoring::detection::detect(
            &days,
            &campaigns,
            &changes,
            end,
            end - 111,
            &rules,
            "USD",
        );
        let d = AccountData {
            customer_id: example::AD.into(),
            name: example::NAME.into(),
            currency: "USD".into(),
            end_day: end,
            first_day: Some(end - 111),
            days,
            campaigns,
            changes,
            rules,
            profile: Profile::default(),
            findings: findings.clone(),
            finding_history: findings.into_iter().map(|f| (f, true)).collect(),
            earlier: recommendations
                .into_iter()
                .map(|recommendation| Earlier {
                    recommendation,
                    opened_day: end - 20,
                    closed_day: None,
                    still_detected: true,
                })
                .collect(),
            recommendations: example::recommendations(),
            deep_check: Some(deep),
            detection,
            action_weeks: vec![(7, "Phone call".into(), 12.0)],
            devices: vec![super::super::data::DeviceData {
                device: 1,
                current: Metrics {
                    spend: 200.0,
                    clicks: 100.0,
                    conversions: Some(4.0),
                    ..Default::default()
                },
                previous: None,
            }],
            locations: vec![LocationTotal {
                id: 1,
                name: "Dublin".into(),
                counts: Counts {
                    cost: 200.0,
                    ..Default::default()
                },
            }],
            hours: vec![HourTotal {
                weekday: 1,
                hour: 9,
                counts: Counts {
                    cost: 200.0,
                    ..Default::default()
                },
            }],
            demographics: vec![(
                "age".into(),
                DemographicTotal {
                    value: "25-34".into(),
                    counts: Counts {
                        cost: 200.0,
                        ..Default::default()
                    },
                },
            )],
            conversion_delay: None,
        };
        (Scope::example(), BTreeMap::from([(Handle::new(0), d)]))
    }
    fn execute(value: Value) -> Vec<Evidence> {
        let (scope, data) = fixture();
        run(&serde_json::from_value(value).unwrap(), 3, &scope, &data).unwrap()
    }
    #[test]
    fn a_users_search_terms_keep_words_and_short_numbers_but_not_contact_details() {
        for (input, expected) in [
            ("dentist near me", "dentist near me"),
            ("open 24 hours 123456", "open 24 hours 123456"),
            ("help@example.com", "[email]"),
            ("Email me: help+ads@example.co.uk.", "Email me: [email]."),
            (
                "élise@example.fr and other@example.org",
                "[email] and [email]",
            ),
            ("call 1234567", "call [phone]"),
            ("call +44 (20) 7123-4567 nearby", "call [phone] nearby"),
            ("call (1234567)", "call [phone]"),
            ("(call 1234567)", "(call [phone])"),
            ("phone 020.7123.4567.", "phone [phone]."),
        ] {
            assert_eq!(redact_search_term(input), expected, "{input}");
        }
    }

    #[test]
    fn a_users_search_term_evidence_redacts_public_contact_details() {
        let (scope, mut data) = fixture();
        let deep = data
            .get_mut(&Handle::new(0))
            .unwrap()
            .deep_check
            .as_mut()
            .unwrap();
        let audit = deep.by_campaign.get_mut(&701).unwrap();
        audit.search_terms.truncate(1);
        audit.search_terms[0].name = "dentist help@example.com +44 (20) 7123-4567 nearby".into();
        audit.search_terms[0].counts.conversions = 0.0;
        for focus in ["top_spend", "spend_without_conversions"] {
            let op = serde_json::from_value(json!({"operation":"investigate_search_terms","account":"A1","campaign":701,"focus":focus,"rows":2})).unwrap();
            let rows = run(&op, 1, &scope, &data).unwrap();
            let expected = "dentist [email] [phone] nearby";
            assert_eq!(rows.last().unwrap().subject.name(), Some(expected));
            if focus == "spend_without_conversions" {
                assert_eq!(rows[0].facts["Search terms"][0]["Term"], expected);
            }
        }
    }

    #[test]
    fn the_user_sees_conversion_counts_when_they_choose_not_to_judge_them() {
        let (scope, mut data) = fixture();
        let d = data.get_mut(&Handle::new(0)).unwrap();
        let mut defaults = d.rules.account().clone();
        defaults.judged = false;
        d.rules = Resolved::new(defaults, BTreeMap::new());
        let op = serde_json::from_value(
            json!({"operation":"account_overview","account":"A1","period":"last_30_days"}),
        )
        .unwrap();
        let rows = run(&op, 1, &scope, &data).unwrap();
        assert!(
            rows[0].facts["Conversions"].is_string(),
            "conversion counts must remain visible"
        );
        assert_eq!(rows[0].facts["Conversions judged"], false);
        assert!(rows[0].facts.get("Cost per conversion").is_none());
    }

    #[test]
    fn the_user_can_compare_device_results_with_the_previous_period() {
        let (scope, mut data) = fixture();
        let d = data.get_mut(&Handle::new(0)).unwrap();
        d.devices[0].previous = Some(Metrics {
            spend: 100.0,
            clicks: 100.0,
            conversions: Some(4.0),
            ..Default::default()
        });
        let op = serde_json::from_value(
            json!({"operation":"investigate_segments","account":"A1","segment":"device"}),
        )
        .unwrap();
        let baseline = dates(d.end_day - 30, 30);
        let rows = run(&op, 1, &scope, &data).unwrap();
        assert_eq!(rows[0].facts["Cost per conversion"], "$50.00");
        assert_eq!(rows[0].facts["Baseline"]["Cost per conversion"], "$25.00");
        assert_eq!(rows[0].facts["Baseline"]["Share of spend"], "100%");
        assert_eq!(rows[0].baseline, Some(baseline));
        assert!(!rows[0].caveats.contains(&Caveat::NoBaseline));
    }

    #[test]
    fn the_user_sees_return_on_ad_spend_for_devices_of_a_value_judged_ad_account() {
        let (scope, mut data) = fixture();
        let d = data.get_mut(&Handle::new(0)).unwrap();
        for day in &mut d.days {
            day.metrics.conversion_value = Some(day.metrics.spend * 4.0);
        }
        d.devices[0].current.conversion_value = Some(600.0);
        let op = serde_json::from_value(
            json!({"operation":"investigate_segments","account":"A1","segment":"device"}),
        )
        .unwrap();
        let rows = run(&op, 1, &scope, &data).unwrap();
        assert_eq!(rows[0].facts["Return on ad spend"], "3.00");
    }

    #[test]
    fn the_user_sees_the_target_return_and_break_even_return_on_comparisons() {
        let (scope, mut data) = fixture();
        let d = data.get_mut(&Handle::new(0)).unwrap();
        d.profile.economics.margin = Some(0.25);
        d.profile.economics.target_return = Some(5.0);
        for operation in ["account_overview", "compare_periods", "compare_campaigns"] {
            let mut value = json!({"operation":operation,"account":"A1","period":"last_30_days"});
            if operation == "compare_campaigns" {
                value["order"] = json!("spend");
                value["rows"] = json!(20);
            }
            let rows = run(&serde_json::from_value(value).unwrap(), 1, &scope, &data).unwrap();
            assert_eq!(rows[0].facts["Target return on ad spend"], "5.00");
            assert_eq!(rows[0].facts["Break-even return on ad spend"], "4.00");
        }
    }

    #[test]
    fn the_user_sees_five_and_eight_conversions_when_the_ad_account_has_too_few_results() {
        let (scope, mut data) = fixture();
        let d = data.get_mut(&Handle::new(0)).unwrap();
        let mut defaults = d.rules.account().clone();
        defaults.state = ConversionTracking::TooFewResults;
        defaults.judged = false;
        d.rules = Resolved::new(defaults, BTreeMap::new());
        d.days = vec![
            metrics::CampaignDay {
                day: d.end_day - 2,
                campaign_id: 701,
                metrics: Metrics {
                    conversions: Some(5.0),
                    ..Default::default()
                },
            },
            metrics::CampaignDay {
                day: d.end_day - 32,
                campaign_id: 701,
                metrics: Metrics {
                    conversions: Some(8.0),
                    ..Default::default()
                },
            },
        ];
        let op = serde_json::from_value(
            json!({"operation":"account_overview","account":"A1","period":"last_30_days"}),
        )
        .unwrap();
        let rows = run(&op, 1, &scope, &data).unwrap();
        assert_eq!(rows[0].facts["Conversions"], "5");
        assert_eq!(rows[0].facts["Baseline"]["Conversions"], "8");
        assert!(rows[0]
            .caveats
            .contains(&Caveat::FewConversions { conversions: 5 }));
        assert!(!rows[0].caveats.contains(&Caveat::TrackingNotWorking));
    }

    #[test]
    fn the_user_sees_a_conversion_spike_as_a_tracking_problem() {
        let (scope, mut data) = fixture();
        let d = data.get_mut(&Handle::new(0)).unwrap();
        d.days = (d.end_day - 89..=d.end_day)
            .map(|day| metrics::CampaignDay {
                day,
                campaign_id: 701,
                metrics: Metrics {
                    clicks: 100.0,
                    spend: 10.0,
                    conversions: Some(if (d.end_day - 7..d.end_day).contains(&day) {
                        10.0
                    } else {
                        1.0
                    }),
                    ..Default::default()
                },
            })
            .collect();
        let op =
            serde_json::from_value(json!({"operation":"investigate_conversions","account":"A1"}))
                .unwrap();
        let rows = run(&op, 1, &scope, &data).unwrap();
        assert_eq!(rows[0].facts["Conversion tracking"], "spike");
        assert_eq!(rows[0].strength, Strength::Weak);
        assert!(rows[0].caveats.contains(&Caveat::TrackingNotWorking));
        let d = data.get_mut(&Handle::new(0)).unwrap();
        for earlier in &mut d.earlier {
            earlier.opened_day = d.end_day - 1;
        }
        let op =
            serde_json::from_value(json!({"operation":"review_history","account":"A1"})).unwrap();
        let rows = run(&op, 1, &scope, &data).unwrap();
        let recommendation = rows
            .iter()
            .find(|row| matches!(row.source, Source::RecommendationHistory))
            .unwrap();
        assert_eq!(recommendation.strength, Strength::Weak);
        assert!(recommendation.caveats.contains(&Caveat::TrackingNotWorking));
    }

    #[test]
    fn the_user_can_rank_every_ad_account_in_the_investigation() {
        let rows = execute(json!({"operation":"rank_accounts","period":"last_30_days"}));
        assert_eq!(rows[0].facts["Ad Account"], "Harbour Dental");
        assert_eq!(rows[0].id, EvidenceId::new(3, 1));
        assert!(rows[0].facts["Spend"].as_str().unwrap().starts_with('$'));
        assert!(rows[0].facts["Active Findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["Facts"]["Finding"].as_str().unwrap().contains("budget")));
    }
    #[test]
    fn a_campaigns_finding_cannot_borrow_another_campaigns_baseline_conversions() {
        let (scope, mut data) = fixture();
        let d = data.get_mut(&Handle::new(0)).unwrap();
        let deep = d.deep_check.as_mut().unwrap();
        let audit = deep.by_campaign.get_mut(&701).unwrap();
        audit.campaigns[0].counts.conversions = 40.0;
        audit.campaigns[0].previous.as_mut().unwrap().conversions = 9.0;
        deep.data.audit.previous.as_mut().unwrap().conversions = 100.0;
        let op = Operation::AccountOverview {
            account: Handle::new(0),
            period: Period::Last30Days,
        };
        let rows = run(&op, 1, &scope, &data).unwrap();
        let row = rows
            .iter()
            .find(|row| {
                matches!(row.source, Source::Finding { .. })
                    && matches!(row.subject, Subject::Campaign { id: 701, .. })
            })
            .unwrap();
        assert_eq!(row.strength, Strength::Weak);
        assert!(row
            .caveats
            .contains(&Caveat::FewConversions { conversions: 9 }));
    }
    #[test]
    fn the_user_sees_the_ad_accounts_metrics_tracking_goal_and_findings() {
        let rows =
            execute(json!({"operation":"account_overview","account":"A1","period":"last_7_days"}));
        assert_eq!(rows[0].facts["Conversion tracking"], "working");
        assert_eq!(rows[0].facts["Goal"], "");
        assert!(rows
            .iter()
            .any(|r| matches!(r.source, Source::Finding { .. })));
        assert!(rows[0]
            .caveats
            .contains(&Caveat::ConversionsNotSettled { days: 1 }));
        assert_ne!(rows[0].strength, Strength::Strong);
    }
    #[test]
    fn the_user_sees_the_ad_accounts_comparison_and_a_missing_last_year_baseline() {
        let rows =
            execute(json!({"operation":"compare_periods","account":"A1","period":"last_30_days"}));
        assert!(rows[0].facts["Changes"]["Spend"].is_string());
        assert!(rows[1].caveats.contains(&Caveat::NoBaseline));
        assert_eq!(rows[1].strength, Strength::Weak);
        assert!(rows[1].facts.get("Changes").is_none());
        assert!(!rows[1]
            .caveats
            .contains(&Caveat::FewConversions { conversions: 0 }));
    }
    #[test]
    fn the_user_sees_campaigns_ranked_by_spend_with_stable_ids_and_omitted_rows() {
        let rows = execute(
            json!({"operation":"compare_campaigns","account":"A1","period":"last_30_days","order":"spend","rows":2}),
        );
        assert_eq!(
            rows[1].subject,
            Subject::Campaign {
                id: 701,
                name: "Search - Emergency".into()
            }
        );
        assert!(rows[1].caveats.contains(&Caveat::RowsLeftOut { rows: 3 }));
        assert!(rows[1].facts["Cost per conversion"].is_string());
    }
    #[test]
    fn the_user_sees_conversion_actions_weeks_and_settlement_limits() {
        let rows = execute(json!({"operation":"investigate_conversions","account":"A1"}));
        assert_eq!(rows[0].facts["Enabled actions"], "3");
        assert_eq!(
            rows[1].subject,
            Subject::ConversionAction {
                name: "Phone call".into()
            }
        );
        assert_eq!(rows[1].facts["Conversions"], "12");
        assert_eq!(rows[1].strength, Strength::Weak);
        assert!(rows[1]
            .caveats
            .contains(&Caveat::ConversionsNotSettled { days: 1 }));
    }
    #[test]
    fn the_user_sees_search_terms_as_an_incomplete_deep_check_snapshot() {
        let rows = execute(
            json!({"operation":"investigate_search_terms","account":"A1","campaign":701,"focus":"spend_without_conversions","rows":1}),
        );
        assert_eq!(rows[0].facts["Conversions"], "0");
        assert!(rows[0]
            .caveats
            .iter()
            .any(|c| matches!(c, Caveat::SearchTermsIncomplete { .. })));
        assert!(rows[0]
            .caveats
            .iter()
            .any(|c| matches!(c, Caveat::Snapshot { .. })));
        assert_eq!(rows[0].facts["Campaign"]["id"], 701);
        let (scope, mut data) = fixture();
        let op = serde_json::from_value(json!({"operation":"investigate_search_terms","account":"A1","campaign":999,"focus":"top_spend","rows":1})).unwrap();
        assert_eq!(
            run(&op, 1, &scope, &data),
            Err(Refused::UnknownCampaign(999))
        );
        data.get_mut(&Handle::new(0)).unwrap().deep_check = None;
        let op = serde_json::from_value(json!({"operation":"investigate_search_terms","account":"A1","campaign":null,"focus":"top_spend","rows":1})).unwrap();
        assert_eq!(run(&op, 1, &scope, &data), Err(Refused::NoDeepCheck));
    }
    #[test]
    fn the_user_sees_devices_from_history_and_other_segments_from_the_deep_check() {
        for (segment, name) in [
            ("device", "Mobile"),
            ("location", "Dublin"),
            ("hour", "Weekday 1, hour 9"),
            ("demographic", "age: 25-34"),
        ] {
            let rows = execute(
                json!({"operation":"investigate_segments","account":"A1","segment":segment}),
            );
            assert_eq!(rows[0].subject, Subject::Segment { name: name.into() });
            assert_eq!(rows[0].facts["Spend"], "$200");
            assert_eq!(
                rows[0]
                    .caveats
                    .iter()
                    .any(|c| matches!(c, Caveat::Snapshot { .. })),
                segment != "device"
            );
        }
    }
    #[test]
    fn the_user_sees_missing_economics_inputs_and_the_existing_break_even_calculation() {
        let rows = execute(json!({"operation":"review_economics","account":"A1"}));
        assert!(rows[0].caveats.contains(&Caveat::MissingInput {
            input: "margin".into()
        }));
        let (scope, mut data) = fixture();
        let d = data.get_mut(&Handle::new(0)).unwrap();
        d.profile.economics.margin = Some(0.25);
        let op =
            serde_json::from_value(json!({"operation":"review_economics","account":"A1"})).unwrap();
        let rows = run(&op, 1, &scope, &data).unwrap();
        assert_eq!(
            rows[0].facts["break_even_return_on_ad_spend"]["value"],
            "4.00"
        );
    }
    #[test]
    fn the_user_sees_the_assessment_recommendation_history_and_account_changes() {
        let rows = execute(json!({"operation":"review_history","account":"A1"}));
        assert!(rows
            .iter()
            .any(|r| matches!(r.source, Source::Assessment { .. })));
        assert!(rows
            .iter()
            .any(|r| matches!(r.source, Source::RecommendationHistory)));
        assert!(rows
            .iter()
            .any(|r| matches!(r.source, Source::AccountChanges)));
        assert!(rows.iter().any(|r| r.facts["state"] == "open"));
    }
    #[test]
    fn the_user_sees_only_verified_business_facts() {
        use crate::monitoring::business::{Fact, FactKind, Kind, Research};
        let (scope, mut data) = fixture();
        data.get_mut(&Handle::new(0))
            .unwrap()
            .deep_check
            .as_mut()
            .unwrap()
            .research = Some(Research {
            day: 100,
            website: None,
            found: true,
            kind: Kind::LocalService,
            summary: "Unverified summary".into(),
            offers: vec![],
            areas: vec![],
            facts: vec![
                Fact {
                    claim: "Verified dentistry".into(),
                    kind: FactKind::Offer,
                    source: "https://example.com".into(),
                    verified: true,
                },
                Fact {
                    claim: "Unverified".into(),
                    kind: FactKind::Offer,
                    source: "https://elsewhere.example".into(),
                    verified: false,
                },
            ],
        });
        let op =
            serde_json::from_value(json!({"operation":"business_context","account":"A1"})).unwrap();
        let rows = run(&op, 1, &scope, &data).unwrap();
        assert_eq!(
            rows[0].facts,
            json!({"Verified facts":[{"claim":"Verified dentistry","source":"https://example.com"}]})
        );
        assert_eq!(
            rows[0].source,
            Source::BusinessContext {
                researched_day: Some(100)
            }
        );
    }
}

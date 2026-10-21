use loco_rs::Result;
use serde_json::{json, Value};

use crate::{
    agent::{
        answer::Limitation,
        evidence::{Caveat, Evidence, Source, Strength},
        investigation::Reply,
        operation::{Operation, Segment},
        Failure, State,
    },
    language::{say, say_plural, Text},
    workspace::{
        history::date_of,
        investigations::{Investigation, Outcome},
        AdAccount,
    },
};

fn state(state: State, text: &Text) -> &'static str {
    match state {
        State::Created => text.written.inv_created,
        State::Gathering => text.written.inv_gathering,
        State::Answering => text.written.inv_answering,
        State::Completed => text.written.inv_completed,
        State::Failed => text.written.inv_failed,
        State::Cancelled => text.written.inv_cancelled,
    }
}

fn strength(strength: Strength, text: &Text) -> &'static str {
    match strength {
        Strength::Strong => text.written.inv_strong,
        Strength::Limited => text.written.inv_limited,
        Strength::Weak => text.written.inv_weak,
    }
}

fn date(day: i64, text: &Text) -> String {
    date_of(day).map_or_else(String::new, |day| text.written.format.date_with_year(day))
}

fn caveat(caveat: &Caveat, text: &Text) -> Result<String> {
    Ok(match caveat {
        Caveat::ConversionsNotSettled { days } => {
            say_plural!(text, inv_unsettled, days.unsigned_abs())?
        }
        Caveat::FewConversions { conversions } => {
            say_plural!(text, inv_few, conversions.unsigned_abs())?
        }
        Caveat::TrackingNotWorking => text.written.inv_tracking_bad.into(),
        Caveat::SearchTermsIncomplete {
            shown,
            of_spend_share,
        } => {
            let mut line = say!(text, inv_terms_incomplete, count = shown)?;
            if let Some(share) = of_spend_share {
                line.push(' ');
                line.push_str(&say!(text, inv_share, share = share)?);
            }
            line
        }
        Caveat::Snapshot { day } => say!(text, inv_snapshot, date = date(*day, text))?,
        Caveat::RowsLeftOut { rows } => say_plural!(text, inv_rows, *rows as u64)?,
        Caveat::NoBaseline => text.written.inv_no_baseline.into(),
        Caveat::MissingInput { input } => {
            let input = match input.as_str() {
                "margin" => text.written.inv_input_margin,
                "customer_value" => text.written.inv_input_customer,
                "close_rate" => text.written.inv_input_close,
                "target_return" => text.written.inv_input_return,
                _ => input.as_str(),
            };
            say!(text, inv_missing, input = input)?
        }
    })
}

fn source(source: &Source, text: &Text) -> Result<String> {
    Ok(match source {
        Source::AccountHistory => text.written.inv_source_history.into(),
        Source::ConversionTracking => text.written.inv_source_tracking.into(),
        Source::DeepCheck { day } => say!(text, inv_source_deep, date = date(*day, text))?,
        Source::Assessment { day } => say!(text, inv_source_assessment, date = date(*day, text))?,
        Source::Finding { .. } => text.written.inv_source_finding.into(),
        Source::RecommendationHistory => text.written.inv_source_recommendations.into(),
        Source::AccountChanges => text.written.inv_source_changes.into(),
        Source::AdAccountProfile => text.written.inv_source_profile.into(),
        Source::BusinessContext { researched_day } => match researched_day {
            Some(day) => format!(
                "{} · {}",
                text.written.inv_source_business,
                date(*day, text)
            ),
            None => text.written.inv_source_business.into(),
        },
    })
}

fn operation(operation: &Operation, text: &Text) -> &'static str {
    match operation {
        Operation::RankAccounts { .. } => text.written.inv_rank,
        Operation::AccountOverview { .. } => text.written.inv_overview,
        Operation::ComparePeriods { .. } => text.written.inv_compare,
        Operation::CompareCampaigns { .. } => text.written.inv_compare_campaigns,
        Operation::InvestigateConversions { .. } => text.written.inv_tracking,
        Operation::InvestigateSearchTerms { .. } => text.written.inv_search,
        Operation::InvestigateSegments { segment, .. } => match segment {
            Segment::Device => text.written.inv_device,
            Segment::Location => text.written.inv_location,
            Segment::Hour => text.written.inv_hour,
            Segment::Demographic => text.written.inv_demographic,
        },
        Operation::ReviewEconomics { .. } => text.written.inv_economics,
        Operation::ReviewHistory { .. } => text.written.inv_history,
        Operation::BusinessContext { .. } => text.written.inv_business,
    }
}

// Flatten nested facts into labelled rows; model and source values remain autoescaped text.
fn facts(value: &Value, label: &str, rows: &mut Vec<Value>) {
    match value {
        Value::Object(fields) => for (key, value) in fields {
            facts(value, &if label.is_empty() { key.clone() } else { format!("{label} · {key}") }, rows);
        },
        Value::Array(items) => for (index, item) in items.iter().enumerate() { facts(item, &format!("{label} · {}", index + 1), rows); },
        Value::Null => {},
        value => rows.push(json!({"label":label,"value":value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())})),
    }
}

fn evidence_for_account(item: &Evidence, account: &str, text: &Text) -> Result<Value> {
    let mut rows = Vec::new();
    facts(&item.facts, "", &mut rows);
    let days = |days: crate::agent::evidence::Days| {
        format!("{} – {}", date(days.start, text), date(days.end, text))
    };
    Ok(json!({
        "id": item.id.0, "anchor":item.id.0.replace('.', "-"),
        "subject":item.subject.name(), "account":account,
        "days":item.days.map(days),"baseline":item.baseline.map(days),
        "source":source(&item.source, text)?, "strength":strength(item.strength,text), "facts":rows,
        "caveats":item.caveats.iter().map(|item| caveat(item,text)).collect::<Result<Vec<_>>>()?,
    }))
}

pub fn recent(row: &Investigation, accounts: &[AdAccount], text: &Text) -> Value {
    json!({"id":row.id,"question":row.question,"accounts":accounts.iter().filter(|ad|row.accounts.contains(&ad.customer_id)).map(|ad|ad.name.clone()).collect::<Vec<_>>(),"state":state(row.state,text),"date":chrono::DateTime::from_timestamp(row.created_at,0).map(|at|text.written.format.date_with_year(at.date_naive()))})
}

pub fn thread(row: &Investigation, accounts: &[AdAccount], text: &Text) -> Result<Value> {
    let scoped = crate::agent::data::Scope::from_accounts(accounts, &row.accounts);
    let account_name = |operation: &Operation| {
        operation
            .account()
            .and_then(|handle| scoped.resolve(handle))
            .map(|ad| ad.name.clone())
    };
    let mut value = recent(row, accounts, text);
    value["finished"] = json!(row.state.finished());
    value["answering"] = json!(row.state == State::Answering);
    value["cancelled"] = json!(row.state == State::Cancelled);
    value["no_access"] = json!(row.failure == Some(Failure::NoAccess));
    value["failure"] = json!(match row.failure {
        Some(Failure::DailyLimit | Failure::Budget) => Some(text.written.inv_budget),
        Some(Failure::Model) => Some(text.written.inv_model),
        Some(Failure::Interrupted) => Some(text.written.inv_interrupted),
        _ => None,
    });
    value["steps"] = json!(row.steps.iter().map(|step| json!({"label":operation(&step.operation,text),"account":account_name(&step.operation),"outcome":match step.outcome { Outcome::Answered => None, Outcome::Refused => Some(text.written.inv_step_refused), Outcome::Repeated => Some(text.written.inv_step_repeated) }})).collect::<Vec<_>>());
    value["report"] = Value::Null;
    value["clarification"] = Value::Null;
    if let Some(report) = &row.report {
        match serde_json::from_value::<Reply>(report.clone())? {
            Reply::Clarification { question } => value["clarification"] = json!(question),
            Reply::Answer(report) => {
                let mut shown = serde_json::to_value(&report)?;
                for key in ["observations", "hypotheses", "recommendations"] {
                    for claim in shown[key].as_array_mut().expect("report list") {
                        let parsed: Strength = serde_json::from_value(claim["strength"].clone())?;
                        claim["strength"] = json!(strength(parsed, text));
                        claim["citations"] = json!(claim["evidence"]
                            .as_array()
                            .expect("citation list")
                            .iter()
                            .map(|id| {
                                let id = id.as_str().expect("evidence ID");
                                json!({"id":id,"anchor":id.replace('.',"-")})
                            })
                            .collect::<Vec<_>>());
                    }
                }
                shown["limitations"] = json!(report
                    .limitations
                    .iter()
                    .map(|item| match item {
                        Limitation::Stated { text } => Ok(text.clone()),
                        Limitation::Evidence { caveat: item, .. } => caveat(item, text),
                    })
                    .collect::<Result<Vec<_>>>()?);
                shown["evidence"] = json!(report
                    .evidence
                    .iter()
                    .map(|item| {
                        let account = scoped
                            .resolve(item.account)
                            .map_or("", |ad| ad.name.as_str());
                        evidence_for_account(item, account, text)
                    })
                    .collect::<Result<Vec<_>>>()?);
                value["report"] = shown;
            }
        }
    }
    Ok(value)
}

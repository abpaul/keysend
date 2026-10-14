use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde_json::Value;

use super::{counts, numeric, resource_id, text, Client, Failure, COUNTS};
use crate::{
    monitoring::audit::Counts,
    workspace::monitoring::{
        Collected, DeepBreakdowns, DemographicTotal, HourTotal, KeywordQuality, LocationTotal,
    },
};

pub(crate) const LOCATIONS_STORED: usize = 20;
pub(crate) const DELAY_DAYS: i64 = 90;

pub(crate) const KEYWORD_QUALITY_FIELDS: &str = "ad_group.id, ad_group_criterion.criterion_id, ad_group_criterion.keyword.match_type, ad_group_criterion.quality_info.quality_score, ad_group_criterion.quality_info.creative_quality_score, ad_group_criterion.quality_info.post_click_quality_score, ad_group_criterion.quality_info.search_predicted_ctr";

fn add(total: &mut Counts, other: &Counts) {
    total.impressions += other.impressions;
    total.clicks += other.clicks;
    total.conversions += other.conversions;
    total.cost += other.cost;
    total.value += other.value;
}

pub(crate) fn unread(part: &str, failure: Failure) -> Result<String, Failure> {
    match failure {
        Failure::Unsupported => Ok(failure.message().into()),
        Failure::Unavailable => {
            tracing::warn!(
                part,
                "A part of the Deep check could not be read from Google Ads"
            );
            Ok(failure.message().into())
        }
        failure => Err(failure),
    }
}

fn part<T>(name: &str, result: Result<T, Failure>) -> Result<Collected<T>, Failure> {
    match result {
        Ok(rows) => Ok(Ok(rows)),
        Err(failure) => unread(name, failure).map(Err),
    }
}

fn weekday(name: &str) -> Option<i64> {
    [
        "MONDAY",
        "TUESDAY",
        "WEDNESDAY",
        "THURSDAY",
        "FRIDAY",
        "SATURDAY",
        "SUNDAY",
    ]
    .iter()
    .position(|day| *day == name)
    .map(|index| index as i64)
}

async fn hours(client: &Client<'_, '_>, days: &str) -> Result<Vec<HourTotal>, Failure> {
    let mut totals: BTreeMap<(i64, i64), Counts> = BTreeMap::new();
    let mut skipped = 0;
    for row in client
        .query_later(&format!(
            "SELECT segments.day_of_week, segments.hour, {COUNTS} FROM customer WHERE {days}"
        ))
        .await?
    {
        let (Some(weekday), Some(hour)) = (
            weekday(&text(&row, "/segments/dayOfWeek")),
            row.pointer("/segments/hour")
                .and_then(|hour| numeric(hour).ok())
                .map(|hour| hour as i64)
                .filter(|hour| (0..24).contains(hour)),
        ) else {
            skipped += 1;
            continue;
        };
        add(totals.entry((weekday, hour)).or_default(), &counts(&row)?);
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            "Rows of the hours with an unknown weekday or hour were left out"
        );
    }
    Ok(totals
        .into_iter()
        .map(|((weekday, hour), counts)| HourTotal {
            weekday,
            hour,
            counts,
        })
        .collect())
}

fn location_name(canonical: &str) -> String {
    canonical
        .split(',')
        .map(str::trim)
        .take(2)
        .collect::<Vec<_>>()
        .join(", ")
}

async fn locations(client: &Client<'_, '_>, days: &str) -> Result<Vec<LocationTotal>, Failure> {
    let mut totals: BTreeMap<i64, Counts> = BTreeMap::new();
    for row in client
        .query_later(&format!("SELECT geographic_view.country_criterion_id, segments.geo_target_city, {COUNTS} FROM geographic_view WHERE {days} ORDER BY metrics.cost_micros DESC LIMIT 200"))
        .await?
    {
        let Some(id) = resource_id(&row, "/segments/geoTargetCity")
            .or_else(|| resource_id(&row, "/geographicView/countryCriterionId"))
        else {
            return Err(Failure::Unavailable);
        };
        add(totals.entry(id).or_default(), &counts(&row)?);
    }
    let mut top = totals.into_iter().collect::<Vec<_>>();
    top.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost).then(a.0.cmp(&b.0)));
    top.truncate(LOCATIONS_STORED);
    if top.is_empty() {
        return Ok(Vec::new());
    }
    let ids = top
        .iter()
        .map(|(id, _)| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let names = client
        .query_later(&format!("SELECT geo_target_constant.id, geo_target_constant.canonical_name FROM geo_target_constant WHERE geo_target_constant.id IN ({ids})"))
        .await?
        .iter()
        .filter_map(|row| {
            Some((
                resource_id(row, "/geoTargetConstant/id")?,
                location_name(row.pointer("/geoTargetConstant/canonicalName")?.as_str()?),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    Ok(top
        .into_iter()
        .map(|(id, counts)| LocationTotal {
            id,
            name: names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("Location {id}")),
            counts,
        })
        .collect())
}

async fn demographic(
    client: &Client<'_, '_>,
    field: &str,
    resource: &str,
    pointer: &str,
    days: &str,
) -> Result<Vec<DemographicTotal>, Failure> {
    let mut totals: BTreeMap<String, Counts> = BTreeMap::new();
    for row in client
        .query_later(&format!(
            "SELECT {field}, {COUNTS} FROM {resource} WHERE {days}"
        ))
        .await?
    {
        let value = row
            .pointer(pointer)
            .and_then(Value::as_str)
            .ok_or(Failure::Unavailable)?;
        add(totals.entry(value.to_owned()).or_default(), &counts(&row)?);
    }
    Ok(totals
        .into_iter()
        .map(|(value, counts)| DemographicTotal { value, counts })
        .collect())
}

async fn delay(
    client: &Client<'_, '_>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<(String, f64)>, Failure> {
    let mut totals: BTreeMap<String, f64> = BTreeMap::new();
    for row in client
        .query_later(&format!("SELECT segments.conversion_lag_bucket, metrics.conversions FROM customer WHERE segments.date BETWEEN '{from}' AND '{to}'"))
        .await?
    {
        let bucket = row
            .pointer("/segments/conversionLagBucket")
            .and_then(Value::as_str)
            .ok_or(Failure::Unavailable)?;
        *totals.entry(bucket.to_owned()).or_default() += numeric(&row["metrics"]["conversions"])?;
    }
    Ok(totals.into_iter().collect())
}

pub(crate) fn keyword_quality(rows: &[Value]) -> Vec<KeywordQuality> {
    let part = |row: &Value, name: &str| {
        row.pointer(&format!("/adGroupCriterion/qualityInfo/{name}"))
            .and_then(Value::as_str)
            .filter(|bucket| matches!(*bucket, "ABOVE_AVERAGE" | "AVERAGE" | "BELOW_AVERAGE"))
            .map(str::to_owned)
    };
    let mut seen = std::collections::BTreeSet::new();
    rows.iter()
        .filter_map(|row| {
            let quality = KeywordQuality {
                campaign_id: resource_id(row, "/campaign/id")?,
                ad_group_id: resource_id(row, "/adGroup/id")?,
                criterion_id: resource_id(row, "/adGroupCriterion/criterionId")?,
                keyword: row
                    .pointer("/adGroupCriterion/keyword/text")?
                    .as_str()?
                    .to_owned(),
                match_type: text(row, "/adGroupCriterion/keyword/matchType"),
                cost: numeric(&row["metrics"]["costMicros"]).ok()? / 1_000_000.0,
                score: row
                    .pointer("/adGroupCriterion/qualityInfo/qualityScore")
                    .and_then(|score| numeric(score).ok())
                    .map(|score| score as i64)
                    .filter(|score| (1..=10).contains(score)),
                ad_relevance: part(row, "creativeQualityScore"),
                landing_page: part(row, "postClickQualityScore"),
                expected_ctr: part(row, "searchPredictedCtr"),
            };
            seen.insert((
                quality.campaign_id,
                quality.ad_group_id,
                quality.criterion_id,
            ))
            .then_some(quality)
        })
        .collect()
}

pub(crate) async fn collect(
    client: &Client<'_, '_>,
    from: NaiveDate,
    to: NaiveDate,
    delay_from: NaiveDate,
    judged: bool,
    keywords: Collected<Vec<KeywordQuality>>,
) -> Result<DeepBreakdowns, Failure> {
    let days = format!("segments.date BETWEEN '{from}' AND '{to}'");
    Ok(DeepBreakdowns {
        hours: part("hours", hours(client, &days).await)?,
        locations: part("locations", locations(client, &days).await)?,
        keywords,
        ages: part(
            "ages",
            demographic(
                client,
                "ad_group_criterion.age_range.type",
                "age_range_view",
                "/adGroupCriterion/ageRange/type",
                &days,
            )
            .await,
        )?,
        genders: part(
            "genders",
            demographic(
                client,
                "ad_group_criterion.gender.type",
                "gender_view",
                "/adGroupCriterion/gender/type",
                &days,
            )
            .await,
        )?,
        delay: if judged {
            Some(part(
                "conversion delay",
                delay(client, delay_from, to).await,
            )?)
        } else {
            None
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_failure_of_one_part_is_stored_with_the_part() {
        let stored = |failure| unread("hours", failure).ok();
        assert_eq!(
            stored(Failure::Unsupported).as_deref(),
            Some("Google Ads does not support this request.")
        );
        assert_eq!(
            stored(Failure::Unavailable).as_deref(),
            Some("Google Ads data is unavailable. Try again later.")
        );
        for failure in [
            Failure::Quota,
            Failure::DailyLimit,
            Failure::Reconnect,
            Failure::Disabled,
        ] {
            assert!(unread("hours", failure).is_err());
        }
    }
}

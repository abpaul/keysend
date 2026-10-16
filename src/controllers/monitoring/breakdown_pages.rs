use std::{collections::BTreeMap, sync::Arc};

use axum::{
    extract::{Path, Query},
    response::{IntoResponse, Redirect},
    Extension,
};
use loco_rs::prelude::*;
use serde_json::json;

use super::{
    account_path, chart_pages::Source, clock, numbers, window_of, with_window, WindowQuery,
};
use crate::{
    auth::OwnedAdAccount,
    language::{fill, say, say_plural, Text},
    monitoring::{
        charts::{self, Chart, Part, SearchShare, Subject},
        deep_view::{self, Section},
        explain,
    },
    settings::Settings,
    workspace::{
        history::Campaign,
        monitoring::{Breakdown, BreakdownState, SearchShareTotal},
        WorkspaceStore,
    },
};

const SEARCH_CAMPAIGNS: usize = 3;

#[derive(Clone, Copy)]
enum Missing {
    Locations,
    Hours,
    Quality,
    SearchShare,
    Actions,
    Networks,
    Age,
    Gender,
}

fn missing(text: &Text, state: &BreakdownState, what: Missing, results: &str) -> Result<String> {
    let w = &text.written;
    let (failed, unread, none) = match what {
        Missing::Locations => (
            w.missing_locations_error,
            w.missing_locations_unread,
            w.missing_locations_none,
        ),
        Missing::Hours => (
            w.missing_hours_error,
            w.missing_hours_unread,
            w.missing_hours_none,
        ),
        Missing::Quality => (
            w.missing_quality_error,
            w.missing_quality_unread,
            w.missing_quality_none,
        ),
        Missing::SearchShare => (
            w.missing_search_share_error,
            w.missing_search_share_unread,
            w.missing_search_share_none,
        ),
        Missing::Actions => (
            w.missing_actions_error,
            w.missing_actions_unread,
            w.missing_actions_none,
        ),
        Missing::Networks => (
            w.missing_networks_error,
            w.missing_networks_unread,
            w.missing_networks_none,
        ),
        Missing::Age => (w.missing_age_error, w.missing_age_unread, ""),
        Missing::Gender => (w.missing_gender_error, w.missing_gender_unread, ""),
    };
    match (&state.error, state.last_day) {
        (Some(error), _) => fill("missing", failed, &[("error", error.as_str())]),
        (None, None) => fill("missing", unread, &[] as &[(&str, String)]),
        (None, Some(_)) if none.contains("{results}") => {
            fill("missing", none, &[("results", results)])
        }
        (None, Some(_)) => fill("missing", none, &[] as &[(&str, String)]),
    }
}

fn network_name(text: &Text, network: &str) -> &'static str {
    let w = &text.written;
    match network {
        "SEARCH" => w.network_search,
        "SEARCH_PARTNERS" => w.network_search_partners,
        "CONTENT" => w.network_content,
        "YOUTUBE" => w.network_youtube,
        "GOOGLE_TV" => w.network_google_tv,
        "MIXED" => w.network_mixed,
        "DISCOVER" => w.network_discover,
        "GMAIL" => w.network_gmail,
        "MAPS" => w.network_maps,
        "GOOGLE_OWNED_CHANNELS" => w.network_owned,
        _ => w.network_other,
    }
}

impl Source<'_> {
    fn start(&self) -> i64 {
        self.end - self.window.days() + 1
    }

    pub async fn search_share(
        &self,
        store: &WorkspaceStore<'_>,
        campaigns: &[Campaign],
        only: Option<i64>,
        href: impl Fn(i64) -> String,
    ) -> Result<Option<Chart>> {
        let shown = self.search_campaigns(campaigns, only);
        if shown.is_empty() {
            return Ok(None);
        }
        let totals = store
            .search_share_totals(self.id, self.start(), self.end)
            .await?;
        if let Some(chart) = self.search_share_of(&shown, &totals, |campaign| {
            only.is_none().then(|| href(campaign))
        })? {
            return Ok(Some(chart));
        }
        let state = store
            .breakdown_state(self.id, Breakdown::SearchShare)
            .await?;
        Ok(Some(Chart::empty(
            self.search_share_title()?,
            missing(self.text, &state, Missing::SearchShare, self.results())?,
        )))
    }

    pub fn search_campaigns<'c>(
        &self,
        campaigns: &'c [Campaign],
        only: Option<i64>,
    ) -> Vec<&'c Campaign> {
        let mut spend: BTreeMap<i64, f64> = BTreeMap::new();
        for day in self.days.iter().filter(|day| day.day >= self.start()) {
            *spend.entry(day.campaign_id).or_default() += day.metrics.spend;
        }
        let mut shown = campaigns
            .iter()
            .filter(|campaign| campaign.channel_type == "SEARCH")
            .filter(|campaign| match only {
                Some(id) => campaign.id == id,
                None => spend.get(&campaign.id).is_some_and(|spend| *spend > 0.0),
            })
            .collect::<Vec<_>>();
        shown.sort_by(|a, b| {
            let of = |campaign: &Campaign| spend.get(&campaign.id).copied().unwrap_or_default();
            of(b).total_cmp(&of(a)).then(a.name.cmp(&b.name))
        });
        shown.truncate(SEARCH_CAMPAIGNS);
        shown
    }

    fn search_share_title(&self) -> Result<String> {
        say_plural!(
            self.text,
            search_share_title,
            u64::try_from(self.window.days()).unwrap_or_default()
        )
    }

    pub fn search_share_of(
        &self,
        shown: &[&Campaign],
        totals: &[SearchShareTotal],
        href: impl Fn(i64) -> Option<String>,
    ) -> Result<Option<Chart>> {
        let rows = shown
            .iter()
            .filter_map(|campaign| {
                let total = totals
                    .iter()
                    .find(|total| total.campaign_id == campaign.id)?;
                Some(SearchShare {
                    name: campaign.name.clone(),
                    href: href(campaign.id),
                    shares: total
                        .shares
                        .map(|shares| (shares.shown, shares.lost_budget, shares.lost_rank)),
                    exact_days: total.exact_days,
                    limit_days: total.limit_days,
                    days: self.window.days(),
                })
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(charts::search_share(
            self.text,
            &self.search_share_title()?,
            rows,
        )?))
    }

    async fn action_weeks(&self, store: &WorkspaceStore<'_>, states: &States) -> Result<Chart> {
        let state = state_of(states, Breakdown::ConversionActions);
        let Some(first) = state.first_day else {
            return Ok(Chart::empty(
                say_plural!(self.text, action_weeks_title, charts::WEEKS as u64)?,
                missing(self.text, &state, Missing::Actions, self.results())?,
            ));
        };
        let starts = (0..charts::WEEKS)
            .rev()
            .map(|week| self.end - week * 7 - 6)
            .filter(|start| *start >= first.max(self.first_day))
            .collect::<Vec<_>>();
        let rows = match starts.first() {
            Some(start) => {
                store
                    .conversion_action_weeks(self.id, *start, self.end)
                    .await?
            }
            None => Vec::new(),
        };
        charts::action_weeks(self.text, &starts, &rows)
    }

    pub async fn network_shares(&self, store: &WorkspaceStore<'_>, campaign: i64) -> Result<Chart> {
        let title = self.share_title(Subject::Network)?;
        let mut named: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
        for total in store
            .network_totals(self.id, self.start(), self.end, campaign)
            .await?
        {
            let part = named
                .entry(network_name(self.text, &total.network))
                .or_default();
            part.0 += total.spend;
            part.1 += if self.judged {
                total.conversions
            } else {
                total.clicks
            };
        }
        let parts = named
            .into_iter()
            .map(|(name, (spend, results))| Part {
                name: name.into(),
                href: None,
                spend,
                results,
            })
            .collect::<Vec<_>>();
        if parts.is_empty() {
            let state = store.breakdown_state(self.id, Breakdown::Networks).await?;
            return Ok(Chart::empty(
                title,
                missing(self.text, &state, Missing::Networks, self.results())?,
            ));
        }
        charts::shares(
            self.text,
            Subject::Network,
            &title,
            parts,
            self.judged,
            self.currency,
        )
    }
}

const KEYWORDS_SHOWN: u32 = 10;
const LOCATIONS_SHOWN: usize = 8;

fn demographic_name(text: &Text, value: &str) -> (String, bool) {
    let w = &text.written;
    let name = match value {
        "AGE_RANGE_18_24" => w.demographic_18_24,
        "AGE_RANGE_25_34" => w.demographic_25_34,
        "AGE_RANGE_35_44" => w.demographic_35_44,
        "AGE_RANGE_45_54" => w.demographic_45_54,
        "AGE_RANGE_55_64" => w.demographic_55_64,
        "AGE_RANGE_65_UP" => w.demographic_65_up,
        "FEMALE" => w.demographic_female,
        "MALE" => w.demographic_male,
        _ => return (w.demographic_unknown.into(), false),
    };
    (name.into(), true)
}

pub(super) async fn keyword_scores(
    text: &Text,
    store: &WorkspaceStore<'_>,
    id: &str,
    campaign: i64,
) -> Result<Section> {
    let (scores, compared) = store.keyword_scores(id, campaign, KEYWORDS_SHOWN).await?;
    if scores.is_empty() {
        let state = store.breakdown_state(id, Breakdown::KeywordQuality).await?;
        return Ok(deep_view::quality_missing(
            text,
            missing(text, &state, Missing::Quality, "")?,
        ));
    }
    deep_view::quality(text, &scores, compared)
}

pub(super) async fn deep_check(
    text: &Text,
    store: &WorkspaceStore<'_>,
    id: &str,
    campaign: i64,
    currency: &str,
    judged: bool,
) -> Result<Option<(String, Vec<Section>)>> {
    let Some(check) = store.latest_deep_check(id).await? else {
        return Ok(None);
    };
    Ok(Some((
        super::view::day_month(text, check.day)?,
        deep_view::sections(
            text,
            keyword_scores(text, store, id, campaign).await?,
            check.by_campaign.get(&campaign),
            campaign,
            currency,
            judged,
        )?,
    )))
}

pub(super) async fn delay_note(
    text: &Text,
    store: &WorkspaceStore<'_>,
    id: &str,
) -> Result<Option<String>> {
    store
        .conversion_delay(id)
        .await?
        .map_or(Ok(None), |(conversions, late)| {
            charts::delay_sentence(text, conversions, late)
        })
}

type States = BTreeMap<String, BreakdownState>;

fn state_of(states: &States, kind: Breakdown) -> BreakdownState {
    states.get(kind.key()).cloned().unwrap_or_default()
}

impl Source<'_> {
    fn result_count(&self, counts: &crate::monitoring::audit::Counts) -> f64 {
        if self.judged {
            counts.conversions
        } else {
            counts.clicks
        }
    }

    async fn hour_grid(&self, store: &WorkspaceStore<'_>, states: &States) -> Result<Chart> {
        let totals = store.hour_totals(self.id).await?;
        if totals.is_empty() {
            return Ok(Chart::empty(
                say!(
                    self.text,
                    hours_title,
                    results = if self.judged {
                        self.text.written.metric_conversions
                    } else {
                        self.text.written.metric_clicks
                    }
                )?,
                missing(
                    self.text,
                    &state_of(states, Breakdown::Hours),
                    Missing::Hours,
                    self.results(),
                )?,
            ));
        }
        let mut values = [[0.0; 24]; 7];
        for total in &totals {
            if let Some(cell) = usize::try_from(total.weekday)
                .ok()
                .and_then(|weekday| values.get_mut(weekday))
                .zip(usize::try_from(total.hour).ok())
                .and_then(|(day, hour)| day.get_mut(hour))
            {
                *cell += self.result_count(&total.counts);
            }
        }
        charts::hour_grid(self.text, &values, self.judged)
    }

    async fn location_shares(
        &self,
        store: &WorkspaceStore<'_>,
        states: &States,
        day: Option<i64>,
    ) -> Result<Chart> {
        let title = charts::shares_title(
            self.text,
            Subject::Location,
            self.judged,
            Some(charts::days(self.text, 30)?),
        )?;
        let mut totals = store.location_totals(self.id).await?;
        let Some(day) = day.filter(|_| !totals.is_empty()) else {
            return Ok(Chart::empty(
                title,
                missing(
                    self.text,
                    &state_of(states, Breakdown::Locations),
                    Missing::Locations,
                    self.results(),
                )?,
            ));
        };
        totals.truncate(LOCATIONS_SHOWN);
        let read;
        let days = if day - 30 >= self.from && day - 1 <= self.end {
            self.days
        } else {
            read = store.monitoring_days(self.id, day - 30, day - 1).await?;
            &read
        };
        let all = crate::monitoring::metrics::sum(days, day - 30, day - 1, None);
        let all_results = if self.judged {
            all.conversions.unwrap_or_default()
        } else {
            all.clicks
        };
        let parts = totals
            .iter()
            .map(|total| Part {
                name: total.name.clone(),
                href: None,
                spend: total.counts.cost,
                results: self.result_count(&total.counts),
            })
            .collect::<Vec<_>>();
        let named = parts.iter().fold((0.0, 0.0), |sum, part| {
            (sum.0 + part.spend, sum.1 + part.results)
        });
        let other = (
            (all.spend - named.0).max(0.0),
            (all_results - named.1).max(0.0),
        );
        let rest = (other.0 >= 0.005 || other.1 >= 0.05).then(|| Part {
            name: self.text.written.shares_other_locations.into(),
            href: None,
            spend: other.0,
            results: other.1,
        });
        charts::shares_of(
            self.text,
            Subject::Location,
            &title,
            parts,
            rest,
            self.judged,
            self.currency,
            LOCATIONS_SHOWN + 1,
        )
    }

    async fn demographics(
        &self,
        store: &WorkspaceStore<'_>,
        states: &States,
    ) -> Result<(Chart, Chart)> {
        let totals = store.demographic_totals(self.id).await?;
        let chart = |kind: Breakdown, subject: Subject, missing_kind: Missing| -> Result<Chart> {
            let title = charts::shares_title(
                self.text,
                subject,
                self.judged,
                Some(charts::days(self.text, 30)?),
            )?;
            let mut groups: BTreeMap<String, charts::Group> = BTreeMap::new();
            for (_, total) in totals.iter().filter(|(stored, _)| stored == kind.key()) {
                let (name, known) = demographic_name(self.text, &total.value);
                let group = groups.entry(name.clone()).or_insert(charts::Group {
                    name,
                    known,
                    spend: 0.0,
                    clicks: 0.0,
                    results: 0.0,
                });
                group.spend += total.counts.cost;
                group.clicks += total.counts.clicks;
                group.results += self.result_count(&total.counts);
            }
            let state = state_of(states, kind);
            if groups.is_empty() && (state.error.is_some() || state.last_day.is_none()) {
                return Ok(Chart::empty(
                    title,
                    missing(self.text, &state, missing_kind, self.results())?,
                ));
            }
            charts::demographic(
                self.text,
                &title,
                subject,
                groups.into_values().collect(),
                self.judged,
                self.currency,
            )
        };
        Ok((
            chart(Breakdown::Age, Subject::AgeGroup, Missing::Age)?,
            chart(Breakdown::Gender, Subject::Gender, Missing::Gender)?,
        ))
    }
}

pub(super) struct Breakdowns {
    pub deep_check: Option<String>,
    pub action_weeks: Option<Chart>,
    pub hours: Chart,
    pub locations: Chart,
    pub ages: Chart,
    pub genders: Chart,
}

impl Source<'_> {
    pub(super) async fn breakdowns(&self, store: &WorkspaceStore<'_>) -> Result<Breakdowns> {
        let states = store.breakdown_states(self.id).await?;
        let checked = state_of(&states, Breakdown::Hours).last_day;
        let action_weeks = if self.judged {
            Some(self.action_weeks(store, &states).await?)
        } else {
            None
        };
        let (ages, genders) = self.demographics(store, &states).await?;
        Ok(Breakdowns {
            deep_check: checked
                .map(|day| super::view::day_month(self.text, day))
                .transpose()?,
            action_weeks,
            hours: self.hour_grid(store, &states).await?,
            locations: self.location_shares(store, &states, checked).await?,
            ages,
            genders,
        })
    }
}

pub(super) async fn show(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<String>,
    Query(query): Query<WindowQuery>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let window = window_of(query.window.as_deref());
    let back = with_window(&account_path(&id), window);
    if !store.monitoring_access(settings.now().timestamp()).await? {
        return Ok(Redirect::to(&back).into_response());
    }
    let clock = clock(&store, &settings, &id).await?;
    let history = store.history_state(&id).await?;
    let Some(numbers) = numbers(text, &store, &id, &clock, history.as_ref(), 1).await? else {
        return Ok(Redirect::to(&back).into_response());
    };
    let source = numbers.source(&id, window);
    let breakdowns = source.breakdowns(&store).await?;
    let data = json!({
        "ad": {"id": id, "name": ad.name, "href": back},
        "notice": numbers.notice,
        "explain": explain::charts(text, numbers.judged),
        "action_weeks": breakdowns.action_weeks,
        "deep_check": breakdowns.deep_check,
        "hours": breakdowns.hours,
        "locations": breakdowns.locations,
        "ages": breakdowns.ages,
        "genders": breakdowns.genders,
    });
    format::render().view(&v, "monitoring/breakdowns.html", data)
}

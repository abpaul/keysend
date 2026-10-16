use super::*;
use keysend::workspace::monitoring::{
    ActionDay, DeepBreakdowns, DemographicTotal, HourTotal, KeywordQuality, LocationTotal,
    NetworkDay, SearchShareDay, Share,
};

const PAGE: &str = "/ad-accounts/4829153307";
const EMERGENCY: i64 = 2;

fn count(page: &str, text: &str) -> usize {
    page.matches(text).count()
}

fn part<'a>(page: &'a str, from: &str, to: &str) -> &'a str {
    let start = page.find(&format!(r#"id="{from}""#)).unwrap();
    let end = page[start..].find(&format!(r#"id="{to}""#)).unwrap();
    &page[start..start + end]
}

fn queries(f: &Fixture, part: &str) -> Vec<String> {
    f.calls
        .lock()
        .unwrap()
        .google
        .requests
        .iter()
        .map(|(_, body)| body["query"].as_str().unwrap().to_owned())
        .filter(|query| query.contains(part))
        .collect()
}

async fn rows(f: &Fixture, query: &str) -> Vec<String> {
    f.ctx
        .db
        .query_all_raw(Statement::from_string(DatabaseBackend::Sqlite, query))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String>("", "line").unwrap())
        .collect()
}

fn day_before(days: u64) -> NaiveDate {
    end().checked_sub_days(Days::new(days)).unwrap()
}

#[tokio::test]
#[serial]
async fn the_daily_collection_reads_the_three_breakdowns_and_stores_their_rows() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    {
        let mut calls = f.calls.lock().unwrap();
        calls.google.breakdown_rows.insert(
            "metrics.search_impression_share",
            vec![
                json!({"campaign":{"id":"701"},"segments":{"date":end().to_string()},"metrics":{"impressions":"520","searchImpressionShare":0.52,"searchBudgetLostImpressionShare":0.31,"searchRankLostImpressionShare":0.17}}),
                json!({"campaign":{"id":"701"},"segments":{"date":day_before(1).to_string()},"metrics":{"impressions":"40","searchImpressionShare":0.0999,"searchBudgetLostImpressionShare":0.9001,"searchRankLostImpressionShare":0.05}}),
                json!({"campaign":{"id":"701"},"segments":{"date":day_before(2).to_string()},"metrics":{"impressions":"0"}}),
            ],
        );
        calls.google.breakdown_rows.insert(
            "segments.conversion_action_name",
            vec![
                json!({"campaign":{"id":"701"},"segments":{"date":end().to_string(),"conversionActionName":"Booking form"},"metrics":{"conversions":3.0,"conversionsValue":150.0}}),
                json!({"campaign":{"id":"701"},"segments":{"date":end().to_string(),"conversionActionName":"Phone call"},"metrics":{"conversions":1.5,"conversionsValue":0.0}}),
            ],
        );
        calls.google.breakdown_rows.insert(
            "segments.ad_network_type",
            vec![
                json!({"campaign":{"id":"701"},"segments":{"date":end().to_string(),"adNetworkType":"SEARCH"},"metrics":{"impressions":"90","clicks":"9","conversions":1.0,"costMicros":"9000000","conversionsValue":5.0}}),
                json!({"campaign":{"id":"701"},"segments":{"date":end().to_string(),"adNetworkType":"SEARCH_PARTNERS"},"metrics":{"impressions":"10","clicks":"1","conversions":0.0,"costMicros":"1000000","conversionsValue":0.0}}),
            ],
        );
    }
    worker::run(&f.ctx, &job).await.unwrap();

    let days = format!("segments.date BETWEEN '{}' AND '{}'", day_before(29), end());
    assert_eq!(
        queries(&f, "metrics.search_impression_share"),
        [format!("SELECT campaign.id, segments.date, metrics.impressions, metrics.search_impression_share, metrics.search_budget_lost_impression_share, metrics.search_rank_lost_impression_share FROM campaign WHERE campaign.advertising_channel_type = 'SEARCH' AND {days}")]
    );
    assert_eq!(
        queries(&f, "segments.conversion_action_name"),
        [format!("SELECT campaign.id, segments.date, segments.conversion_action_name, metrics.conversions, metrics.conversions_value FROM campaign WHERE {days}")]
    );
    assert_eq!(
        queries(&f, "segments.ad_network_type"),
        [format!("SELECT campaign.id, segments.date, segments.ad_network_type, metrics.impressions, metrics.clicks, metrics.conversions, metrics.cost_micros, metrics.conversions_value FROM campaign WHERE {days}")]
    );

    let last = day_number(end());
    assert_eq!(
        rows(&f, "SELECT printf('%d %d %d %s %d %s %d %s %d',day,campaign_id,impressions,COALESCE(shown,'none'),shown_under,COALESCE(lost_budget,'none'),lost_budget_over,COALESCE(lost_rank,'none'),lost_rank_over) AS line FROM campaign_search_share_days ORDER BY day DESC").await,
        [
            format!("{last} 701 520 0.52 0 0.31 0 0.17 0"),
            format!("{} 701 40 none 1 none 1 0.05 0", last - 1),
            format!("{} 701 0 none 0 none 0 none 0", last - 2),
        ]
    );
    assert_eq!(
        rows(&f, "SELECT printf('%d %d %s %.1f %.1f',day,campaign_id,action,conversions,conversions_value) AS line FROM conversion_action_days ORDER BY action").await,
        [
            format!("{last} 701 Booking form 3.0 150.0"),
            format!("{last} 701 Phone call 1.5 0.0"),
        ]
    );
    assert_eq!(
        rows(&f, "SELECT printf('%d %d %s %d %d %d %.1f %.1f',day,campaign_id,network,impressions,clicks,cost_micros,conversions,conversions_value) AS line FROM campaign_network_days ORDER BY network").await,
        [
            format!("{last} 701 SEARCH 90 9 9000000 1.0 5.0"),
            format!("{last} 701 SEARCH_PARTNERS 10 1 1000000 0.0 0.0"),
        ]
    );
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let state = store
        .breakdown_state(AD, Breakdown::SearchShare)
        .await
        .unwrap();
    assert_eq!(state.last_day, Some(last));
    assert_eq!(state.error, None);

    let totals = store.search_share_totals(AD, last - 6, last).await.unwrap();
    assert_eq!(totals.len(), 1);
    assert_eq!(
        (totals[0].days, totals[0].limit_days, totals[0].exact_days),
        (3, 1, 1)
    );
    let shares = totals[0].shares.unwrap();
    assert_eq!(
        format!(
            "{:.2} {:.2} {:.2}",
            shares.shown, shares.lost_budget, shares.lost_rank
        ),
        "0.52 0.31 0.17"
    );
}

#[tokio::test]
#[serial]
async fn a_breakdown_that_google_refuses_stores_its_reason_and_the_rest_is_collected() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    {
        let mut calls = f.calls.lock().unwrap();
        calls.google.refused_query = Some("metrics.search_impression_share");
        calls.google.breakdown_rows.insert(
            "segments.ad_network_type",
            vec![json!({"campaign":{"id":"701"},"segments":{"date":end().to_string(),"adNetworkType":"SEARCH"},"metrics":{"impressions":"90","clicks":"9","conversions":1.0,"costMicros":"9000000","conversionsValue":5.0}})],
        );
    }
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(
        store
            .updates(AD, day_number(today()), day_number(today()))
            .await
            .unwrap()
            .len(),
        1
    );
    let refused = store
        .breakdown_state(AD, Breakdown::SearchShare)
        .await
        .unwrap();
    assert_eq!(
        refused.error.as_deref(),
        Some("Google Ads does not support this request.")
    );
    assert_eq!(refused.last_day, Some(day_number(end())));
    assert_eq!(queries(&f, "metrics.search_impression_share").len(), 1);
    assert_eq!(
        rows(
            &f,
            "SELECT network AS line FROM campaign_network_days ORDER BY network"
        )
        .await,
        ["SEARCH"]
    );
    assert_eq!(
        store
            .breakdown_state(AD, Breakdown::Networks)
            .await
            .unwrap()
            .error,
        None
    );
    let page = f.page(&format!("{PAGE}/campaigns/701")).await;
    assert!(page.contains(
        r#"id="search-share">Search impression share, last 7 days</h2>
<p class="chart-say">We could not read the Search impression share from Google Ads. Google Ads does not support this request.</p>"#
    ), "{page}");
}

#[tokio::test]
#[serial]
async fn conversions_by_conversion_action_are_not_collected_without_judging_by_conversions() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(queries(&f, "segments.conversion_action_name").len(), 0);
    assert_eq!(queries(&f, "metrics.search_impression_share").len(), 1);
    assert_eq!(queries(&f, "segments.ad_network_type").len(), 1);
}

#[tokio::test]
#[serial]
async fn a_new_ad_account_backfills_each_breakdown_only_over_the_weeks_the_pages_show() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    keysend::monitoring::schedule::collect_history(&f.ctx, &signed.workspace)
        .await
        .unwrap();
    let history = queries(&f, "segments.date, segments.device");
    let range = |query: &String| {
        query
            .split("segments.date BETWEEN ")
            .nth(1)
            .unwrap()
            .to_owned()
    };
    assert_eq!(history.len(), 9, "{history:?}");
    for part in [
        "metrics.search_impression_share",
        "segments.conversion_action_name",
        "segments.ad_network_type",
    ] {
        assert_eq!(
            queries(&f, part).iter().map(range).collect::<Vec<_>>(),
            [
                format!("'{}' AND '{}'", day_before(59), end()),
                format!("'{}' AND '{}'", day_before(83), day_before(60)),
            ],
            "{part}"
        );
    }
    assert_eq!(
        range(&history[0]),
        format!("'{}' AND '{}'", day_before(59), end())
    );
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let state = store
        .breakdown_state(AD, Breakdown::Networks)
        .await
        .unwrap();
    assert_eq!(state.first_day, Some(day_number(day_before(83))));
    assert_eq!(state.last_day, Some(day_number(end())));
}

fn exact(day: u64, campaign_id: i64, impressions: i64, shares: (f64, f64, f64)) -> SearchShareDay {
    SearchShareDay {
        day: day_number(day_before(day)),
        campaign_id,
        impressions,
        shown: Share::Exact(shares.0),
        lost_budget: Share::Exact(shares.1),
        lost_rank: Share::Exact(shares.2),
    }
}

async fn harbour_breakdowns(tracked: bool) -> Fixture {
    let f = pages::harbour(tracked).await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut shares = Vec::new();
    for day in 0..7 {
        shares.push(exact(day, EMERGENCY, 520, (0.52, 0.31, 0.17)));
        shares.push(exact(day, 3, 640, (0.64, 0.06, 0.30)));
        shares.push(if day == 6 {
            SearchShareDay {
                shown: Share::OutsideExactRange,
                lost_budget: Share::Exact(0.02),
                lost_rank: Share::OutsideExactRange,
                ..exact(day, 1, 9, (0.0, 0.0, 0.0))
            }
        } else {
            exact(day, 1, 910, (0.91, 0.02, 0.07))
        });
    }
    let first = day_number(day_before(59));
    let last = day_number(end());
    store
        .save_breakdown(AD, first, last, BreakdownDays::SearchShare(shares))
        .await
        .unwrap();
    store
        .save_breakdown(
            AD,
            first,
            last,
            BreakdownDays::Networks(
                [
                    ("SEARCH", 2000.0, 880.0, 50.0),
                    ("SEARCH_PARTNERS", 349.0, 130.0, 8.0),
                ]
                .into_iter()
                .map(|(network, cost, clicks, conversions)| NetworkDay {
                    day: last,
                    campaign_id: EMERGENCY,
                    network: network.into(),
                    counts: Counts {
                        impressions: 1000.0,
                        clicks,
                        cost,
                        conversions: if tracked { conversions } else { 0.0 },
                        value: 0.0,
                    },
                })
                .collect(),
            ),
        )
        .await
        .unwrap();
    if tracked {
        let mut actions = Vec::new();
        for week in 0..8u64 {
            for (action, conversions) in [("Booking form", 12.0), ("Phone call", 6.0)] {
                actions.push(ActionDay {
                    day: day_number(day_before(week * 7)),
                    campaign_id: EMERGENCY,
                    action: action.into(),
                    conversions,
                    value: 0.0,
                });
            }
        }
        store
            .save_breakdown(AD, first, last, BreakdownDays::ConversionActions(actions))
            .await
            .unwrap();
    }
    f
}

#[tokio::test]
#[serial]
async fn the_ad_account_page_opens_when_every_search_share_day_is_below_ten_percent() {
    let f = pages::harbour(false).await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let shares = (0..7)
        .map(|day| SearchShareDay {
            shown: Share::OutsideExactRange,
            ..exact(day, EMERGENCY, 100, (0.0, 0.25, 0.70))
        })
        .collect();
    store
        .save_breakdown(
            AD,
            day_number(day_before(6)),
            day_number(end()),
            BreakdownDays::SearchShare(shares),
        )
        .await
        .unwrap();

    for (path, days, said) in [
        (
            PAGE.to_owned(),
            7,
            "Google Ads reports under 10% for each of these days and gives no exact share.",
        ),
        (
            format!("{PAGE}?window=30"),
            30,
            "Google Ads reports no exact Search impression share for these days.",
        ),
    ] {
        let page = f.page(&path).await;
        assert!(page.contains(said), "{said}\n{page}");
        assert!(page.contains(&format!("<td>0 of {days}</td>")), "{page}");
    }
}

#[tokio::test]
#[serial]
async fn the_ad_account_page_shows_the_searches_that_its_top_search_campaigns_missed() {
    let f = harbour_breakdowns(true).await;
    let page = f.page(PAGE).await;
    let chart = part(&page, "search-share", "device-shares");
    assert!(chart.starts_with(
        r##"id="search-share">Search impression share, last 7 days</h2>
<p class="chart-say"><b class="campaign-name">Search - Emergency</b> lost 31% of its estimated eligible Search impressions to budget. That is the largest loss in this chart.</p>

<a class="chart-skip" href="#search-share-end">Skip the chart</a>
<figure class="chart chart--stack" aria-labelledby="search-share">"##
    ), "{chart}");
    assert_eq!(count(chart, r#"<div class="chart__row">"#), 3);
    assert!(chart.contains(r#"<div class="chart__row"><a class="campaign-name" href="/ad-accounts/4829153307/campaigns/2">Search - Emergency</a><svg class="chart__bars" height="28" role="group" aria-label="Search - Emergency"><g class="pt" tabindex="0" role="img" aria-label="Search - Emergency: shown for 52% of the estimated eligible Search impressions"><rect class="seg seg--shown" x="0.00%" y="0" width="52.00%" height="28" rx="4"/><text class="seg__label seg__label--shown" x="26.00%" y="18" text-anchor="middle">52%</text>"#), "{chart}");
    assert!(chart.contains(
        r#"<rect class="seg seg--budget" x="52.00%" y="0" width="31.00%" height="28" rx="4"/>"#
    ));
    for row in [
        r#"<tr><th scope="row">Search - Emergency</th><td>52%</td><td>31%</td><td>17%</td><td>7 of 7</td></tr>"#,
        r#"<tr><th scope="row">Search - Implants</th><td>64%</td><td>6%</td><td>30%</td><td>7 of 7</td></tr>"#,
        r#"<tr><th scope="row">Search - Brand</th><td>91%</td><td>2%</td><td>7%</td><td>6 of 7</td></tr>"#,
    ] {
        assert!(chart.contains(row), "{row}");
    }
    assert!(chart.contains(r#"<li><i class="key key--shown"></i>Impression share</li><li><i class="key key--budget"></i>Lost to budget</li><li><i class="key key--rank"></i>Lost to ad rank</li>"#));
    assert!(chart.contains(r#"<span id="search-share-end" tabindex="-1"></span>"#));
    assert!(page.contains(r#"<p class="small"><a href="/ad-accounts/4829153307/breakdowns">More breakdowns: hours, locations, age and gender</a></p>"#));

    let month = f.page(&format!("{PAGE}?window=30")).await;
    assert!(month.contains(r#"id="search-share">Search impression share, last 30 days</h2>"#));
    assert!(month.contains(r#"<a href="/ad-accounts/4829153307/breakdowns?window=30">More breakdowns: hours, locations, age and gender</a>"#));
}

#[tokio::test]
#[serial]
async fn the_campaign_page_shows_its_missed_searches_and_its_network_split() {
    let f = harbour_breakdowns(true).await;
    let page = f.page(&format!("{PAGE}/campaigns/1")).await;
    let chart = part(&page, "search-share", "device-shares");
    assert!(chart.starts_with(
        r#"id="search-share">Search impression share, last 7 days</h2>
<p class="chart-say"><b class="campaign-name">Search - Brand</b> lost 7% of its estimated eligible Search impressions to ad rank. That is the largest loss in this chart. Google Ads gives an exact share for 6 of its 7 days, and these shares use only those days. On 1 of the 7 days, Google Ads reports values outside its exact reporting range. Those days are excluded.</p>"#
    ), "{chart}");
    assert!(chart.contains(
        r#"<div class="chart__row"><span><b class="campaign-name">Search - Brand</b></span>"#
    ));
    assert_eq!(count(chart, r#"<div class="chart__row">"#), 1);
    assert!(!chart.contains(">2%</text>"));
    assert!(chart.contains("<td>6 of 7</td>"));

    let page = f.page(&format!("{PAGE}/campaigns/2")).await;
    let networks = part(&page, "network-shares", "deep-check");
    assert!(networks.starts_with(
        r#"id="network-shares">Spend and conversions by network, last 7 days</h2>
<p class="chart-say"><b>Google Search</b> takes <mark class="chart-say__point">85% of spend and brings 86% of conversions.</mark></p>"#
    ), "{networks}");
    assert!(networks.contains(r#"<tr><th scope="row">Search partners</th><td>$349</td><td>15%</td><td>8</td><td>14%</td></tr>"#));

    let other = f.page(&format!("{PAGE}/campaigns/4")).await;
    assert!(other.contains(
        r#"id="network-shares">Spend and conversions by network, last 7 days</h2>
<p class="chart-say">No network had spend in these days.</p>"#
    ));
    assert!(other.contains(
        r#"id="search-share">Search impression share, last 7 days</h2>
<p class="chart-say">Google Ads has no Search impression share for these days.</p>"#
    ));
}

#[tokio::test]
#[serial]
async fn the_breakdowns_page_shows_the_conversions_of_each_week_by_conversion_action() {
    let f = harbour_breakdowns(true).await;
    let page = f.page(&format!("{PAGE}/breakdowns?window=30")).await;
    assert!(page.contains(r#"<a class="back-link" href="/ad-accounts/4829153307?window=30">Back to Harbour Dental</a>"#));
    assert!(page.contains(r#"<h1 class="heading-xl">More breakdowns</h1>"#));
    let chart = part(&page, "action-weeks", "action-weeks-end");
    assert!(chart.starts_with(
        r#"id="action-weeks">Conversions by conversion action, 8 weeks</h2>
<p class="chart-say"><b>Booking form</b> has <mark class="chart-say__point">67% of the conversions of the last 8 weeks.</mark> The last week is not complete: conversions can still arrive.</p>"#
    ), "{chart}");
    assert_eq!(
        count(
            chart,
            r#"<g class="pt" tabindex="0" role="img" aria-label="#
        ),
        8
    );
    assert!(chart.contains(
        r#"aria-label="Week of 5 Oct (so far): 18 conversions. Booking form: 12. Phone call: 6.""#
    ));
    assert!(chart.contains(r#"<tr><th scope="col">Week</th><th scope="col">Booking form</th><th scope="col">Phone call</th><th scope="col">All conversions</th></tr>"#));
    assert!(chart.contains(
        r#"<tr><th scope="row">Week of 17 Aug</th><td>12</td><td>6</td><td>18</td></tr>"#
    ));
    assert!(chart.contains(r#"<li><i class="key key--act-0"></i>Booking form</li><li><i class="key key--act-1"></i>Phone call</li>"#));
}

#[tokio::test]
#[serial]
async fn an_ad_account_that_is_not_judged_by_conversions_has_no_conversion_word_in_a_breakdown() {
    let f = harbour_breakdowns(false).await;
    let page = f.page(&format!("{PAGE}/breakdowns")).await;
    assert!(!page.contains(r#"id="action-weeks""#));

    let campaign = f.page(&format!("{PAGE}/campaigns/2")).await;
    let charts = part(&campaign, "search-share", "deep-check");
    assert!(!charts.to_lowercase().contains("conversion"), "{charts}");
    assert!(charts.contains(
        r#"id="network-shares">Spend and clicks by network, last 7 days</h2>
<p class="chart-say"><b>Google Search</b> takes <mark class="chart-say__point">85% of spend and brings 87% of clicks.</mark></p>"#
    ), "{charts}");
    assert!(charts.contains("lost 31% of its estimated eligible Search impressions to budget. That is the largest loss in this chart."));
}

#[tokio::test]
#[serial]
async fn the_breakdowns_page_is_not_found_for_an_ad_account_of_another_workspace() {
    let f = harbour_breakdowns(true).await;
    for path in [
        format!("{PAGE}/breakdowns"),
        format!("{PAGE}/breakdowns?window=30"),
    ] {
        let other = f.request.get(&path).add_cookie(f.b.clone()).await;
        assert_eq!(other.status_code(), 404, "{path}");
        assert!(!other.text().contains("Harbour Dental"), "{path}");
        let own = f.request.get(&path).add_cookie(f.a.clone()).await;
        assert_eq!(own.status_code(), 200, "{path}");
    }
}

#[tokio::test]
#[serial]
async fn the_breakdowns_page_goes_back_to_the_ad_account_after_the_trial() {
    let f = harbour_breakdowns(true).await;
    let signed = signed(&f).await;
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started_at=started_at-9*86400 WHERE workspace_id=?",
            vec![signed.workspace.id().into()],
        ))
        .await
        .unwrap();
    for (path, location) in [
        (format!("{PAGE}/breakdowns"), PAGE.to_owned()),
        (
            format!("{PAGE}/breakdowns?window=30"),
            format!("{PAGE}?window=30"),
        ),
    ] {
        let response = f.request.get(&path).add_cookie(f.a.clone()).await;
        assert_eq!(response.status_code(), 303, "{path}");
        assert_eq!(response.header("location"), location, "{path}");
    }
}

#[tokio::test]
#[serial]
async fn a_breakdown_without_rows_is_one_sentence() {
    let f = pages::harbour(true).await;
    let page = f.page(PAGE).await;
    assert!(page.contains(
        r#"id="search-share">Search impression share, last 7 days</h2>
<p class="chart-say">We have not read the Search impression share of this Ad Account yet.</p>"#
    ));
    let breakdowns = f.page(&format!("{PAGE}/breakdowns")).await;
    assert!(breakdowns.contains(
        r#"id="action-weeks">Conversions by conversion action, 12 weeks</h2>
<p class="chart-say">We have not read the conversions by conversion action of this Ad Account yet.</p>"#
    ));
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_breakdown(
            AD,
            day_number(day_before(59)),
            day_number(end()),
            BreakdownDays::ConversionActions(vec![]),
        )
        .await
        .unwrap();
    let breakdowns = f.page(&format!("{PAGE}/breakdowns")).await;
    assert!(breakdowns.contains(
        r#"<p class="chart-say">No conversion action had conversions in the last 12 weeks.</p>"#
    ));
    assert!(!breakdowns.contains("<figure"));
}

#[tokio::test]
#[serial]
async fn removing_an_ad_account_deletes_its_breakdowns() {
    let f = harbour_breakdowns(true).await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let tables = [
        "breakdown_history",
        "campaign_search_share_days",
        "conversion_action_days",
        "campaign_network_days",
    ];
    for table in tables {
        assert_ne!(
            rows(
                &f,
                &format!("SELECT CAST(COUNT(*) AS TEXT) AS line FROM {table}")
            )
            .await,
            ["0"],
            "{table}"
        );
    }
    store.remove_ad_account(AD).await.unwrap();
    for table in tables {
        assert_eq!(
            rows(
                &f,
                &format!("SELECT CAST(COUNT(*) AS TEXT) AS line FROM {table}")
            )
            .await,
            ["0"],
            "{table}"
        );
    }
}

fn counted(clicks: i64, cost: i64, conversions: f64) -> Value {
    json!({"impressions":(clicks * 10).to_string(),"clicks":clicks.to_string(),"conversions":conversions,"costMicros":(cost * 1_000_000).to_string(),"conversionsValue":0.0})
}

fn deep_rows(f: &Fixture, score: i64) {
    let mut calls = f.calls.lock().unwrap();
    let rows = &mut calls.google.breakdown_rows;
    rows.insert(
        "segments.hour",
        vec![
            json!({"segments":{"dayOfWeek":"MONDAY","hour":9},"metrics":counted(40, 80, 4.0)}),
            json!({"segments":{"dayOfWeek":"SUNDAY","hour":23},"metrics":counted(5, 10, 0.0)}),
        ],
    );
    rows.insert(
        "FROM geographic_view",
        vec![
            json!({"geographicView":{"countryCriterionId":"2036"},"segments":{"geoTargetCity":"geoTargetConstants/1000286"},"metrics":counted(30, 60, 3.0)}),
            json!({"geographicView":{"countryCriterionId":"2036"},"segments":{"geoTargetCity":"geoTargetConstants/1000286"},"metrics":counted(5, 10, 1.0)}),
            json!({"geographicView":{"countryCriterionId":"2036"},"metrics":counted(10, 20, 0.0)}),
        ],
    );
    rows.insert(
        "FROM geo_target_constant",
        vec![
            json!({"geoTargetConstant":{"id":"1000286","canonicalName":"Harbour City,New South Wales,Australia"}}),
            json!({"geoTargetConstant":{"id":"2036","canonicalName":"Australia"}}),
        ],
    );
    rows.insert(
        "FROM age_range_view",
        vec![
            json!({"adGroupCriterion":{"ageRange":{"type":"AGE_RANGE_25_34"}},"metrics":counted(20, 40, 2.0)}),
            json!({"adGroupCriterion":{"ageRange":{"type":"AGE_RANGE_25_34"}},"metrics":counted(10, 20, 1.0)}),
            json!({"adGroupCriterion":{"ageRange":{"type":"AGE_RANGE_UNDETERMINED"}},"metrics":counted(15, 30, 1.0)}),
        ],
    );
    rows.insert(
        "FROM gender_view",
        vec![
            json!({"adGroupCriterion":{"gender":{"type":"FEMALE"}},"metrics":counted(25, 50, 2.0)}),
            json!({"adGroupCriterion":{"gender":{"type":"MALE"}},"metrics":counted(20, 40, 2.0)}),
        ],
    );
    rows.insert(
        "segments.conversion_lag_bucket",
        vec![
            json!({"segments":{"conversionLagBucket":"LESS_THAN_ONE_DAY"},"metrics":{"conversions":30.0}}),
            json!({"segments":{"conversionLagBucket":"TWO_TO_THREE_DAYS"},"metrics":{"conversions":10.0}}),
        ],
    );
    calls.google.monitoring_audit_rows.insert(
        "keyword_view".into(),
        vec![
            json!({"campaign":{"id":"701"},"adGroup":{"id":"55"},"adGroupCriterion":{"criterionId":"901","keyword":{"text":"emergency dentist","matchType":"EXACT"},"qualityInfo":{"qualityScore":score,"creativeQualityScore":"AVERAGE","postClickQualityScore":"ABOVE_AVERAGE","searchPredictedCtr":"BELOW_AVERAGE"}},"metrics":counted(30, 40, 3.0)}),
            json!({"campaign":{"id":"701"},"adGroup":{"id":"55"},"adGroupCriterion":{"criterionId":"902","keyword":{"text":"dentist near me","matchType":"PHRASE"}},"metrics":counted(10, 20, 1.0)}),
        ],
    );
}

async fn deep_check_on(f: &Fixture, day: i64) {
    let signed = signed(f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let check =
        keysend::monitoring::deep_check::collect(&Settings::load(&f.ctx).unwrap(), &store, AD, day)
            .await
            .unwrap();
    store.refresh_deep_check(AD, &check, &[]).await.unwrap();
}

#[tokio::test]
#[serial]
async fn the_deep_check_stores_hours_locations_quality_scores_age_gender_and_delay_as_rows() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    deep_rows(&f, 7);
    let day = day_number(today());
    deep_check_on(&f, day).await;

    let days = format!("segments.date BETWEEN '{}' AND '{}'", day_before(29), end());
    let counts = "metrics.impressions, metrics.clicks, metrics.conversions, metrics.cost_micros, metrics.conversions_value";
    for (part, query) in [
        ("segments.hour", format!("SELECT segments.day_of_week, segments.hour, {counts} FROM customer WHERE {days}")),
        ("FROM geographic_view", format!("SELECT geographic_view.country_criterion_id, segments.geo_target_city, {counts} FROM geographic_view WHERE {days} ORDER BY metrics.cost_micros DESC LIMIT 200")),
        ("FROM geo_target_constant", "SELECT geo_target_constant.id, geo_target_constant.canonical_name FROM geo_target_constant WHERE geo_target_constant.id IN (1000286, 2036)".to_owned()),
        ("FROM age_range_view", format!("SELECT ad_group_criterion.age_range.type, {counts} FROM age_range_view WHERE {days}")),
        ("FROM gender_view", format!("SELECT ad_group_criterion.gender.type, {counts} FROM gender_view WHERE {days}")),
        ("segments.conversion_lag_bucket", format!("SELECT segments.conversion_lag_bucket, metrics.conversions FROM customer WHERE segments.date BETWEEN '{}' AND '{}'", day_before(89), end())),
        ("FROM keyword_view", format!("SELECT campaign.id, ad_group_criterion.keyword.text, ad_group.id, ad_group_criterion.criterion_id, ad_group_criterion.keyword.match_type, ad_group_criterion.quality_info.quality_score, ad_group_criterion.quality_info.creative_quality_score, ad_group_criterion.quality_info.post_click_quality_score, ad_group_criterion.quality_info.search_predicted_ctr, {counts} FROM keyword_view WHERE {days} ORDER BY metrics.cost_micros DESC LIMIT 200")),
    ] {
        assert_eq!(queries(&f, part), [query], "{part}");
    }

    assert_eq!(
        rows(&f, "SELECT printf('%d %d %d %d %d %d %.1f',day,weekday,hour,impressions,clicks,cost_micros,conversions) AS line FROM hour_totals ORDER BY weekday").await,
        [
            format!("{day} 0 9 400 40 80000000 4.0"),
            format!("{day} 6 23 50 5 10000000 0.0"),
        ]
    );
    assert_eq!(
        rows(&f, "SELECT printf('%d %d %s %d %d %.1f',day,location_id,name,clicks,cost_micros,conversions) AS line FROM location_totals ORDER BY cost_micros DESC").await,
        [
            format!("{day} 1000286 Harbour City, New South Wales 35 70000000 4.0"),
            format!("{day} 2036 Australia 10 20000000 0.0"),
        ]
    );
    assert_eq!(
        rows(&f, "SELECT printf('%d %s %s %d %d %.1f',day,kind,value,clicks,cost_micros,conversions) AS line FROM demographic_totals ORDER BY kind,value").await,
        [
            format!("{day} age AGE_RANGE_25_34 30 60000000 3.0"),
            format!("{day} age AGE_RANGE_UNDETERMINED 15 30000000 1.0"),
            format!("{day} gender FEMALE 25 50000000 2.0"),
            format!("{day} gender MALE 20 40000000 2.0"),
        ]
    );
    assert_eq!(
        rows(&f, "SELECT printf('%d %s %.1f',day,bucket,conversions) AS line FROM conversion_lag_totals ORDER BY bucket").await,
        [
            format!("{day} LESS_THAN_ONE_DAY 30.0"),
            format!("{day} TWO_TO_THREE_DAYS 10.0"),
        ]
    );
    let quality = "SELECT printf('%d %d %d %d %s %s %d %s %s %s %s',day,campaign_id,ad_group_id,criterion_id,keyword,match_type,cost_micros,COALESCE(quality_score,'none'),COALESCE(ad_relevance,'none'),COALESCE(landing_page,'none'),COALESCE(expected_ctr,'none')) AS line FROM keyword_quality ORDER BY day,criterion_id";
    assert_eq!(
        rows(&f, quality).await,
        [
            format!("{day} 701 55 901 emergency dentist EXACT 40000000 7 AVERAGE ABOVE_AVERAGE BELOW_AVERAGE"),
            format!("{day} 701 55 902 dentist near me PHRASE 20000000 none none none none"),
        ]
    );
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(
        store.conversion_delay(AD).await.unwrap(),
        Some((40.0, 10.0))
    );
    let states = store.breakdown_states(AD).await.unwrap();
    for kind in [
        Breakdown::Hours,
        Breakdown::Locations,
        Breakdown::KeywordQuality,
        Breakdown::Age,
        Breakdown::Gender,
        Breakdown::ConversionDelay,
    ] {
        let state = &states[kind.key()];
        assert_eq!((state.last_day, state.error.as_deref()), (Some(day), None));
    }

    deep_rows(&f, 5);
    deep_check_on(&f, day + 7).await;
    let (scores, compared) = store.keyword_scores(AD, 701, 10).await.unwrap();
    assert!(compared);
    assert_eq!(
        scores
            .iter()
            .map(|row| (
                row.quality.keyword.as_str(),
                row.quality.score,
                row.previous,
                row.known_before
            ))
            .collect::<Vec<_>>(),
        [
            ("emergency dentist", Some(5), Some(7), true),
            ("dentist near me", None, None, true)
        ]
    );
    assert_eq!(rows(&f, quality).await.len(), 4);
    assert_eq!(
        rows(&f, "SELECT CAST(COUNT(*) AS TEXT) AS line FROM hour_totals").await,
        ["2"]
    );
    deep_check_on(&f, day + 14).await;
    assert_eq!(
        rows(
            &f,
            "SELECT DISTINCT CAST(day AS TEXT) AS line FROM keyword_quality ORDER BY day"
        )
        .await,
        [(day + 7).to_string(), (day + 14).to_string()]
    );
}

#[tokio::test]
#[serial]
async fn the_deep_check_reads_its_keywords_without_the_quality_fields_that_google_refuses() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    deep_rows(&f, 7);
    f.calls.lock().unwrap().google.refused_query = Some("quality_info");
    deep_check_on(&f, day_number(today())).await;
    let keyword_queries = queries(&f, "FROM keyword_view");
    assert_eq!(keyword_queries.len(), 2);
    assert!(keyword_queries[0].contains("ad_group_criterion.quality_info.quality_score"));
    assert!(keyword_queries[1]
        .starts_with("SELECT campaign.id, ad_group_criterion.keyword.text, metrics.impressions,"));
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let check = store.latest_deep_check(AD).await.unwrap().unwrap();
    assert_eq!(check.data.audit.keywords.len(), 2);
    assert_eq!(
        store
            .breakdown_state(AD, Breakdown::KeywordQuality)
            .await
            .unwrap()
            .error
            .as_deref(),
        Some("Google Ads does not support this request.")
    );
    assert!(rows(&f, "SELECT keyword AS line FROM keyword_quality")
        .await
        .is_empty());
    assert_eq!(
        rows(&f, "SELECT CAST(COUNT(*) AS TEXT) AS line FROM hour_totals").await,
        ["2"]
    );
    let page = f.page(&format!("{PAGE}/campaigns/701")).await;
    assert!(page.contains(
        r#"id="keyword-quality">Quality score of the keywords with the most spend</h2>
  <p class="chart-say">We could not read the Quality scores from Google Ads. Google Ads does not support this request.</p>"#
    ), "{page}");

    f.calls.lock().unwrap().google.refused_query = Some("FROM age_range_view");
    deep_check_on(&f, day_number(today()) + 7).await;
    let states = store.breakdown_states(AD).await.unwrap();
    assert_eq!(
        states["age"].error.as_deref(),
        Some("Google Ads does not support this request.")
    );
    assert_eq!(states["gender"].error, None);
    assert_eq!(states["keyword_quality"].error, None);
    assert_eq!(
        rows(&f, "SELECT DISTINCT kind AS line FROM demographic_totals").await,
        ["gender"]
    );
}

#[tokio::test]
#[serial]
async fn a_failed_deep_breakdown_is_stored_with_its_reason_and_the_deep_check_goes_on() {
    for (part, key) in [
        ("segments.hour", "hours"),
        ("FROM geographic_view", "locations"),
        ("FROM age_range_view", "age"),
        ("FROM gender_view", "gender"),
        ("segments.conversion_lag_bucket", "conversion_delay"),
    ] {
        let (captured, _subscriber) = capture();
        let f = Fixture::monitoring().await;
        f.prepare_monitoring().await;
        deep_rows(&f, 7);
        f.calls.lock().unwrap().google.failed_query = Some(part);
        let day = day_number(today());
        deep_check_on(&f, day).await;
        let signed = signed(&f).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        assert_eq!(store.latest_deep_check(AD).await.unwrap().unwrap().day, day);
        let states = store.breakdown_states(AD).await.unwrap();
        for (other, state) in &states {
            let expected =
                (other == key).then_some("Google Ads data is unavailable. Try again later.");
            assert_eq!(state.error.as_deref(), expected, "{key}: {other}");
        }
        if key == "hours" {
            let page = f.page(&format!("{PAGE}/breakdowns")).await;
            assert!(
                page.contains("We could not read the hours from Google Ads. Google Ads data is unavailable. Try again later."),
                "{page}"
            );
        }
        let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(
            logs.contains("A part of the Deep check could not be read from Google Ads"),
            "{key}: {logs}"
        );
    }
}

#[tokio::test]
#[serial]
async fn a_failed_quality_score_query_is_stored_and_the_keywords_are_read_without_it() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    deep_rows(&f, 7);
    f.calls.lock().unwrap().google.failed_query = Some("quality_info");
    deep_check_on(&f, day_number(today())).await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(
        store
            .latest_deep_check(AD)
            .await
            .unwrap()
            .unwrap()
            .data
            .audit
            .keywords
            .len(),
        2
    );
    assert_eq!(
        store
            .breakdown_state(AD, Breakdown::KeywordQuality)
            .await
            .unwrap()
            .error
            .as_deref(),
        Some("Google Ads data is unavailable. Try again later.")
    );
}

#[tokio::test]
#[serial]
async fn an_unknown_weekday_or_hour_leaves_out_its_row_and_is_logged_once() {
    let (captured, _subscriber) = capture();
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    deep_rows(&f, 7);
    f.calls.lock().unwrap().google.breakdown_rows.insert(
        "segments.hour",
        vec![
            json!({"segments":{"dayOfWeek":"MONDAY","hour":9},"metrics":counted(40, 80, 4.0)}),
            json!({"segments":{"dayOfWeek":"UNSPECIFIED","hour":9},"metrics":counted(1, 2, 0.0)}),
            json!({"segments":{"dayOfWeek":"TUESDAY","hour":24},"metrics":counted(1, 2, 0.0)}),
        ],
    );
    deep_check_on(&f, day_number(today())).await;
    assert_eq!(
        rows(&f, "SELECT CAST(COUNT(*) AS TEXT) AS line FROM hour_totals").await,
        ["1"]
    );
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert_eq!(
        logs.matches("with an unknown weekday or hour were left out")
            .count(),
        1,
        "{logs}"
    );
    assert!(logs.contains("skipped=2"), "{logs}");
}

#[tokio::test]
#[serial]
async fn the_conversion_delay_is_not_read_without_judging_by_conversions() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    deep_rows(&f, 7);
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    deep_check_on(&f, day_number(today())).await;
    assert!(queries(&f, "segments.conversion_lag_bucket").is_empty());
    assert_eq!(queries(&f, "segments.hour").len(), 1);
    assert_eq!(store.conversion_delay(AD).await.unwrap(), None);
    assert!(!store
        .breakdown_states(AD)
        .await
        .unwrap()
        .contains_key("conversion_delay"));
}

fn totals(clicks: f64, cost: f64, conversions: f64) -> Counts {
    Counts {
        impressions: clicks * 10.0,
        clicks,
        cost,
        conversions,
        value: 0.0,
    }
}

fn keyword(
    criterion_id: i64,
    keyword: &str,
    match_type: &str,
    score: Option<i64>,
) -> KeywordQuality {
    KeywordQuality {
        campaign_id: EMERGENCY,
        ad_group_id: 55,
        criterion_id,
        keyword: keyword.into(),
        match_type: match_type.into(),
        cost: 1000.0 - criterion_id as f64,
        score,
        ad_relevance: score.map(|_| "AVERAGE".into()),
        landing_page: score.map(|_| "ABOVE_AVERAGE".into()),
        expected_ctr: score.map(|_| "BELOW_AVERAGE".into()),
    }
}

pub(super) async fn harbour_deep(tracked: bool) -> Fixture {
    let f = harbour_breakdowns(tracked).await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let day = day_number(today());
    let converted = |conversions: f64| if tracked { conversions } else { 0.0 };
    let mut hours = Vec::new();
    for weekday in 0..5 {
        hours.push(HourTotal {
            weekday,
            hour: 9,
            counts: totals(60.0, 120.0, converted(16.0)),
        });
        hours.push(HourTotal {
            weekday,
            hour: 14,
            counts: totals(30.0, 60.0, converted(6.0)),
        });
    }
    hours.push(HourTotal {
        weekday: 5,
        hour: 20,
        counts: totals(10.0, 20.0, converted(1.0)),
    });
    let demographic = |rows: &[(&str, f64, f64, f64)]| {
        rows.iter()
            .map(|(value, clicks, cost, conversions)| DemographicTotal {
                value: (*value).into(),
                counts: totals(*clicks, *cost, converted(*conversions)),
            })
            .collect::<Vec<_>>()
    };
    let rows = |keywords: Vec<KeywordQuality>| DeepBreakdowns {
        hours: Ok(hours.clone()),
        locations: Ok(vec![
            LocationTotal {
                id: 1000286,
                name: "Harbour City, New South Wales".into(),
                counts: totals(900.0, 2000.0, converted(60.0)),
            },
            LocationTotal {
                id: 1000290,
                name: "Bayside, New South Wales".into(),
                counts: totals(300.0, 1000.0, converted(10.0)),
            },
        ]),
        keywords: Ok(keywords),
        ages: Ok(demographic(&[
            ("AGE_RANGE_25_34", 500.0, 1000.0, 30.0),
            ("AGE_RANGE_35_44", 400.0, 900.0, 20.0),
            ("AGE_RANGE_UNDETERMINED", 100.0, 200.0, 5.0),
        ])),
        genders: Ok(demographic(&[
            ("FEMALE", 300.0, 600.0, 20.0),
            ("MALE", 200.0, 400.0, 10.0),
            ("UNDETERMINED", 500.0, 1000.0, 20.0),
        ])),
        delay: tracked.then(|| {
            Ok(vec![
                ("LESS_THAN_ONE_DAY".into(), 150.0),
                ("ONE_TO_TWO_DAYS".into(), 4.0),
                ("THREE_TO_FOUR_DAYS".into(), 46.0),
                ("UNKNOWN".into(), 9.0),
            ])
        }),
    };
    store
        .save_deep_breakdowns(
            AD,
            day - 7,
            &rows(vec![
                keyword(901, "emergency dentist", "EXACT", Some(7)),
                keyword(903, "tooth pain", "BROAD", None),
                keyword(904, "24 hour dentist", "PHRASE", Some(8)),
            ]),
        )
        .await
        .unwrap();
    store
        .refresh_deep_check(AD, &check(day - 7), &[])
        .await
        .unwrap();
    store
        .save_deep_breakdowns(
            AD,
            day,
            &rows(vec![
                keyword(901, "emergency dentist", "EXACT", Some(5)),
                keyword(902, "dentist open now", "PHRASE", Some(3)),
                keyword(903, "tooth pain", "BROAD", None),
                keyword(904, "24 hour dentist", "PHRASE", Some(8)),
            ]),
        )
        .await
        .unwrap();
    f
}

#[tokio::test]
#[serial]
async fn the_breakdowns_page_draws_hours_locations_age_and_gender_of_the_deep_check() {
    let f = harbour_deep(true).await;
    let page = f.page(&format!("{PAGE}/breakdowns")).await;
    assert!(page.contains(
        r#"<h2 class="heading-l" id="deep-check">From the Deep check of 12 October</h2>"#
    ));

    let hours = part(&page, "hours", "hours-end");
    assert!(hours.starts_with(
        r#"id="hours">Conversions by hour and weekday, 30 days</h2>
<p class="chart-say">Most conversions come <mark class="chart-say__point">between 09:00 and 12:00</mark>: 80 of 111. 5 blocks of 3 hours have the fewest: 0 each. Hours are in the time zone of the Ad Account.</p>"#
    ), "{hours}");
    assert!(hours.contains(r#"<figure class="chart chart--grid" aria-labelledby="hours">"#));
    assert_eq!(count(hours, r#"<rect class="cell "#), 168);
    assert_eq!(
        count(
            hours,
            r#"<g class="pt" tabindex="0" role="img" aria-label="#
        ),
        7
    );
    assert!(hours.contains(r#"aria-label="Monday: 22 conversions, the most at 09:00 (16).""#));
    assert!(hours.contains(
        r#"<rect class="cell cell--4" x="37.50%" y="0" width="4.17%" height="18" rx="3"><title>Mon 09:00: 16 conversions</title></rect>"#
    ));
    assert!(hours.contains("<caption>Conversions in each hour of each weekday, 30 days</caption>"));
    assert!(hours.contains(r#"<tr><th scope="row">Saturday</th><td>0</td>"#));

    let locations = part(&page, "locations", "locations-end");
    assert!(locations.starts_with(
        r#"id="locations">Spend and conversions by location, 30 days</h2>
<p class="chart-say"><b>Harbour City, New South Wales</b> takes <mark class="chart-say__point">12% of spend and brings 16% of conversions.</mark></p>"#
    ), "{locations}");
    assert!(locations.contains(r#"<tr><th scope="row">Harbour City, New South Wales</th><td>$2,000</td><td>12%</td><td>60</td><td>16%</td></tr>"#), "{locations}");
    assert!(locations.contains(r#"<tr><th scope="row">Other locations</th><td>$14,053</td><td>82%</td><td>314</td><td>82%</td></tr>"#));
    assert!(locations.contains(r#"aria-label="Other locations: 82% of spend, $14,053 of $17,053""#));
    assert!(locations.contains(r#"<tr><th scope="row">Bayside, New South Wales</th><td>$1,000</td><td>6%</td><td>10</td><td>3%</td></tr>"#));
    assert_eq!(count(locations, r#"<div class="chart__row">"#), 3);

    let ages = part(&page, "ages", "genders");
    assert!(ages.starts_with(
        r#"id="ages">Spend and conversions by age group, 30 days</h2>
<p class="chart-say"><b>35 to 44</b> takes <mark class="chart-say__point">43% of spend and brings 36% of conversions.</mark></p>"#
    ), "{ages}");
    assert!(ages.contains(
        r#"<tr><th scope="row">Unknown</th><td>$200</td><td>10%</td><td>5</td><td>9%</td></tr>"#
    ));

    assert!(page.contains(
        r#"id="genders">Spend and conversions by gender, 30 days</h2>
<p class="chart-say">The gender data is too thin: Google Ads knows the gender for 50% of clicks.</p>"#
    ));
    assert_eq!(count(&page, r#"<a class="chart-skip""#), 4);
}

#[tokio::test]
#[serial]
async fn the_breakdowns_of_an_ad_account_that_is_not_judged_by_conversions_use_clicks() {
    let f = harbour_deep(false).await;
    let page = f.page(&format!("{PAGE}/breakdowns")).await;
    let charts = &page[page.find(r#"id="deep-check""#).unwrap()..page.find("</main>").unwrap()];
    assert!(!charts.to_lowercase().contains("conversion"), "{charts}");
    assert!(charts.contains(
        r#"id="hours">Clicks by hour and weekday, 30 days</h2>
<p class="chart-say">Most clicks come <mark class="chart-say__point">between 09:00 and 12:00</mark>: 300 of 460. 5 blocks of 3 hours have the fewest: 0 each. Hours are in the time zone of the Ad Account.</p>"#
    ), "{charts}");
    assert!(charts.contains(r#"id="locations">Spend and clicks by location, 30 days</h2>"#));
    assert!(charts.contains(
        r#"id="ages">Spend and clicks by age group, 30 days</h2>
<p class="chart-say"><b>25 to 34</b> takes <mark class="chart-say__point">48% of spend and brings 50% of clicks.</mark></p>"#
    ));
    let account = f.page(PAGE).await;
    assert!(!account.contains(r#"<p class="chart-note">About"#));
}

#[tokio::test]
#[serial]
async fn the_campaign_page_lists_quality_scores_and_the_ad_account_page_states_the_delay() {
    let f = harbour_deep(true).await;
    let page = f.page(&format!("{PAGE}/campaigns/2")).await;
    let keywords = &page[page.find(r#"id="keyword-quality""#).unwrap()..];
    assert!(keywords.starts_with(
        r#"id="keyword-quality">Quality score of the keywords with the most spend</h2>
  <p class="chart-say">1 of the 3 keywords with the most spend has a Quality score of 4 or lower.</p>"#
    ), "{keywords}");
    assert_eq!(
        count(
            keywords,
            r#"<p class="chart-say">Each change is against the Deep check before.</p>"#
        ),
        1
    );
    assert!(!keywords.contains("Not available"));
    assert!(!keywords.contains("to compare with"));
    assert!(keywords.contains(r#"<ul class="plain-list facts facts--4">
    <li class="facts__head" aria-hidden="true"><span>Keyword</span><div><span>Quality score</span><span>Expected clickthrough rate</span><span>Ad relevance</span><span>Landing page experience</span></div></li>"#), "{keywords}");
    let rows = keywords
        .split("<li>\n")
        .skip(1)
        .map(|row| {
            row.split("</li>")
                .next()
                .unwrap()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .take(4)
        .collect::<Vec<_>>();
    let parts = r#"<div><dt>Expected clickthrough rate</dt><dd><span class="level level--below">Below average</span></dd></div> <div><dt>Ad relevance</dt><dd><span class="level level--average">Average</span></dd></div> <div><dt>Landing page experience</dt><dd><span class="level level--above">Above average</span></dd></div> </dl>"#;
    let scored = |keyword: &str, kind: &str, score: i64, change: &str| {
        format!(
            r#"<div class="facts__name">{keyword} <span class="tag">{kind}</span></div> <dl> <div class="fact--bar"><dt>Quality score</dt><dd><b>{score} of 10</b><svg class="score" viewBox="0 0 118 8" width="118" height="8" role="img" aria-label="Quality score {score} of 10"><path class="score__track" d="M0 4h118"/><path class="score__value" d="M0 4h{}"/></svg>{change}</dd></div> {parts}"#,
            score * 12 - 2
        )
    };
    assert_eq!(
        rows,
        [
            scored(
                "emergency dentist",
                "Exact match",
                5,
                r#"<span class="level level--lower">Down 2</span>"#
            ),
            scored(
                "dentist open now",
                "Phrase match",
                3,
                r#"<span class="fact__detail">New keyword</span>"#
            ),
            r#"<div class="facts__name">tooth pain <span class="tag">Broad match</span></div> <dl> <div class="fact--wide"><dt>Quality score</dt><dd><span class="fact__detail">Google Ads gives no Quality score yet. It needs more information about this keyword. You do not need to do anything.</span></dd></div> </dl>"#.to_owned(),
            scored(
                "24 hour dentist",
                "Phrase match",
                8,
                r#"<span class="level level--equal">No change</span>"#
            ),
        ]
    );
    let other = f.page(&format!("{PAGE}/campaigns/1")).await;
    assert!(other.contains(
        r#"<p class="chart-say">The Deep check has no keyword with spend for this Campaign.</p>"#
    ));

    let account = f.page(PAGE).await;
    let weeks = part(&account, "cost-weeks", "device-shares");
    assert!(weeks.contains(
        r#"<p class="chart-note">About 23% of conversions arrive more than 2 days after the ad was shown. The newest days are not complete.</p>"#
    ), "{weeks}");
}

#[tokio::test]
#[serial]
async fn the_breakdowns_page_before_the_first_deep_check_is_sentences_only() {
    let f = pages::harbour(false).await;
    let page = f.page(&format!("{PAGE}/breakdowns")).await;
    assert!(page.contains(r#"<h2 class="heading-l" id="deep-check">No Deep check yet</h2>"#));
    for sentence in [
        r#"id="hours">Clicks by hour and weekday, 30 days</h2>
<p class="chart-say">We have not read the hours of this Ad Account yet.</p>"#,
        r#"id="locations">Spend and clicks by location, 30 days</h2>
<p class="chart-say">We have not read the locations of this Ad Account yet.</p>"#,
        r#"id="ages">Spend and clicks by age group, 30 days</h2>
<p class="chart-say">We have not read the age data of this Ad Account yet.</p>"#,
        r#"id="genders">Spend and clicks by gender, 30 days</h2>
<p class="chart-say">We have not read the gender data of this Ad Account yet.</p>"#,
    ] {
        assert!(page.contains(sentence), "{sentence}");
    }
    assert!(!page.contains("<figure"));
}

#[tokio::test]
#[serial]
async fn the_weekly_email_shows_spend_against_results_by_campaign_as_a_table_of_bars() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO campaign_days(workspace_id,ad_account_customer_id,day,campaign_id,device,impressions,clicks,cost_micros,conversions,conversions_value) SELECT workspace_id,ad_account_customer_id,day,702,1,100,30,30000000,1.0,0 FROM campaign_days WHERE day>?",
            vec![(day_number(end()) - 7).into()],
        ))
        .await
        .unwrap();
    let stored = store
        .monitoring_days(AD, day_number(end()) - 6, day_number(end()))
        .await
        .unwrap();
    let mut campaigns = store.campaigns(AD).await.unwrap();
    campaigns.push(Campaign {
        id: 702,
        name: "Display - Remarketing".into(),
        ..campaign()
    });
    let metrics = |judged: bool| {
        keysend::monitoring::metrics::window_metrics(
            &stored,
            day_number(end()),
            Window::SevenDays,
            None,
            judged,
        )
    };
    let parts = keysend::monitoring::email::campaign_parts(&stored, &campaigns, &metrics(true));
    assert_eq!(
        parts
            .iter()
            .map(|part| (part.name.as_str(), part.spend, part.results))
            .collect::<Vec<_>>(),
        [
            ("Search - Real", 70.0, 7.0),
            ("Display - Remarketing", 210.0, 7.0)
        ]
    );
    assert_eq!(
        keysend::monitoring::email::campaign_parts(&stored, &campaigns, &metrics(false))[1].results,
        210.0
    );
    let updates = vec![serde_json::from_value(json!({
        "day": day_number(today()),
        "summary": "Spend was steady.",
        "lines": [],
        "more_campaigns": 0,
        "metrics": metrics(true),
        "not_useful": false,
        "detection": keysend::monitoring::detection::detect(&stored, &campaigns, &[], day_number(end()), day_number(oldest()), AllFollowAccount { judged: true }, "USD"),
    }))
    .unwrap()];
    let message = keysend::monitoring::email::message(
        keysend::language::Language::En,
        &keysend::monitoring::email::content(
            day_number(today()),
            vec![keysend::monitoring::email::block(
                AD.into(),
                "Harbour Dental".into(),
                "USD".into(),
                metrics(true),
                updates,
                None,
                parts,
            )],
            Vec::new(),
        )
        .unwrap(),
        "https://keysend.example",
        &keysend::translation::Translations::english(),
    )
    .unwrap();
    let chart = &message.html[message
        .html
        .find("Spend and conversions by Campaign")
        .unwrap()..];
    assert!(chart.starts_with(
        r#"Spend and conversions by Campaign</td>
                </tr>
                <tr>
                  <td style="padding:0 0 14px; font-size:16px; line-height:24px; color:#5C5470;"><b style="padding:1px 5px; border-radius:4px; background:#EEE9F6; color:#3F1D85; font-weight:700;">Display - Remarketing</b> takes 75% of spend and brings 50% of conversions.</td>"#
    ), "{chart}");
    let cells = chart
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("<td") && (line.contains("bgcolor") || line.contains("nowrap"))
        })
        .map(|line| {
            let width = line.split('"').nth(1).filter(|_| line.contains("bgcolor"));
            let label = line.rsplit('>').nth(1).unwrap().trim_end_matches("</td");
            format!("{} {label}", width.unwrap_or("label"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        cells,
        [
            "55% &nbsp;",
            "label 75% of spend",
            "37% &nbsp;",
            "label 50% of conversions",
            "18% &nbsp;",
            "label 25% of spend",
            "37% &nbsp;",
            "label 50% of conversions",
        ],
        "{chart}"
    );
    assert!(chart.contains(r##"<td width="55%" height="12" bgcolor="#5126A6" style="height:12px; background:#5126A6; border-radius:4px; font-size:0; line-height:0;">&nbsp;</td>"##));
    assert!(chart.contains(r##"<td width="37%" height="12" bgcolor="#D6ED79" style="height:12px; background:#D6ED79; border-radius:4px; font-size:0; line-height:0;">&nbsp;</td>"##));
    assert!(!message.html.contains("<svg") && !message.html.contains("<img"));
    assert!(message.text.contains(
        "\nSpend and conversions by Campaign\nDisplay - Remarketing takes 75% of spend and brings 50% of conversions.\n- Display - Remarketing: 75% of spend, 50% of conversions\n- Search - Real: 25% of spend, 50% of conversions\n"
    ), "{}", message.text);

    worker::run(&f.ctx, &job).await.unwrap();
    let calls = f.calls.lock().unwrap();
    let html = calls.emails[0]["html"].as_str().unwrap();
    assert!(html.contains(">Northwind Dental</h2>"));
    assert!(!html.contains("by Campaign") && !html.contains("of spend"));
}

async fn export_of(
    f: &Fixture,
    judged: bool,
) -> (
    Vec<keysend::monitoring::export::ChartImage>,
    keysend::monitoring::export::DeepCharts,
) {
    use keysend::controllers::monitoring::{
        export_changes, export_charts, first_read_day, Collected,
    };
    let signed = signed(f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let last = day_number(end());
    let days = store
        .monitoring_days(AD, first_read_day(last), last)
        .await
        .unwrap();
    let campaigns = store.campaigns(AD).await.unwrap();
    let first = store
        .history_state(AD)
        .await
        .unwrap()
        .unwrap()
        .first_day
        .unwrap();
    let collected = Collected {
        text: keysend::language::english(),
        id: AD,
        days: &days,
        campaigns: &campaigns,
        first_day: day_number(first),
        end: last,
        currency: "USD",
        judged,
        window: Window::SevenDays,
        translations: &keysend::translation::Translations::english(),
    };
    let changes = export_changes(&store, &collected, None).await.unwrap();
    export_charts(
        &store,
        &collected,
        &changes,
        &rules_of(&store, &days, last).await,
    )
    .await
    .unwrap()
}

async fn rules_of(
    store: &WorkspaceStore<'_>,
    days: &[keysend::monitoring::metrics::CampaignDay],
    last: i64,
) -> keysend::monitoring::campaign_settings::Resolved {
    let monitoring = store.monitoring_settings(AD).await.unwrap();
    let (state, _) = keysend::monitoring::tracking::of_days(
        days,
        last,
        store.conversion_actions(AD).await.unwrap(),
        monitoring.judge_by_conversions,
    );
    store.campaign_rules(AD, state, &monitoring).await.unwrap()
}

fn words_of(page: &str) -> String {
    let mut text = String::new();
    let mut inside = false;
    for letter in page.chars() {
        match letter {
            '<' => inside = true,
            '>' => inside = false,
            letter if !inside => text.push(letter),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn drawn(charts: &[keysend::monitoring::export::ChartImage]) -> Vec<(&str, bool)> {
    charts
        .iter()
        .map(|chart| (chart.title.as_str(), chart.svg.is_some()))
        .collect()
}

#[tokio::test]
#[serial]
async fn the_export_has_every_chart_of_the_ad_account_page_and_of_the_breakdowns_page_in_page_order(
) {
    let f = harbour_deep(true).await;
    let (charts, deep) = export_of(&f, true).await;
    assert_eq!(charts[0].note.as_deref(), Some("The band shows one standard deviation above and below the weekday average, using up to 8 earlier matching weekdays and requiring at least 4. It describes past variation, not a forecast or confidence interval. A red point marks a larger deviation beyond the alert threshold. Crossing the shaded band alone does not trigger it. A dotted line marks a change in the Ad Account."));
    assert!(charts[1..4].iter().all(|chart| chart.note.is_none()));
    assert_eq!(
        drawn(&charts),
        [
            ("Spend per day, against its usual range", true),
            ("Conversions per day, against its usual range", true),
            ("Cost per conversion per day, against its usual range", true),
            ("Clicks per day, against its usual range", true),
            ("Cost per conversion, 12 weeks", true),
            ("Spend and conversions by Campaign, last 7 days", true),
            ("Search impression share, last 7 days", true),
            ("Spend and conversions by device, last 7 days", false),
            ("Conversions by conversion action, 8 weeks", true),
        ]
    );
    assert_eq!(deep.date.as_deref(), Some("12 October"));
    assert_eq!(
        drawn(&deep.charts),
        [
            ("Conversions by hour and weekday, 30 days", true),
            ("Spend and conversions by location, 30 days", true),
            ("Spend and conversions by age group, 30 days", true),
            ("Spend and conversions by gender, 30 days", false),
        ]
    );
    assert_eq!(
        charts[4].note.as_deref(),
        Some("About 23% of conversions arrive more than 2 days after the ad was shown. The newest days are not complete.")
    );
    let mut trends = Vec::new();
    for metric in ["spend", "conversions", "cost-per-conversion", "clicks"] {
        trends.push(words_of(&f.page(&format!("{PAGE}?metric={metric}")).await));
    }
    let account = words_of(&f.page(PAGE).await);
    let breakdowns = words_of(&f.page(&format!("{PAGE}/breakdowns")).await);
    for (index, chart) in charts.iter().chain(&deep.charts).enumerate() {
        let page = match index {
            0..4 => &trends[index],
            4..8 => &account,
            _ => &breakdowns,
        };
        assert!(
            page.contains(&format!("{} {}", chart.title, chart.sentence)),
            "{}: {}",
            chart.title,
            chart.sentence
        );
    }
}

#[tokio::test]
#[serial]
async fn the_export_of_an_ad_account_that_is_not_judged_by_conversions_has_no_conversion_chart_and_no_conversion_word(
) {
    let f = harbour_deep(false).await;
    let (charts, deep) = export_of(&f, false).await;
    assert_eq!(
        drawn(&charts),
        [
            ("Spend per day, against its usual range", true),
            ("Clicks per day, against its usual range", true),
            ("Cost per click per day, against its usual range", true),
            ("Impressions per day, against its usual range", true),
            ("Spend and clicks by Campaign, last 7 days", true),
            ("Search impression share, last 7 days", true),
            ("Spend and clicks by device, last 7 days", false),
        ]
    );
    assert_eq!(
        drawn(&deep.charts),
        [
            ("Clicks by hour and weekday, 30 days", true),
            ("Spend and clicks by location, 30 days", true),
            ("Spend and clicks by age group, 30 days", true),
            ("Spend and clicks by gender, 30 days", false),
        ]
    );
    for chart in charts.iter().chain(&deep.charts) {
        let all = format!(
            "{} {} {} {}",
            chart.title,
            chart.sentence,
            chart.svg.as_deref().unwrap_or_default(),
            chart.note.as_deref().unwrap_or_default()
        );
        assert!(!all.to_lowercase().contains("conversion"), "{all}");
    }
}

#[tokio::test]
#[serial]
async fn the_export_of_an_ad_account_without_a_deep_check_has_the_daily_charts_and_the_sentences_of_the_breakdowns_page(
) {
    let f = pages::harbour(true).await;
    let (charts, deep) = export_of(&f, true).await;
    assert_eq!(
        drawn(&charts),
        [
            ("Spend per day, against its usual range", true),
            ("Conversions per day, against its usual range", true),
            ("Cost per conversion per day, against its usual range", true),
            ("Clicks per day, against its usual range", true),
            ("Cost per conversion, 12 weeks", true),
            ("Spend and conversions by Campaign, last 7 days", true),
            ("Search impression share, last 7 days", false),
            ("Spend and conversions by device, last 7 days", false),
            ("Conversions by conversion action, 12 weeks", false),
        ]
    );
    assert_eq!(deep.date, None);
    assert_eq!(
        deep.charts
            .iter()
            .map(|chart| (chart.title.as_str(), chart.sentence.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                "Conversions by hour and weekday, 30 days",
                "We have not read the hours of this Ad Account yet."
            ),
            (
                "Spend and conversions by location, 30 days",
                "We have not read the locations of this Ad Account yet."
            ),
            (
                "Spend and conversions by age group, 30 days",
                "We have not read the age data of this Ad Account yet."
            ),
            (
                "Spend and conversions by gender, 30 days",
                "We have not read the gender data of this Ad Account yet."
            ),
        ]
    );
    assert!(deep.charts.iter().all(|chart| chart.svg.is_none()));
}

#[tokio::test]
#[serial]
async fn the_export_of_a_campaign_has_the_charts_of_its_page_and_its_quality_scores() {
    use keysend::controllers::monitoring::{
        campaign_deep_check, campaign_export_charts, export_changes, first_read_day, Collected,
    };
    let f = harbour_deep(true).await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let last = day_number(end());
    let days = store
        .monitoring_days(AD, first_read_day(last), last)
        .await
        .unwrap();
    let campaigns = store.campaigns(AD).await.unwrap();
    let collected = Collected {
        text: keysend::language::english(),
        id: AD,
        days: &days,
        campaigns: &campaigns,
        first_day: day_number(
            store
                .history_state(AD)
                .await
                .unwrap()
                .unwrap()
                .first_day
                .unwrap(),
        ),
        end: last,
        currency: "USD",
        judged: true,
        window: Window::SevenDays,
        translations: &keysend::translation::Translations::english(),
    };
    let campaign = campaigns
        .iter()
        .find(|campaign| campaign.id == EMERGENCY)
        .unwrap();
    let changes = export_changes(&store, &collected, Some(campaign))
        .await
        .unwrap();
    let rules = rules_of(&store, &days, last).await;
    let charts = campaign_export_charts(&store, &collected, campaign, &changes, &rules)
        .await
        .unwrap();
    assert_eq!(
        drawn(&charts),
        [
            ("Spend per day, against its usual range", true),
            ("Conversions per day, against its usual range", true),
            ("Cost per conversion per day, against its usual range", true),
            ("Clicks per day, against its usual range", true),
            ("Cost per conversion, 12 weeks", true),
            ("Search impression share, last 7 days", true),
            ("Spend and conversions by device, last 7 days", false),
            ("Spend and conversions by network, last 7 days", true),
        ]
    );
    assert_eq!(
        charts[0].changes,
        keysend::monitoring::export::ChartChanges::Listed(vec![
            keysend::monitoring::export::DatedChange {
                date: "25 September".to_owned(),
                text: "Daily budget changed from $300.00 to $340.00".to_owned(),
                follow_up: Some("KeySend cannot judge it. In the 14 days after the change, against the 14 days before it. Spend $4,627 (before $4,014), conversions 93 (before 74), cost per conversion $49.75 (before $54.24). Other changes to this Campaign in the same 28 days: 1. The effect cannot be separated.".to_owned()),
            },
            keysend::monitoring::export::DatedChange {
                date: "4 October".to_owned(),
                text: "Bid strategy changed from maximize clicks to maximize conversions"
                    .to_owned(),
                follow_up: Some("KeySend checks the result on 20 October.".to_owned()),
            },
        ])
    );
    let page = words_of(&f.page(&format!("{PAGE}/campaigns/2")).await);
    let mut pages = vec![page.clone()];
    for metric in ["conversions", "cost-per-conversion", "clicks"] {
        pages.push(words_of(
            &f.page(&format!("{PAGE}/campaigns/2?metric={metric}")).await,
        ));
    }
    for chart in &charts {
        let said = format!("{} {}", chart.title, chart.sentence);
        assert!(
            pages.iter().any(|page| page.contains(&said)),
            "{}",
            chart.title
        );
    }
    assert!(page.contains("KeySend cannot judge it. In the 14 days after the change"));
    let deep = campaign_deep_check(&store, &collected, campaign, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deep.date, "5 October");
    assert_eq!(deep.sections.len(), 1);
    let quality = &deep.sections[0];
    assert_eq!(
        quality.sentence,
        "1 of the 3 keywords with the most spend has a Quality score of 4 or lower."
    );
    assert_eq!(
        quality.note,
        "Each change is against the Deep check before."
    );
    assert_eq!(
        quality
            .rows
            .iter()
            .map(|row| (
                row.name.as_str(),
                row.facts[0].value.as_str(),
                row.facts[0].badge.as_ref().map(|badge| badge.text.as_str()),
                row.facts[0].detail.as_str(),
            ))
            .collect::<Vec<_>>(),
        [
            ("emergency dentist", "5 of 10", Some("Down 2"), ""),
            ("dentist open now", "3 of 10", None, "New keyword"),
            (
                "tooth pain",
                "",
                None,
                "Google Ads gives no Quality score yet. It needs more information about this keyword. You do not need to do anything."
            ),
            ("24 hour dentist", "8 of 10", Some("No change"), ""),
        ]
    );
}

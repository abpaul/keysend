use std::collections::BTreeMap;

use super::*;
use keysend::{
    language::Language,
    monitoring::audit::AuditRow,
    monitoring::{
        detection::{CampaignMovement, Detection},
        update::{CampaignLine, Update},
    },
    workspace::{history::AccountChange, WorkspaceStore},
};

const BRAND: i64 = 1;
const EMERGENCY: i64 = 2;
const IMPLANTS: i64 = 3;
const REMARKETING: i64 = 4;
const PAGE: &str = "/ad-accounts/4829153307";

const EMERGENCY_SPEND: [i64; 30] = [
    296, 288, 301, 279, 262, 294, 299, 291, 303, 285, 271, 258, 297, 338, 331, 336, 334, 318, 301,
    329, 337, 332, 339, 341, 322, 329, 338, 340, 339, 340,
];
const EMERGENCY_CONVERSIONS: [i64; 30] = [
    6, 5, 7, 4, 3, 6, 6, 5, 7, 6, 4, 3, 6, 6, 7, 6, 6, 4, 5, 7, 6, 6, 7, 7, 6, 8, 9, 9, 10, 9,
];

fn share(total: i64, day: i64) -> i64 {
    total / 7 + i64::from(day < total % 7)
}

fn day(offset: u64) -> NaiveDate {
    end().checked_sub_days(Days::new(offset)).unwrap()
}

fn change(day: NaiveDate, campaign: i64, kind: &str, old: Value, new: Value) -> AccountChange {
    AccountChange {
        changed_at: format!("{day} 10:00:00"),
        resource_name: format!("customers/{AD}/changeEvents/{campaign}-{day}"),
        change_resource_name: None,
        resource_type: kind.into(),
        operation: "UPDATE".into(),
        campaign_id: Some(campaign),
        changed_fields: String::new(),
        old_value: old.to_string(),
        new_value: new.to_string(),
    }
}

fn row(name: &str, cost: f64, clicks: f64, impressions: f64, conversions: f64) -> AuditRow {
    AuditRow {
        name: name.into(),
        counts: Counts {
            impressions,
            clicks,
            conversions,
            cost,
            value: 0.0,
        },
        ..Default::default()
    }
}

fn finding(campaign: i64, key: &str, detail: &str, spend: f64) -> Finding {
    Finding {
        campaign_id: Some(campaign),
        rule: findings::Finding {
            key: key.into(),
            title: key.into(),
            detail: detail.into(),
            subjects: vec![],
            spend,
        },
    }
}

const JOBS_KEY: &str = "wasted-term:3:dental implants jobs";
const JOBS_ACTION: &str = "Add dental implants jobs as a negative keyword in Search - Implants.";
const JOBS_FINDING: &str =
    "dental implants jobs: $186 spend, 74 clicks, 0 conversions in the last 30 days.";
const PAGE_KEY: &str = "landing-page:2:home";
const PAGE_ACTION: &str =
    "Send the ads in Search - Emergency to the emergency page, not to the home page.";
const PAGE_FINDING: &str = "The home page: $2,958 spend, 34 conversions, $87.00 each. The emergency page: $6,410 spend, 152 conversions, $42.17 each, in the last 30 days.";
const FREE_FINDING: &str =
    "free dental care: $96 spend, 61 clicks, 0 conversions in the last 30 days.";

async fn store_update(
    f: &Fixture,
    day: i64,
    summary: &str,
    highlight: Option<&str>,
    lines: &[(i64, &str, &str)],
) {
    let signed = signed(f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let metrics = store
        .window_metrics(AD, day - 1, Window::SevenDays, None)
        .await
        .unwrap();
    let judge = metrics.current.conversions.is_some();
    store
        .save_update(
            AD,
            &Update {
                day,
                summary: summary.into(),
                highlight: highlight.map(Into::into),
                cause: None,
                lines: lines
                    .iter()
                    .map(|(campaign_id, _, text)| CampaignLine {
                        campaign_id: *campaign_id,
                        text: (*text).into(),
                        cause: None,
                    })
                    .collect(),
                metrics: metrics.clone(),
                detection: Detection {
                    day,
                    currency: "USD".into(),
                    metrics: metrics.clone(),
                    movements: vec![],
                    yesterday: vec![],
                    campaigns: lines
                        .iter()
                        .map(|(campaign_id, name, _)| CampaignMovement {
                            campaign_id: *campaign_id,
                            name: (*name).into(),
                            metrics: metrics.clone(),
                            movements: vec![],
                            reasons: vec![],
                            account_changes: vec![],
                            follow_ups: vec![],
                        })
                        .collect(),
                    judge_by_conversions: judge,
                    causes: vec![],
                },
                not_useful: false,
                written_without_ai: lines.is_empty(),
                more_campaigns: 0,
            },
        )
        .await
        .unwrap();
}

pub(super) async fn harbour(tracked: bool) -> Fixture {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE ad_accounts SET name='Harbour Dental' WHERE customer_id=?",
            vec![AD.into()],
        ))
        .await
        .unwrap();
    store
        .save_ad_account_metadata(AD, "USD", "UTC")
        .await
        .unwrap();
    let weeks = [
        (
            BRAND,
            "Search - Brand",
            [620, 610],
            [28, 27],
            [540, 528],
            [9400, 9000],
        ),
        (
            IMPLANTS,
            "Search - Implants",
            [936, 880],
            [12, 10],
            [450, 420],
            [12000, 11500],
        ),
        (
            REMARKETING,
            "Display - Remarketing",
            [215, 310],
            [6, 10],
            [310, 310],
            [12000, 11400],
        ),
        (
            EMERGENCY,
            "Search - Emergency",
            [2349, 2290],
            [58, 41],
            [1010, 930],
            [28000, 27000],
        ),
    ];
    let campaigns = weeks
        .iter()
        .map(|(id, name, ..)| Campaign {
            id: *id,
            name: (*name).into(),
            ..campaign()
        })
        .collect::<Vec<_>>();
    let mut days = Vec::new();
    for offset in 0..60i64 {
        for (id, _, spend, conversions, clicks, impressions) in weeks {
            let week = usize::from(offset >= 7);
            let position = 6 - offset % 7;
            let (cost, converted) = if id == EMERGENCY {
                match usize::try_from(29 - offset) {
                    Ok(index) => (
                        EMERGENCY_SPEND[index] as f64,
                        EMERGENCY_CONVERSIONS[index] as f64,
                    ),
                    Err(_) => (290.0, 6.0),
                }
            } else {
                (
                    share(spend[week] * 1_000_000, position) as f64 / 1_000_000.0,
                    share(conversions[week], position) as f64,
                )
            };
            days.push(CampaignDay {
                day: day(offset as u64),
                campaign_id: id,
                device: 1,
                counts: Counts {
                    impressions: share(impressions[week], position) as f64,
                    clicks: share(clicks[week], position) as f64,
                    cost,
                    conversions: if tracked { converted } else { 0.0 },
                    value: 0.0,
                },
            });
        }
    }
    let changes = [
        change(
            day(16),
            EMERGENCY,
            "CAMPAIGN_BUDGET",
            json!({"campaignBudget":{"amountMicros":"300000000"}}),
            json!({"campaignBudget":{"amountMicros":"340000000"}}),
        ),
        change(
            day(7),
            EMERGENCY,
            "CAMPAIGN",
            json!({"campaign":{"biddingStrategyType":"MAXIMIZE_CLICKS"}}),
            json!({"campaign":{"biddingStrategyType":"MAXIMIZE_CONVERSIONS"}}),
        ),
        change(
            day(3),
            REMARKETING,
            "CAMPAIGN_BUDGET",
            json!({"campaignBudget":{"amountMicros":"45000000"}}),
            json!({"campaignBudget":{"amountMicros":"20000000"}}),
        ),
    ];
    store
        .save_history(AD, oldest(), end(), &campaigns, &days, &changes)
        .await
        .unwrap();
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started=1 WHERE workspace_id=?",
            vec![signed.workspace.id().into()],
        ))
        .await
        .unwrap();
    f
}

async fn deep_check(f: &Fixture, tracked: bool) {
    let signed = signed(f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut check = check(day_number(end()));
    check.data.currency = "USD".into();
    check.data.audit.conversion_actions = Some(u32::from(tracked));
    check.judge_by_conversions = tracked;
    check.by_campaign = BTreeMap::from([(
        EMERGENCY,
        Audit {
            search_terms: vec![
                row("free dental care", 96.0, 61.0, 900.0, 0.0),
                row("emergency dentist", 1240.0, 520.0, 6000.0, 28.0),
                row("emergency dentist near me", 880.0, 310.0, 4000.0, 31.0),
            ],
            keywords: vec![row("emergency dentist", 3960.0, 1560.0, 20000.0, 82.0)],
            landing_pages: vec![
                row(
                    "https://harbourdental.example/",
                    2958.0,
                    1150.0,
                    15000.0,
                    34.0,
                ),
                row(
                    "https://harbourdental.example/emergency",
                    6410.0,
                    2610.0,
                    30000.0,
                    152.0,
                ),
            ],
            ads: vec![
                row(
                    "Same-day emergency appointments (9001)",
                    6100.0,
                    1930.0,
                    19694.0,
                    104.0,
                ),
                row(" (9002)", 3200.0, 1240.0, 26957.0, 58.0),
            ],
            campaigns: vec![AuditRow {
                id: Some(EMERGENCY),
                ..row("Search - Emergency", 9368.0, 3760.0, 45000.0, 186.0)
            }],
            ..Default::default()
        },
    )]);
    check.findings = vec![
        finding(IMPLANTS, JOBS_KEY, JOBS_FINDING, 186.0),
        finding(EMERGENCY, PAGE_KEY, PAGE_FINDING, 2958.0),
        finding(
            EMERGENCY,
            "wasted-term:2:free dental care",
            FREE_FINDING,
            96.0,
        ),
    ];
    check.findings[0].rule.title = "A search term has spend and no conversions".into();
    check.findings[1].rule.title = "A landing page of Search - Emergency has a high cost".into();
    let recommendation = |key: &str, action: &str, evidence: &str, campaign| Recommendation {
        finding_key: key.into(),
        action: action.into(),
        reason: "The Finding shows spend without results.".into(),
        evidence: evidence.into(),
        measure: String::new(),
        intent: String::new(),
        subject: String::new(),
        confidence: Default::default(),
        campaign_id: Some(campaign),
        state: RecommendationState::Open,
    };
    store
        .refresh_deep_check(
            AD,
            &check,
            &[
                recommendation(JOBS_KEY, JOBS_ACTION, JOBS_FINDING, IMPLANTS),
                recommendation(PAGE_KEY, PAGE_ACTION, PAGE_FINDING, EMERGENCY),
            ],
        )
        .await
        .unwrap();
}

const SUMMARY: &str = "Spend was steady this week at $4,120, and conversions rose from 88 to 104. Most of the gain came from Search - Emergency after its bid strategy changed on 4 October. Display - Remarketing is the one to watch.";
const EMERGENCY_LINE: &str = "58 conversions, up from 41, at $40.50 each.";

pub(super) async fn normal_day() -> Fixture {
    let f = harbour(true).await;
    deep_check(&f, true).await;
    let today = day_number(today());
    store_update(
        &f,
        today - 7,
        "Search - Emergency changed its bid strategy on 4 October. We cannot judge the change yet.",
        None,
        &[(
            EMERGENCY,
            "Search - Emergency",
            "The bid strategy changed on 4 October.",
        )],
    )
    .await;
    store_update(&f, today - 1, update::QUIET, None, &[]).await;
    store_update(
        &f,
        today,
        SUMMARY,
        Some("conversions rose from 88 to 104"),
        &[
            (EMERGENCY, "Search - Emergency", EMERGENCY_LINE),
            (
                REMARKETING,
                "Display - Remarketing",
                "Conversions fell from 10 to 6 after the daily budget went from $45 to $20 on 8 October.",
            ),
        ],
    )
    .await;
    f
}

fn count(page: &str, text: &str) -> usize {
    page.matches(text).count()
}

#[tokio::test]
#[serial]
async fn the_ad_account_page_shows_the_update_the_metrics_the_campaigns_and_the_recommendations() {
    let f = normal_day().await;
    let page = f.page(PAGE).await;
    assert!(page.contains("<title>Harbour Dental | KeySend</title>"));
    assert!(page.contains(r#"<h1 class="heading-xl">Harbour Dental</h1>"#));
    assert!(page.contains("Today's Update <span>Monday 12 October</span>"));
    assert!(page.contains(r#"<p class="lead">Spend was steady this week at $4,120, and <mark>conversions rose from 88 to 104</mark>. Most of the gain came from <a class="campaign-name" href="/ad-accounts/4829153307/campaigns/2">Search - Emergency</a> after its bid strategy changed on 4 October. <a class="campaign-name" href="/ad-accounts/4829153307/campaigns/4">Display - Remarketing</a> is the one to watch.</p>"#), "{page}");
    assert_eq!(count(&page, "<mark>"), 1);
    assert!(page.contains(
        r#"<li><a class="campaign-name" href="/ad-accounts/4829153307/campaigns/2">Search - Emergency</a>: 58 conversions, up from 41, at $40.50 each.</li>"#
    ));
    assert!(page.contains(
        r#"<form class="export-form" method="get" action="/ad-accounts/4829153307/export.pdf">"#
    ));
    assert!(page.contains(r#"<a href="/ad-accounts/4829153307/settings">Settings</a>"#));
    assert!(page.contains(r#"<h2 class="heading-m" id="window">Last 7 days</h2>"#));
    assert!(
        page.contains(r#"<p class="muted">5 to 11 October, compared with the 7 days before.</p>"#)
    );
    assert!(page.contains(
        r#"<nav class="switch" aria-label="Window">
  <a href="/ad-accounts/4829153307" data-swap aria-label="Show the last 7 days" aria-current="true">7 days</a><a href="/ad-accounts/4829153307?window=30" data-swap aria-label="Show the last 30 days">30 days</a>
</nav>"#
    ), "{page}");
    for metric in [
        r#"<dt>Spend</dt><dd class="metric__value">$4,120</dd><dd class="metric__change"><b>up 0.7%</b> from $4,090</dd>"#,
        r#"<dt>Conversions</dt><dd class="metric__value">104</dd><dd class="metric__change"><b>up 18%</b> from 88</dd>"#,
        r#"<dt>Cost per conversion</dt><dd class="metric__value">$39.62</dd><dd class="metric__change"><b>down 15%</b> from $46.48</dd>"#,
        r#"<dt>Clicks</dt><dd class="metric__value">2,310</dd><dd class="metric__change"><b>up 5.6%</b> from 2,188</dd>"#,
    ] {
        assert!(page.contains(metric), "{metric}\n{page}");
    }
    assert!(page.contains(r#"<dd class="metric__spark"><svg class="spark spark--metric" viewBox="0 0 100 32" preserveAspectRatio="none" role="img" aria-label="Spend per day from 5 Oct to 11 Oct: "#), "{page}");
    assert_eq!(count(&page, r#"<svg class="spark spark--metric""#), 4);
    let rows = [
        ("Search - Emergency", 2),
        ("Search - Implants", 3),
        ("Search - Brand", 1),
        ("Display - Remarketing", 4),
    ]
    .map(|(name, id)| {
        page.find(&format!(
            r#"<th scope="row"><a class="campaign-name" href="/ad-accounts/4829153307/campaigns/{id}">{name}</a></th>"#
        ))
        .unwrap_or_else(|| panic!("{name}\n{page}"))
    });
    assert!(rows.is_sorted());
    assert!(page.contains(r#"<td class="num">$2,349 <span class="was">was $2,290</span></td><td class="num">58 <span class="was">was 41</span></td><td class="num">$40.50 <span class="was">was $55.85</span></td>"#));
    assert!(page.contains(
        r#"<details class="panel" id="recommendations">
  <summary><h2 class="panel__title">Recommendations <span class="panel__count">(2)</span></h2></summary>"#
    ), "{page}");
    assert!(page.contains(&format!(
        r#"<dl class="parts">
          <div><dt>Finding</dt><dd>A search term has spend and no conversions</dd></div>
          <div><dt>Impact</dt><dd>The Finding shows spend without results.</dd></div>
          <div class="parts__action"><dt>Suggested action</dt><dd>Add dental implants jobs as a negative keyword in <a class="campaign-name" href="/ad-accounts/4829153307/campaigns/3">Search - Implants</a>.</dd></div>
          <div><dt>Evidence</dt><dd>{JOBS_FINDING}</dd></div>
        </dl>"#
    )), "{page}");
    assert!(page.contains(&format!(
        r#"<span class="visually-hidden">: {JOBS_ACTION}</span>"#
    )));
    assert!(page.contains(r#"<div><dt>Finding</dt><dd>A landing page of <a class="campaign-name" href="/ad-accounts/4829153307/campaigns/2">Search - Emergency</a> has a high cost</dd></div>"#));
    let evidence = [PAGE_FINDING, JOBS_FINDING].map(|text| {
        page.find(&format!("<dt>Evidence</dt><dd>{text}</dd>"))
            .unwrap()
    });
    assert!(
        evidence.is_sorted(),
        "the Finding with the most spend is first"
    );
    assert!(page.contains(
        r#"<ol class="plain-list top">
      
      <li><span>Send the ads in <a class="campaign-name" href="/ad-accounts/4829153307/campaigns/2">Search - Emergency</a> to the emergency page, not to the home page.</span></li>"#
    ), "{page}");
    assert!(page.contains(r##"<a href="/ad-accounts/4829153307?recommendations=open#recommendations">Open all Recommendations (2)</a>"##));
    let open = f.page(&format!("{PAGE}?recommendations=open")).await;
    assert!(open.contains(r#"<details class="panel" id="recommendations" open>"#));
    assert!(page.contains("<h3>Sunday 11 October</h3>"));
    assert!(page.contains("<h3>Sunday 11 October</h3>\n        <p>Nothing notable changed.</p>"));
    assert!(page.contains("<h3>Monday 5 October</h3>"));
    assert!(!page.contains(r#"<details class="more">"#));
    assert!(!page.contains("kite-"));
    assert_eq!(count(&page, "<script"), 2);
    assert!(page.contains(r#"<script defer src="/static/"#) && page.contains("/window-switch.js"));
    assert!(page.contains("/popover.js"));

    let month = f.page(&format!("{PAGE}?window=30")).await;
    assert!(month.contains(r#"<h2 class="heading-m" id="window">Last 30 days</h2>"#));
    assert!(month.contains(
        r#"<p class="muted">12 September to 11 October, compared with the 30 days before.</p>"#
    ));
    assert!(month.contains(r#"<a href="/ad-accounts/4829153307?window=30" data-swap aria-label="Show the last 30 days" aria-current="true">30 days</a>"#));
    assert!(month.contains(r##"<a href="/ad-accounts/4829153307?window=30&amp;recommendations=open#recommendations">Open all Recommendations (2)</a>"##));
    assert!(month.contains(
        r#"action="/ad-accounts/4829153307/export.pdf">
  <input type="hidden" name="window" value="30">"#
    ));
    assert!(month.contains(
        r#"<a class="campaign-name" href="/ad-accounts/4829153307/campaigns/2?window=30">Search - Emergency</a>"#
    ));
}

#[tokio::test]
#[serial]
async fn the_pages_show_the_latest_three_updates_and_keep_the_rest_in_a_closed_list() {
    let f = harbour(true).await;
    let today = day_number(today());
    for offset in 0..6 {
        store_update(
            &f,
            today - offset,
            &format!("Update number {offset}."),
            None,
            &[(
                EMERGENCY,
                "Search - Emergency",
                &format!("Line number {offset}."),
            )],
        )
        .await;
    }
    let page = f.page(PAGE).await;
    let earlier = &page[page.find(r#"id="earlier""#).unwrap()..];
    assert!(
        earlier.contains(
            r#"<details class="more">
      <summary>Show 2 more Updates</summary>
      <ol class="plain-list updates" start="4">"#
        ),
        "{earlier}"
    );
    let places = [
        "<p>Update number 1.</p>",
        "<p>Update number 3.</p>",
        r#"<details class="more">"#,
        "<p>Update number 4.</p>",
        "<p>Update number 5.</p>",
        "</details>",
    ]
    .map(|text| {
        earlier
            .find(text)
            .unwrap_or_else(|| panic!("{text}\n{earlier}"))
    });
    assert!(places.is_sorted());
    assert_eq!(count(earlier, "<li>"), 5);

    let campaign = f.page(&format!("{PAGE}/campaigns/2")).await;
    let updates = &campaign[campaign.find(r#"id="updates""#).unwrap()..];
    assert!(
        updates.contains("<summary>Show 3 more Updates</summary>"),
        "{updates}"
    );
    let places = [
        "<p>Line number 0.</p>",
        "<p>Line number 2.</p>",
        r#"<details class="more">"#,
        "<p>Line number 3.</p>",
        "<p>Line number 5.</p>",
        "</details>",
    ]
    .map(|text| {
        updates
            .find(text)
            .unwrap_or_else(|| panic!("{text}\n{updates}"))
    });
    assert!(places.is_sorted());
}

#[tokio::test]
#[serial]
async fn the_export_button_keeps_the_window_and_uses_the_saved_language() {
    let f = harbour(true).await;
    store_update(&f, day_number(today()), "Update number 0.", None, &[]).await;
    let page = f.page(&format!("{PAGE}?window=30")).await;
    assert!(page.contains(&format!(
        r#"<form class="export-form" method="get" action="{PAGE}/export.pdf">
  <input type="hidden" name="window" value="30">"#
    )));
    let export_form = page
        .split(r#"class="export-form""#)
        .nth(1)
        .unwrap()
        .split("</form>")
        .next()
        .unwrap();
    assert!(!export_form.contains("select"));
    assert!(!export_form.contains(r#"name="language""#));
    let campaign = f.page(&format!("{PAGE}/campaigns/2")).await;
    assert!(campaign.contains(&format!(
        r#"action="{PAGE}/campaigns/2/export.pdf">
  <input type="hidden" name="window" value="7">"#
    )));
    let signed = signed(&f).await;
    WorkspaceStore::new(&f.ctx.db, &signed.workspace)
        .set_user_language(Language::Ro)
        .await
        .unwrap();
    let romanian = f.page(PAGE).await;
    assert!(romanian.contains(r#"<html lang="ro">"#));
    assert!(romanian.contains(r#"value="ro" lang="ro" aria-current="true""#));
    let export_form = romanian
        .split(r#"class="export-form""#)
        .nth(1)
        .unwrap()
        .split("</form>")
        .next()
        .unwrap();
    assert!(!export_form.contains("select"));
}

#[tokio::test]
#[serial]
async fn earlier_quiet_days_in_a_row_show_as_one_nothing_notable_changed() {
    let f = harbour(true).await;
    let today = day_number(today());
    store_update(&f, today, "Update number 0.", None, &[]).await;
    store_update(&f, today - 1, update::QUIET, None, &[]).await;
    store_update(&f, today - 2, update::QUIET, None, &[]).await;
    let line = [(EMERGENCY, "Search - Emergency", "Line number 3.")];
    store_update(&f, today - 3, "Update number 3.", None, &line).await;
    store_update(&f, today - 4, update::QUIET, None, &[]).await;
    let page = f.page(PAGE).await;
    let earlier = &page[page.find(r#"id="earlier""#).unwrap()..];
    let places = [
        "<h3>10 to 11 October</h3>\n        <p>Nothing notable changed.</p>",
        "<h3>Friday 9 October</h3>\n        <p>Update number 3.</p>",
        "<h3>Thursday 8 October</h3>\n        <p>Nothing notable changed.</p>",
    ]
    .map(|text| {
        earlier
            .find(text)
            .unwrap_or_else(|| panic!("{text}\n{earlier}"))
    });
    assert!(places.is_sorted());
    assert_eq!(count(earlier, "<li>"), 3);
    assert!(!earlier.contains("in the last 7 days"));
}

#[tokio::test]
#[serial]
async fn a_quiet_day_shows_one_sentence_with_the_kite_and_no_highlight() {
    let f = harbour(true).await;
    store_update(&f, day_number(today()), update::QUIET, None, &[]).await;
    let page = f.page(PAGE).await;
    assert!(page.contains(r#"<p class="lead">Nothing notable changed in the last 7 days. Spend, clicks and conversions are inside their normal range.</p>"#));
    assert!(page.contains("images/kite-welcome.webp"));
    assert!(!page.contains("<mark>"));
    let update = page.split("</article>").next().unwrap();
    assert!(update.contains(r#"<p class="lead">"#));
    assert!(!update.contains(r#"<ul class="list">"#));
    assert!(page.contains(r#"<dd class="metric__value">$4,120</dd>"#));
    assert!(page.contains("No earlier Updates yet."));
}

#[tokio::test]
#[serial]
async fn the_first_session_shows_the_numbers_before_the_first_update() {
    let f = harbour(true).await;
    let page = f.page(PAGE).await;
    assert!(page.contains(
        r#"<h1 class="heading-xl"><span class="caption">Step 4 of 4</span>Harbour Dental</h1>"#
    ));
    assert!(page.contains(r#"<span class="waiting">We are reading your Ad Account</span><br>Your first Update appears here in about a minute."#));
    assert!(page.contains(
        r#"<a href="/ad-accounts/4829153307" data-turbo="false">Check for the Update</a>"#
    ));
    assert!(page.contains("images/kite-delivering.webp"));
    assert!(page.contains(r#"<dd class="metric__value">$4,120</dd>"#));
    assert!(page.contains("Search - Emergency</a>"));
    assert!(page
        .contains("No Recommendations yet. They appear here after we have read the Ad Account."));
    assert!(page.contains(
        "The Export is a PDF of this Ad Account for the last 7 days. We never send it to anyone."
    ));
    assert!(!page.contains("Earlier Updates"));
    assert!(!page.contains("Not useful"));

    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store.history_failed(AD, "Google refused").await.unwrap();
    let failed = f.page(PAGE).await;
    assert!(failed.contains("We could not read this Ad Account from Google Ads."));
    assert!(!failed.contains("Google refused"));
    assert!(!failed.contains("kite-"));
}

#[tokio::test]
#[serial]
async fn an_ad_account_that_is_not_judged_by_conversions_shows_one_notice_and_no_conversion_metric()
{
    let f = harbour(false).await;
    deep_check(&f, false).await;
    store_update(
        &f,
        day_number(today()),
        "Spend was steady this week at $4,120, and clicks rose from 2,188 to 2,310.",
        Some("clicks rose from 2,188 to 2,310"),
        &[(
            EMERGENCY,
            "Search - Emergency",
            "1,010 clicks, up from 930.",
        )],
    )
    .await;
    let page = f.page(PAGE).await;
    assert_eq!(count(&page, r#"class="notice""#), 1);
    assert!(page.contains(r#"<h2 class="notice__title">Conversion tracking is not set up</h2>"#));
    assert!(page.contains(
        "Google Ads has no active conversion action for this Ad Account, so results cannot be judged."
    ));
    for metric in [
        r#"<dt>Spend</dt><dd class="metric__value">$4,120</dd>"#,
        r#"<dt>Clicks</dt><dd class="metric__value">2,310</dd><dd class="metric__change"><b>up 5.6%</b> from 2,188</dd>"#,
        r#"<dt>Cost per click</dt><dd class="metric__value">$1.78</dd><dd class="metric__change"><b>down 4.6%</b> from $1.87</dd>"#,
        r#"<dt>Impressions</dt><dd class="metric__value">61,400</dd><dd class="metric__change"><b>up 4.2%</b> from 58,900</dd>"#,
    ] {
        assert!(page.contains(metric), "{metric}\n{page}");
    }
    assert!(page.contains(r#"<th scope="col" class="num">Cost per click</th>"#));
    assert!(page.contains(r#"<td class="num">1,010 <span class="was">was 930</span></td><td class="num">$2.33 <span class="was">was $2.46</span></td>"#));
    assert!(!page.contains("Cost per conversion"));
    assert!(!page.contains("Conversions"));
    assert!(page.contains("<mark>clicks rose from 2,188 to 2,310</mark>"));

    let campaign = f.page(&format!("{PAGE}/campaigns/2")).await;
    assert_eq!(count(&campaign, r#"class="notice""#), 1);
    assert!(campaign.contains(r#"<li class="facts__head" aria-hidden="true"><span>Search term</span><div><span>Spend</span><span>Clicks</span><span>Cost per click</span></div></li>"#));
    assert!(campaign.contains(
        r#"<p class="chart-say">The search term with the most spend is emergency dentist: $1,240 for 520 clicks.</p>"#
    ));
    let deep = &campaign[campaign.find(r#"id="deep-check""#).unwrap()..];
    assert!(!deep.to_lowercase().contains("conversion"), "{deep}");
    assert!(!deep.contains("level--flag"));
    assert!(!campaign.contains("Conversions"));
    assert!(!campaign.contains("Cost per conversion"));
    assert!(!campaign.contains("Bid strategy changed"));

    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let judged = f.page(PAGE).await;
    assert_eq!(count(&judged, r#"class="notice""#), 0);
    assert!(judged.contains("<dt>Conversions</dt>"));
}

#[tokio::test]
#[serial]
async fn after_the_trial_the_page_keeps_the_updates_and_offers_the_subscription() {
    let f = normal_day().await;
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
    let page = f.page(PAGE).await;
    assert!(page.contains(r#"<p class="lead">Your Trial ended on 11 October. We have stopped writing Updates for this Ad Account.</p>"#), "{page}");
    assert!(page.contains("It costs $49 per month for up to 5 Ad Accounts."));
    assert!(page.contains(r#"<a class="button" href="/subscription">Start the Subscription</a>"#));
    assert!(page.contains(r#"<h2 class="heading-m" id="earlier">Updates from your Trial</h2>"#));
    assert!(page.contains("<h3>Monday 12 October</h3>"));
    assert!(
        page.contains("Spend was steady this week at $4,120, and conversions rose from 88 to 104.")
    );
    assert!(page.contains("<h3>Sunday 11 October</h3>"));
    assert!(!page.contains("Export PDF"));
    assert!(!page.contains("metric__value"));
    assert!(!page.contains("Not useful"));
    let campaign = f
        .request
        .get(&format!("{PAGE}/campaigns/2"))
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(campaign.status_code(), 303);
    assert_eq!(campaign.header("location"), PAGE);
    assert!(f
        .page(&format!("{PAGE}/settings"))
        .await
        .contains("Settings for Harbour Dental"));
}

#[tokio::test]
#[serial]
async fn the_campaign_page_shows_its_updates_its_chart_its_findings_and_the_deep_check() {
    let f = normal_day().await;
    let page = f.page(&format!("{PAGE}/campaigns/2")).await;
    assert!(page.contains(
        r#"<a class="back-link" href="/ad-accounts/4829153307">Back to Harbour Dental</a>"#
    ));
    assert!(page.contains(
        r#"<h1 class="heading-xl">Search - Emergency</h1>
  <div class="actions">
    <form class="export-form" method="get" action="/ad-accounts/4829153307/campaigns/2/export.pdf">"#
    ), "{page}");
    assert!(page.contains(
        r#"<h2 class="heading-m" id="window">Last 7 days</h2>
    <nav class="switch" aria-label="Window">
  <a href="/ad-accounts/4829153307/campaigns/2" data-swap aria-label="Show the last 7 days" aria-current="true">7 days</a><a href="/ad-accounts/4829153307/campaigns/2?window=30" data-swap aria-label="Show the last 30 days">30 days</a>
</nav>"#
    ), "{page}");
    assert!(page.contains(r#"<p class="small muted">An Update always compares the last 7 days with the 7 days before.</p>"#) || !page.contains("update-today"));
    assert!(page.contains(&format!(
        "<h3>Monday 12 October</h3>\n        <p>{EMERGENCY_LINE}</p>"
    )));
    let places = [
        r#"id="recommendations""#,
        r#"<dl class="metrics cell""#,
        r#"id="trend""#,
        r#"id="updates""#,
        r#"id="deep-check""#,
    ]
    .map(|text| page.find(text).unwrap_or_else(|| panic!("{text}\n{page}")));
    assert!(
        places.is_sorted(),
        "the Metrics come before the charts and the Updates"
    );
    assert!(page.contains("<p>The bid strategy changed on 4 October.</p>"));
    assert!(!page.contains("Conversions fell from 10 to 6"));
    for metric in [
        r#"<dt>Spend</dt><dd class="metric__value">$2,349</dd><dd class="metric__change"><b>up 2.6%</b> from $2,290</dd>"#,
        r#"<dt>Conversions</dt><dd class="metric__value">58</dd><dd class="metric__change"><b>up 41%</b> from 41</dd>"#,
        r#"<dt>Cost per conversion</dt><dd class="metric__value">$40.50</dd><dd class="metric__change"><b>down 27%</b> from $55.85</dd>"#,
        r#"<dt>Clicks</dt><dd class="metric__value">1,010</dd><dd class="metric__change"><b>up 8.6%</b> from 930</dd>"#,
    ] {
        assert!(page.contains(metric), "{metric}\n{page}");
    }
    assert!(page.contains("<li>25 September: Daily budget changed from $300.00 to $340.00."));
    assert!(page.contains("KeySend cannot judge it."));
    assert!(page.contains(
        "Other changes to this Campaign in the same 28 days: 1. The effect cannot be separated."
    ));
    assert!(page.contains(
        "<li>4 October: Bid strategy changed from maximize clicks to maximize conversions."
    ));
    assert!(page.contains("KeySend checks the result on 20 October."));
    assert!(!page.contains("$45 to $20"));
    assert!(page.contains(r#"<summary><h2 class="panel__title">Recommendations <span class="panel__count">(1)</span> and other Findings <span class="panel__count">(1)</span></h2></summary>"#), "{page}");
    assert!(page.contains(r#"<div><dt>Finding</dt><dd>A landing page of <b class="campaign-name">Search - Emergency</b> has a high cost</dd></div>
          <div><dt>Impact</dt><dd>The Finding shows spend without results.</dd></div>
          <div class="parts__action"><dt>Suggested action</dt><dd>Send the ads in <b class="campaign-name">Search - Emergency</b> to the emergency page, not to the home page.</dd></div>"#), "{page}");
    assert!(page.contains(r#"<h3 class="heading-s" id="findings">Other Findings</h3>"#));
    assert!(!page.contains(r#"<a class="campaign-name""#));
    assert!(!page.contains(JOBS_ACTION));
    assert!(page.contains(&format!("<li>{FREE_FINDING}</li>")));
    assert_eq!(count(&page, PAGE_FINDING), 1);
    assert!(page.contains("From the Deep check of 11 October"));
    let deep = &page[page.find(r#"id="deep-check""#).unwrap()..];
    let terms = [
        r#"<div class="facts__name">emergency dentist</div>"#,
        r#"<div class="facts__name">emergency dentist near me</div>"#,
        r#"<div class="facts__name">free dental care</div>"#,
    ]
    .map(|term| deep.find(term).unwrap());
    assert!(terms.is_sorted());
    assert!(deep.contains(
        r#"id="search-terms">Search terms with the most spend</h2>
  <p class="chart-say">1 of the 3 search terms with the most spend has spend and no conversions: $96.00.</p>"#
    ), "{deep}");
    assert!(deep.contains(r#"<li class="facts__head" aria-hidden="true"><span>Search term</span><div><span>Spend</span><span>Clicks</span><span>Conversions</span></div></li>"#));
    assert!(deep.contains(r#"<div class="fact--bar"><dt>Spend</dt><dd><b>$880</b><svg class="meter" width="100%" height="8" role="img" aria-label="Spend $880"><rect class="meter__track" width="100%" height="8" rx="4"/><rect class="meter__value" width="70.97%" height="8" rx="4"/></svg></dd></div>
        <div><dt>Clicks</dt><dd><b>310</b></dd></div>
        <div><dt>Conversions</dt><dd><b>31</b></dd></div>"#), "{deep}");
    assert!(deep.contains(r#"<div><dt>Conversions</dt><dd><b>0</b><span class="level level--flag">Spend, no conversions</span></dd></div>"#));
    assert_eq!(count(deep, "level--flag"), 1);
    assert!(deep.contains(
        r#"id="keywords">Keywords with the most spend</h2>
  <p class="chart-say">The keyword with the most spend has a cost per conversion of $48.29, below the Campaign average of $50.37.</p>"#
    ), "{deep}");
    assert!(deep.contains(r#"<div><dt>Cost per conversion</dt><dd><b>$48.29</b><span class="fact__detail">from 82 conversions</span><span class="level level--lower">Below the Campaign average</span></dd></div>"#));
    assert!(deep.contains(
        r#"id="landing-pages">Landing pages with the most spend</h2>
  <p class="chart-say">1 of the 2 landing pages with the most spend has a conversion rate below the Campaign rate of 4.9%.</p>"#
    ), "{deep}");
    assert!(deep.contains(r#"<div class="facts__name facts__name--url">harbourdental.example/emergency</div>
      <dl>
        <div class="fact--bar"><dt>Conversion rate</dt><dd><b>5.8%</b><svg class="meter" width="100%" height="8" role="img" aria-label="Conversion rate 5.8%. Campaign rate 4.9%"><rect class="meter__track" width="100%" height="8" rx="4"/><rect class="meter__value" width="100.00%" height="8" rx="4"/><rect class="meter__mark" x="84.94%" y="-3" width="2" height="14"/></svg><span class="fact__detail">152 conversions from 2,610 clicks</span><span class="level level--higher">Above the Campaign rate</span></dd></div>
        <div><dt>Spend</dt><dd><b>$6,410</b></dd></div>"#), "{deep}");
    assert!(deep
        .contains(r#"<p class="facts__key"><i class="key key--mark"></i>Campaign rate: 4.9%</p>"#));
    assert_eq!(
        count(deep, "KeySend does not read your website analytics: bounce rate and time on page are not available."),
        1
    );
    assert!(deep.contains(
        r#"id="ads">Ads with the most spend</h2>
  <p class="chart-say">Same-day emergency appointments has the highest click rate of the 2 ads with impressions: 9.8%.</p>"#
    ), "{deep}");
    assert!(deep.contains(r#"<div class="facts__name">Ad without a name</div>
      <dl>
        <div class="fact--bar"><dt>Click rate</dt><dd><b>4.6%</b><svg class="meter" width="100%" height="8" role="img" aria-label="Click rate 4.6%"><rect class="meter__track" width="100%" height="8" rx="4"/><rect class="meter__value" width="46.94%" height="8" rx="4"/></svg><span class="fact__detail">1,240 clicks from 26,957 impressions</span></dd></div>"#), "{deep}");
    assert_eq!(count(deep, "<summary>What you can do</summary>"), 4);
    assert_eq!(count(deep, "<summary>What this shows</summary>"), 4);
    assert_eq!(count(deep, "<b>Data limitation:</b>"), 4);
    assert!(deep.contains("<li>A term that does not fit your business: add it as a negative keyword in Google Ads. Your ads do not appear for a negative keyword.</li>"));
    assert!(deep.contains("<div><dt>Broad match</dt><dd>Your ad can appear for searches that relate to the keyword. These searches can be without the words of the keyword.</dd></div>"));
    assert!(page.contains("<p>The search terms, keywords, landing pages and ads with the most spend in the 30 days before the Deep check. We run a Deep check once a week.</p>"));
    assert!(!page[page.find(r#"id="deep-check""#).unwrap()..].contains("Not available"));

    let other = f.page(&format!("{PAGE}/campaigns/1?window=30")).await;
    assert!(other.contains(
        r#"action="/ad-accounts/4829153307/campaigns/1/export.pdf">
  <input type="hidden" name="window" value="30">"#
    ));
    assert!(other.contains("No Update was about this Campaign in the last month."));
    assert!(
        other.contains("Google Ads shows no Account change for this Campaign in these 8 weeks.")
    );
    assert!(other.contains("No open Recommendations for this Campaign."));
    assert!(other.contains("<p>The Deep check found no search terms, keywords, landing pages or ads with spend for this Campaign. We run a Deep check once a week.</p>"));
    assert!(!other.contains("Other Findings"));
    assert_eq!(
        f.request
            .get(&format!("{PAGE}/campaigns/99"))
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        404
    );
}

#[tokio::test]
#[serial]
async fn the_monitoring_pages_refuse_another_workspace_and_a_signed_out_user() {
    let f = normal_day().await;
    let gets = [
        PAGE.to_owned(),
        format!("{PAGE}?window=30"),
        format!("{PAGE}/campaigns/2"),
        format!("{PAGE}/settings"),
        format!("{PAGE}/remove"),
    ];
    for path in &gets {
        let other = f.request.get(path).add_cookie(f.b.clone()).await;
        assert_eq!(other.status_code(), 404, "{path}");
        assert!(!other.text().contains("Harbour Dental"));
        let signed_out = f.request.get(path).await;
        assert_eq!(signed_out.status_code(), 303, "{path}");
        assert_eq!(signed_out.header("location"), "/sign-in");
    }
    for (path, form) in [
        (
            format!("{PAGE}/updates/2026-10-12/not-useful"),
            json!({"window":"7"}),
        ),
        (
            format!("{PAGE}/recommendations"),
            json!({"finding_key":JOBS_KEY,"state":"done"}),
        ),
        (format!("{PAGE}/remove"), json!({})),
    ] {
        assert_eq!(f.post(&f.b, &path, form).await.status_code(), 404, "{path}");
    }
    assert_eq!(
        f.request
            .post(&format!("{PAGE}/settings"))
            .add_header("origin", ORIGIN)
            .add_cookie(f.b.clone())
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_text("name", "Changed")
                    .add_text("accent_colour", "#123456")
                    .add_text("weekly_email", "off")
            )
            .await
            .status_code(),
        404
    );
    let empty = f.request.get("/ad-accounts").add_cookie(f.b.clone()).await;
    assert_eq!(empty.status_code(), 200);
    assert!(!empty.text().contains("Harbour Dental"));
    assert!(empty.text().contains("You use 0 of 5 Ad Accounts."));
    let page = f.page(PAGE).await;
    assert!(page.contains(JOBS_ACTION));
    assert!(!page.contains("You marked this Update as not useful."));
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(
        store.branding(AD).await.unwrap().unwrap().name,
        "Harbour Dental"
    );
    assert!(store.monitoring_settings(AD).await.unwrap().weekly_email);
}

#[tokio::test]
#[serial]
async fn not_useful_and_done_change_what_the_pages_show() {
    let f = normal_day().await;
    let marked = f
        .post(
            &f.a,
            &format!("{PAGE}/updates/2026-10-12/not-useful"),
            json!({"window":"30"}),
        )
        .await;
    assert_eq!(marked.status_code(), 303);
    assert_eq!(
        marked.header("location"),
        "/ad-accounts/4829153307?window=30"
    );
    let page = f.page(PAGE).await;
    assert!(page.contains(r#"<p class="small muted">You marked this Update as not useful.</p>"#));
    assert!(!page.contains("/updates/2026-10-12/not-useful"));
    assert!(page.contains(SUMMARY.split(", and").next().unwrap()));
    for missing in ["2026-10-01", "yesterday"] {
        assert_eq!(
            f.post(
                &f.a,
                &format!("{PAGE}/updates/{missing}/not-useful"),
                json!({})
            )
            .await
            .status_code(),
            404
        );
    }

    let done = f
        .post(
            &f.a,
            &format!("{PAGE}/recommendations"),
            json!({"finding_key":JOBS_KEY,"state":"done","window":"7"}),
        )
        .await;
    assert_eq!(done.status_code(), 303);
    assert_eq!(
        done.header("location"),
        "/ad-accounts/4829153307?recommendations=open#recommendations"
    );
    let page = f.page(PAGE).await;
    assert!(!page.contains(JOBS_ACTION));
    assert!(page.contains(PAGE_ACTION));
    assert_eq!(
        f.post(
            &f.a,
            &format!("{PAGE}/recommendations"),
            json!({"finding_key":PAGE_KEY,"state":"open"}),
        )
        .await
        .status_code(),
        422
    );
    assert_eq!(
        f.post(
            &f.a,
            &format!("{PAGE}/recommendations"),
            json!({"finding_key":JOBS_KEY,"state":"done"}),
        )
        .await
        .status_code(),
        404
    );
    let not_useful = f
        .post(
            &f.a,
            &format!("{PAGE}/recommendations"),
            json!({"finding_key":PAGE_KEY,"state":"not_useful","campaign":"2"}),
        )
        .await;
    assert_eq!(
        not_useful.header("location"),
        "/ad-accounts/4829153307/campaigns/2?recommendations=open#recommendations"
    );
    let page = f.page(PAGE).await;
    assert!(!page.contains(PAGE_ACTION));
    assert!(
        page.contains("No open Recommendations. New ones appear here after the next Deep check.")
    );
    let campaign = f.page(&format!("{PAGE}/campaigns/2")).await;
    assert!(campaign.contains("No open Recommendations for this Campaign."));
    assert!(campaign.contains(&format!("<li>{PAGE_FINDING}</li>")));
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(
        store
            .monitoring_recommendations(AD)
            .await
            .unwrap()
            .into_iter()
            .map(|recommendation| recommendation.state)
            .collect::<Vec<_>>(),
        [RecommendationState::NotUseful, RecommendationState::Done]
    );
}

#[tokio::test]
#[serial]
async fn the_ad_accounts_list_shows_the_first_sentence_and_one_ad_account_goes_to_its_page() {
    let f = normal_day().await;
    let only = f.request.get("/ad-accounts").add_cookie(f.a.clone()).await;
    assert_eq!(only.status_code(), 303);
    assert_eq!(only.header("location"), PAGE);
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let connection = store.google_connections().await.unwrap().remove(0).id;
    let selection = store
        .create_selection(
            &connection,
            vec![AdAccountChoice {
                customer_id: "6170254418".into(),
                name: "Northgate Physio".into(),
                login_customer_id: None,
            }],
        )
        .await
        .unwrap();
    assert!(store
        .save_selected(&selection, &["6170254418".into()])
        .await
        .unwrap());
    let page = f.page("/ad-accounts").await;
    assert!(page.contains(r#"<h1 class="heading-xl">Ad Accounts</h1>"#));
    assert!(page.contains("<a href=\"/ad-accounts/4829153307\">Harbour Dental</a></h2>\n    \n    <p>Spend was steady this week at $4,120, and conversions rose from 88 to 104.</p>\n    <p class=\"small muted\">Update of Monday 12 October</p>"), "{page}");
    assert!(!page.contains("Most of the gain"));
    assert!(page.contains(r#"<a href="/ad-accounts/6170254418">Northgate Physio</a>"#));
    assert!(
        page.contains("No Update yet. The first Update appears after we have read the Ad Account.")
    );
    assert!(page.contains(&format!(
        r#"<form method="post" action="/connections/{connection}/ad-accounts"><button class="button" type="submit">Add an Ad Account</button></form>"#
    )));
    assert!(page.contains("You use 2 of 5 Ad Accounts."));
    assert!(page.contains(r#"<a href="/ad-accounts" aria-current="page">Ad Accounts</a>"#));
}

fn settings_form() -> axum_test::multipart::MultipartForm {
    axum_test::multipart::MultipartForm::new()
        .add_text("goal", "calls")
        .add_text("note", "Closed on Sundays.")
        .add_text("judge_by_conversions", "no")
        .add_text("name", "Harbour Dental Group")
        .add_text("accent_colour", "#0B6E6E")
        .add_text("weekly_email", "off")
}

#[tokio::test]
#[serial]
async fn settings_save_the_profile_the_conversion_choice_the_branding_and_the_weekly_email() {
    let f = normal_day().await;
    let path = format!("{PAGE}/settings");
    let page = f.page(&path).await;
    assert!(page.contains(r#"<h1 class="heading-xl">Settings for Harbour Dental</h1>"#));
    assert!(page.contains("Google Ads records conversions for this Ad Account, and the count is large enough to judge."));
    assert!(
        page.contains(r#"<input type="radio" name="judge_by_conversions" value="yes" checked>"#)
    );
    assert!(page.contains(r#"<input type="radio" name="weekly_email" value="on" checked>"#));
    assert!(page.contains(r#"<span aria-hidden="true">$</span>"#));
    assert!(page.contains(r#"name="name" maxlength="100" value="Harbour Dental""#));
    assert!(page.contains("We send the Weekly email once a week to a@example.com."));
    assert!(page.contains(r#"<a href="/ad-accounts/4829153307/remove">Remove this Ad Account</a>"#));

    let refused = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(settings_form().add_text("target_cost", "cheap"))
        .await;
    assert_eq!(refused.status_code(), 422);
    let refused = refused.text();
    assert!(refused.contains(r##"<li><a href="#target">Enter the target cost per conversion as a number, for example 45.00.</a></li>"##));
    assert!(refused.contains(r#"<p class="error-message" id="target-error"><span class="visually-hidden">Error: Enter the target cost per conversion as a number, for example 45.00.</span><span aria-hidden="true">Enter the target cost per conversion as a number, for example 45.00.</span></p>"#));
    assert!(refused.contains(
        r#"name="target_cost" value="cheap" aria-describedby="target-error" aria-invalid="true">"#
    ));
    assert!(refused.contains(r#"value="Harbour Dental Group""#));
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(store.profile(AD).await.unwrap().goal, "");
    assert_eq!(
        store.branding(AD).await.unwrap().unwrap().name,
        "Harbour Dental"
    );
    assert_eq!(
        store.monitoring_settings(AD).await.unwrap(),
        AccountSettings::default()
    );

    let saved = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(settings_form().add_text("target_cost", "45"))
        .await;
    assert_eq!(saved.status_code(), 303, "{}", saved.text());
    assert_eq!(
        saved.header("location"),
        "/ad-accounts/4829153307/settings?saved=1"
    );
    let page = f.page("/ad-accounts/4829153307/settings?saved=1").await;
    assert!(page.contains("We saved your changes."));
    assert!(page.contains(r#"<option value="calls" selected>Calls</option>"#));
    assert!(page.contains(r#"name="target_cost" value="45.00">"#));
    assert!(page.contains(">Closed on Sundays.</textarea>"));
    assert!(page.contains(r#"<input type="radio" name="judge_by_conversions" value="no" checked>"#));
    assert!(page.contains(r#"value="Harbour Dental Group""#));
    assert!(page.contains(r##"name="accent_colour" value="#0B6E6E""##));
    assert!(page.contains(r#"<input type="radio" name="weekly_email" value="off" checked>"#));
    assert_eq!(
        store.monitoring_settings(AD).await.unwrap(),
        AccountSettings {
            judge_by_conversions: Some(false),
            weekly_email: false,
            alerts: true,
        }
    );
    assert_eq!(store.profile(AD).await.unwrap().target_cost, Some(45.0));
    let account = f.page(PAGE).await;
    assert!(account.contains("We do not judge this Ad Account by conversions"));
    assert!(!account.contains("<dt>Conversions</dt>"));

    let again = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(
            axum_test::multipart::MultipartForm::new()
                .add_text("judge_by_conversions", "yes")
                .add_text("name", "Harbour Dental")
                .add_text("accent_colour", "#0B6E6E")
                .add_text("weekly_email", "on"),
        )
        .await;
    assert_eq!(again.status_code(), 303);
    assert_eq!(
        store.monitoring_settings(AD).await.unwrap(),
        AccountSettings::default()
    );
}

#[tokio::test]
#[serial]
async fn removal_states_what_is_deleted_and_then_deletes_it() {
    let f = normal_day().await;
    let page = f.page(&format!("{PAGE}/remove")).await;
    assert!(page.contains(r#"<h1 class="heading-xl">Remove Harbour Dental?</h1>"#));
    assert!(page.contains("We stop reading this Ad Account and delete its Account history, Updates and Recommendations."));
    assert!(page.contains(
        r#"<button class="button button--warning" type="submit">Remove Harbour Dental</button>"#
    ));
    assert!(page.contains(r#"<a href="/ad-accounts/4829153307/settings">Cancel</a>"#));
    let removed = f.post(&f.a, &format!("{PAGE}/remove"), json!({})).await;
    assert_eq!(removed.status_code(), 303);
    assert_eq!(removed.header("location"), "/ad-accounts");
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert!(store.ad_accounts().await.unwrap().is_empty());
    assert!(store.updates(AD, 0, i64::MAX).await.unwrap().is_empty());
    assert!(store
        .monitoring_days(AD, 0, i64::MAX)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .monitoring_recommendations(AD)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        f.request
            .get(PAGE)
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        404
    );
    let list = f.page("/ad-accounts").await;
    assert!(list.contains("No Ad Account yet."));
    assert!(!list.contains("Harbour Dental"));
}

async fn written(highlight: Value) -> (Fixture, Update) {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    {
        let mut calls = f.calls.lock().unwrap();
        for row in calls
            .google
            .monitoring_days
            .as_mut()
            .unwrap()
            .iter_mut()
            .rev()
            .take(7)
        {
            row["metrics"]["costMicros"] = json!("20000000");
        }
        calls.override_monitoring = Some(json!({
            "summary": "Spend rose to EUR 140 from EUR 70.00 in the last 7 days.",
            "highlight": highlight,
            "lines": [{"campaign_id": 701, "text": "Spend was EUR 140 against EUR 70.00."}],
        }));
    }
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let update = WorkspaceStore::new(&f.ctx.db, &signed.workspace)
        .updates(AD, day_number(today()), day_number(today()))
        .await
        .unwrap()
        .remove(0);
    (f, update)
}

#[tokio::test]
#[serial]
async fn a_highlight_is_kept_only_when_it_is_a_phrase_of_the_summary_with_a_detected_number() {
    let (f, update) = written(json!("Spend rose to EUR 140")).await;
    assert_eq!(update.highlight.as_deref(), Some("Spend rose to EUR 140"));
    assert!(!update.written_without_ai);
    assert!(
        f.calls.lock().unwrap().commentary[0]["text"]["format"]["schema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("highlight"))
    );
    let page = f.page(PAGE).await;
    assert!(page.contains(r#"<p class="lead"><mark>Spend rose to EUR 140</mark> from EUR 70.00 in the last 7 days.</p>"#), "{page}");
    drop(f);

    for refused in [
        json!("Spend rose to EUR 999"),
        json!("in the last 7 day"),
        json!("Spend rose"),
        json!("pend rose to EUR 140"),
        json!("Spend rose to EUR 14"),
        json!(" "),
        Value::Null,
    ] {
        let (f, update) = written(refused.clone()).await;
        assert_eq!(update.highlight, None, "{refused}");
        assert_eq!(
            update.summary,
            "Spend rose to EUR 140 from EUR 70.00 in the last 7 days."
        );
        assert!(!update.written_without_ai, "{refused}");
        let page = f.page(PAGE).await;
        assert!(!page.contains("<mark>"));
        assert!(page.contains(
            r#"<p class="lead">Spend rose to EUR 140 from EUR 70.00 in the last 7 days.</p>"#
        ));
    }
}

#[tokio::test]
#[serial]
async fn the_text_of_an_update_is_escaped_around_the_highlight() {
    let f = harbour(true).await;
    store_update(
        &f,
        day_number(today()),
        "Spend <b>rose</b> to $4,120 & more",
        Some("$4,120"),
        &[(EMERGENCY, "<i>Search</i>", "<u>58</u> conversions")],
    )
    .await;
    let page = f.page(PAGE).await;
    assert!(page.contains("to <mark>$4,120</mark> &amp; more"), "{page}");
    assert!(page.contains("Spend &lt;b&gt;rose&lt;"));
    assert!(!page.contains("<b>rose</b>"));
    assert!(!page.contains("<i>Search</i>"));
    assert!(!page.contains("<u>58</u>"));
}

#[tokio::test]
#[serial]
async fn settings_with_a_wrong_branding_name_save_nothing() {
    let f = normal_day().await;
    let path = format!("{PAGE}/settings");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(64, 32)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let refused = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(
            axum_test::multipart::MultipartForm::new()
                .add_text("goal", "calls")
                .add_text("target_cost", "45")
                .add_text("note", "Closed on Sundays.")
                .add_text("judge_by_conversions", "no")
                .add_text("name", "")
                .add_text("accent_colour", "#0B6E6E")
                .add_text("weekly_email", "off")
                .add_part(
                    "logo",
                    axum_test::multipart::Part::bytes(png.into_inner())
                        .file_name("logo.png")
                        .mime_type("image/png"),
                ),
        )
        .await;
    assert_eq!(refused.status_code(), 422);
    assert!(refused
        .text()
        .contains("Enter a name of 1 to 100 characters for the Branding."));
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(store.profile(AD).await.unwrap().goal, "");
    assert_eq!(store.profile(AD).await.unwrap().target_cost, None);
    assert!(store.logo(AD).await.unwrap().is_none());
    assert_eq!(
        store.branding(AD).await.unwrap().unwrap().accent_colour,
        "#0F766E"
    );
    assert_eq!(
        store.monitoring_settings(AD).await.unwrap(),
        AccountSettings::default()
    );
}

#[tokio::test]
#[serial]
async fn a_logo_uploaded_in_the_settings_is_stored_square_and_only_for_its_workspace() {
    let f = normal_day().await;
    let path = format!("{PAGE}/settings");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1024, 512)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let form = || {
        axum_test::multipart::MultipartForm::new()
            .add_text("name", "Harbour Dental Group")
            .add_text("accent_colour", "#238A72")
            .add_part(
                "logo",
                axum_test::multipart::Part::bytes(png.get_ref().clone())
                    .file_name("logo.png")
                    .mime_type("image/png"),
            )
    };
    let saved = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(form())
        .await;
    assert_eq!(saved.status_code(), 303, "{}", saved.text());
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let logo = store.logo(AD).await.unwrap().unwrap();
    assert!(logo.len() <= 500 * 1024);
    let image = image::load_from_memory(&logo).unwrap();
    assert_eq!((image.width(), image.height()), (512, 512));
    assert!(f
        .page(&path)
        .await
        .contains("A logo is saved now. A new file replaces it."));
    let other = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.b.clone())
        .multipart(form())
        .await;
    assert_eq!(other.status_code(), 404);
    let refused = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(
            axum_test::multipart::MultipartForm::new()
                .add_text("name", "Harbour Dental Group")
                .add_text("accent_colour", "#238A72")
                .add_part(
                    "logo",
                    axum_test::multipart::Part::bytes(b"not an image".to_vec())
                        .file_name("logo.png")
                        .mime_type("image/png"),
                ),
        )
        .await;
    assert_eq!(refused.status_code(), 422);
    assert!(refused
        .text()
        .contains("Use a PNG or JPEG logo up to 5 MB."));
}

#[tokio::test]
#[serial]
async fn the_user_can_add_five_ad_accounts_and_cannot_post_a_sixth() {
    let f = normal_day().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let connection = store.google_connections().await.unwrap().remove(0).id;
    let page = f.page(&format!("{PAGE}/settings")).await;
    assert!(page.contains(r#"<h2 class="heading-m" id="ad-accounts-h">Your Ad Accounts</h2>"#));
    assert!(page.contains("You use 1 of 5 Ad Accounts."));
    assert!(page.contains(&format!(
        r#"<form method="post" action="/connections/{connection}/ad-accounts"><button class="button" type="submit">Add an Ad Account</button></form>"#
    )));
    assert!(page.contains(r#"<a href="/settings">Google Connections and your Workspace</a>"#));
    let selection = store
        .create_selection(
            &connection,
            ["6170254418", "3309581126", "5550000001", "5550000002"]
                .map(|id| AdAccountChoice {
                    customer_id: id.into(),
                    name: id.into(),
                    login_customer_id: None,
                })
                .to_vec(),
        )
        .await
        .unwrap();
    assert!(store
        .save_selected(
            &selection,
            &[
                "6170254418".into(),
                "3309581126".into(),
                "5550000001".into(),
                "5550000002".into()
            ]
        )
        .await
        .unwrap());
    let full = f.page(&format!("{PAGE}/settings")).await;
    assert!(full.contains("You use 5 of 5 Ad Accounts."));
    assert!(full.contains("To add another Ad Account, remove one first."));
    assert!(!full.contains("Add an Ad Account</button>"));
    let more = store
        .create_selection(
            &connection,
            vec![AdAccountChoice {
                customer_id: "9042276630".into(),
                name: "Sixth".into(),
                login_customer_id: None,
            }],
        )
        .await
        .unwrap();
    let refused = f
        .request
        .post(&format!("/ad-accounts/choose/{more}"))
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .form(&[("ad_account", "9042276630")])
        .await;
    assert_eq!(refused.status_code(), 422);
    assert!(refused
        .text()
        .contains("You would have 6 Ad Accounts. Choose up to 5 in all."));
    let after = f.page(&format!("{PAGE}/settings")).await;
    assert!(after.contains("You use 5 of 5 Ad Accounts."));
    assert!(!after.contains("Sixth"));
}

const BUDGET_KEY: &str = "budget-limited:2:Search - Emergency (2)";

async fn named_campaigns() -> Fixture {
    let f = harbour(true).await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    for (id, name) in [(BRAND, "Search"), (IMPLANTS, "Tom & <b>Jerry</b>")] {
        f.ctx
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE campaigns SET name=? WHERE ad_account_customer_id=? AND campaign_id=?",
                vec![name.into(), AD.into(), id.into()],
            ))
            .await
            .unwrap();
    }
    let mut check = check(day_number(end()));
    check.data.currency = "USD".into();
    check.findings = vec![
        finding(
            EMERGENCY,
            BUDGET_KEY,
            "Search - Emergency (2) lost 20% of its search impressions to budget.",
            900.0,
        ),
        finding(
            EMERGENCY,
            "campaign-cost-high:2:Search - Emergency (2)",
            "Search - Emergency (2) spent $600 for 1 conversions.",
            600.0,
        ),
        finding(
            BRAND,
            "campaign-cost-high:1:Search (1)",
            "Search spent $500 for 1 conversions.",
            500.0,
        ),
        finding(
            IMPLANTS,
            "campaign-cost-high:3:Tom & <b>Jerry</b> (3)",
            "Tom & <b>Jerry</b> spent $400 for 1 conversions.",
            400.0,
        ),
    ];
    check.findings[0].rule.title = "Search - Emergency (2) is limited by its budget".into();
    let recommendation = |key: &str, action: &str, evidence: &str, campaign| Recommendation {
        finding_key: key.into(),
        action: action.into(),
        reason: "The Finding shows it.".into(),
        evidence: evidence.into(),
        measure: String::new(),
        intent: String::new(),
        subject: String::new(),
        confidence: Default::default(),
        campaign_id: Some(campaign),
        state: RecommendationState::Open,
    };
    store
        .refresh_deep_check(
            AD,
            &check,
            &[
                recommendation(
                    BUDGET_KEY,
                    "Increase the daily budget of Search - Emergency (2).",
                    "Search - Emergency (2) lost 20% of its search impressions to budget.",
                    EMERGENCY,
                ),
                recommendation(
                    "campaign-cost-high:1:Search (1)",
                    "Review the keywords of Search.",
                    "Search spent $500 for 1 conversions.",
                    BRAND,
                ),
                recommendation(
                    "campaign-cost-high:3:Tom & <b>Jerry</b> (3)",
                    "Review Tom & <b>Jerry</b>.",
                    "Tom & <b>Jerry</b> spent $400 for 1 conversions.",
                    IMPLANTS,
                ),
            ],
        )
        .await
        .unwrap();
    store_update(
        &f,
        day_number(today()),
        "Search - Emergency gained clicks and Search lost clicks. Research and search terms did not change.",
        None,
        &[(EMERGENCY, "Search - Emergency", "Search - Emergency gained clicks.")],
    )
    .await;
    f
}

#[tokio::test]
#[serial]
async fn a_campaign_name_in_a_recommendation_a_finding_or_an_update_is_a_link_to_its_campaign() {
    let f = named_campaigns().await;
    let page = f.page(&format!("{PAGE}?window=30")).await;
    const EMERGENCY_LINK: &str = r#"<a class="campaign-name" href="/ad-accounts/4829153307/campaigns/2?window=30">Search - Emergency</a>"#;
    const SEARCH_LINK: &str = r#"<a class="campaign-name" href="/ad-accounts/4829153307/campaigns/1?window=30">Search</a>"#;
    const JERRY_LINK: &str = r#"<a class="campaign-name" href="/ad-accounts/4829153307/campaigns/3?window=30">Tom &amp; &lt;b&gt;Jerry&lt;/b&gt;</a>"#;
    for text in [
        format!(r#"<div><dt>Finding</dt><dd>{EMERGENCY_LINK} is limited by its budget</dd></div>"#),
        format!(r#"<div class="parts__action"><dt>Suggested action</dt><dd>Increase the daily budget of {EMERGENCY_LINK}.</dd></div>"#),
        format!(r#"<div><dt>Evidence</dt><dd>{EMERGENCY_LINK} lost 20% of its search impressions to budget.</dd></div>"#),
        r#"<span class="visually-hidden">: Increase the daily budget of Search - Emergency.</span>"#.to_owned(),
        format!(r#"<dt>Suggested action</dt><dd>Review the keywords of {SEARCH_LINK}.</dd>"#),
        format!(r#"<dt>Evidence</dt><dd>{SEARCH_LINK} spent $500 for 1 conversions.</dd>"#),
        format!(r#"<dt>Suggested action</dt><dd>Review {JERRY_LINK}.</dd>"#),
        format!(r#"<dt>Evidence</dt><dd>{JERRY_LINK} spent $400 for 1 conversions.</dd>"#),
        format!(r#"<p class="lead">{EMERGENCY_LINK} gained clicks and {SEARCH_LINK} lost clicks. Research and search terms did not change.</p>"#),
        format!("<li>{EMERGENCY_LINK}: Search - Emergency gained clicks.</li>"),
        format!(r#"<input type="hidden" name="finding_key" value="{BUDGET_KEY}">"#),
    ] {
        assert!(page.contains(&text), "{text}\n{page}");
    }
    assert_eq!(count(&page, "Search - Emergency (2)"), 1);
    assert!(!page.contains("<b>Jerry</b>"));

    let week = f.page(PAGE).await;
    assert!(week.contains(
        r#"Increase the daily budget of <a class="campaign-name" href="/ad-accounts/4829153307/campaigns/2">Search - Emergency</a>."#
    ));
}

#[tokio::test]
#[serial]
async fn the_campaign_page_marks_the_name_of_its_campaign_without_a_link() {
    let f = named_campaigns().await;
    let page = f.page(&format!("{PAGE}/campaigns/2")).await;
    for text in [
        r#"<dt>Finding</dt><dd><b class="campaign-name">Search - Emergency</b> is limited by its budget</dd>"#,
        r#"<dt>Suggested action</dt><dd>Increase the daily budget of <b class="campaign-name">Search - Emergency</b>.</dd>"#,
        r#"<dt>Evidence</dt><dd><b class="campaign-name">Search - Emergency</b> lost 20% of its search impressions to budget.</dd>"#,
        r#"<li><b class="campaign-name">Search - Emergency</b> spent $600 for 1 conversions.</li>"#,
        r#"<p><b class="campaign-name">Search - Emergency</b> gained clicks.</p>"#,
    ] {
        assert!(page.contains(text), "{text}\n{page}");
    }
    assert!(!page.contains(r#"<a class="campaign-name""#));
    assert_eq!(count(&page, "Search - Emergency (2)"), 1);
}

#[tokio::test]
#[serial]
async fn a_stored_recommendation_is_read_without_the_campaign_id_for_the_email_and_the_export() {
    let f = named_campaigns().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let stored = store.monitoring_recommendations(AD).await.unwrap();
    let budget = stored
        .iter()
        .find(|recommendation| recommendation.finding_key == BUDGET_KEY)
        .unwrap();
    assert_eq!(
        budget.action,
        "Increase the daily budget of Search - Emergency."
    );
    assert_eq!(
        budget.evidence,
        "Search - Emergency lost 20% of its search impressions to budget."
    );
    let findings = store.monitoring_findings(AD).await.unwrap();
    assert_eq!(
        findings
            .iter()
            .map(|finding| (finding.rule.key.as_str(), finding.rule.detail.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                BUDGET_KEY,
                "Search - Emergency lost 20% of its search impressions to budget."
            ),
            (
                "campaign-cost-high:1:Search (1)",
                "Search spent $500 for 1 conversions."
            ),
            (
                "campaign-cost-high:2:Search - Emergency (2)",
                "Search - Emergency spent $600 for 1 conversions."
            ),
            (
                "campaign-cost-high:3:Tom & <b>Jerry</b> (3)",
                "Tom & <b>Jerry</b> spent $400 for 1 conversions."
            ),
        ]
    );
}

#[tokio::test]
#[serial]
async fn a_user_reads_the_assessment_and_how_to_check_a_recommendation() {
    use keysend::monitoring::assessment::{Assessment, Verdict};
    let f = normal_day().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut check = check(day_number(today()));
    check.findings = vec![standing_finding()];
    check.assessment = Some(Assessment {
        verdict: Verdict::Mixed,
        headline: "Search - Emergency has more enquiries.".into(),
        worked: vec![keysend::monitoring::claims::Point {
            observation: "Search - Emergency delivered 42 conversions.".into(),
            explanation: Some("The new opening hours may have helped.".into()),
        }],
        attention: vec!["Review wasted searches.".into()],
        next: vec!["Check the search terms.".into()],
        market: Some("Demand varies in autumn.".into()),
        written_without_ai: false,
    });
    check.research = Some(keysend::monitoring::business::Research {
        day: check.day,
        website: Some("https://example.com".into()),
        found: true,
        kind: keysend::monitoring::business::Kind::LocalService,
        summary: String::new(),
        offers: vec![],
        areas: vec![],
        facts: vec![keysend::monitoring::business::Fact {
            claim: "Demand varies in autumn.".into(),
            kind: keysend::monitoring::business::FactKind::Seasonality,
            source: "https://example.com/season".into(),
            verified: true,
        }],
    });
    let mut recommendation = standing_recommendation();
    recommendation.subject = "dental implants jobs".into();
    recommendation.confidence = keysend::monitoring::recommendation::Confidence::High;
    recommendation.measure = "Compare conversions after 30 days.".into();
    store
        .refresh_deep_check(AD, &check, &[recommendation])
        .await
        .unwrap();
    let page = f.page(PAGE).await;
    for words in [
        "Assessment",
        "Possible explanation",
        "The new opening hours may have helped.",
        "Sources: example.com",
        "Confidence: high",
        "About: dental implants jobs",
        "Result: Mixed",
        "What worked",
        "Needs attention",
        "Next 30 days",
        "Market context",
        "Review wasted searches.",
        "Check the search terms.",
        "Demand varies in autumn.",
        "How to check",
        "Compare conversions after 30 days.",
    ] {
        assert!(page.contains(words), "Missing {words}");
    }
    let period = format!(
        "The 30 days to {}",
        Language::En
            .text()
            .unwrap()
            .written
            .format
            .date_with_year(today().pred_opt().unwrap())
    );
    assert!(page.contains(&period), "{period}");
    assert!(page.contains(
        r#"href="/ad-accounts/4829153307/campaigns/2">Search - Emergency</a> has more enquiries."#
    ));
    assert!(page.find("Result: Mixed").unwrap() < page.find("An Update always compares").unwrap());
    check.assessment = None;
    let mut recommendation = standing_recommendation();
    recommendation.measure.clear();
    recommendation.finding_key = "legacy-recommendation".into();
    check.findings[0].rule.key = recommendation.finding_key.clone();
    store
        .refresh_deep_check(AD, &check, &[recommendation])
        .await
        .unwrap();
    let page = f.page(PAGE).await;
    assert!(!page.contains("Result: Mixed"));
    assert!(!page.contains("How to check"));
}

#[tokio::test]
#[serial]
async fn a_user_saves_and_reopens_the_ad_account_economics_and_sees_invalid_values_refused() {
    let f = normal_day().await;
    let path = format!("{PAGE}/settings");
    let form = || {
        settings_form()
            .add_text("margin", "35.5")
            .add_text("customer_value", "250.75")
            .add_text("close_rate", "20")
            .add_text("target_return", "4")
    };
    let saved = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(form())
        .await;
    assert_eq!(saved.status_code(), 303);
    let page = f.page(&path).await;
    for value in [
        r#"name="margin" value="35.50""#,
        r#"name="customer_value" value="250.75""#,
        r#"name="close_rate" value="20.00""#,
        r#"name="target_return" value="4.00""#,
    ] {
        assert!(page.contains(value), "Missing {value}");
    }
    for (field, value) in [
        ("margin", "0"),
        ("margin", "101"),
        ("margin", "-1"),
        ("customer_value", "-1"),
        ("customer_value", "0"),
        ("customer_value", "1000000000"),
        ("close_rate", "0"),
        ("close_rate", "101"),
        ("target_return", "-4"),
        ("target_return", "0"),
        ("target_return", "10000"),
        ("margin", "cheap"),
        ("customer_value", "NaN"),
        ("close_rate", "cheap"),
        ("target_return", "infinity"),
    ] {
        let refused = f
            .request
            .post(&path)
            .add_header("origin", ORIGIN)
            .add_cookie(f.a.clone())
            .multipart(settings_form().add_text(field, value))
            .await;
        assert_eq!(refused.status_code(), 422, "{field}: {value}");
        assert!(refused.text().contains(&format!("{field}-error")));
        assert!(f
            .page(&path)
            .await
            .contains(r#"name="margin" value="35.50""#));
    }
    let whole_share = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(
            settings_form()
                .add_text("margin", "100")
                .add_text("close_rate", "100"),
        )
        .await;
    assert_eq!(whole_share.status_code(), 303);
    let page = f.page(&path).await;
    assert!(page.contains(r#"name="margin" value="100.00""#));
    assert!(page.contains(r#"name="close_rate" value="100.00""#));
    let cleared = f
        .request
        .post(&path)
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .multipart(settings_form())
        .await;
    assert_eq!(cleared.status_code(), 303);
    let page = f.page(&path).await;
    for field in ["margin", "customer_value", "close_rate", "target_return"] {
        assert!(page.contains(&format!(r#"name="{field}" value="""#)));
    }
}

#[tokio::test]
#[serial]
async fn a_user_whose_google_connection_was_refused_is_asked_to_connect_google_ads_again() {
    let f = normal_day().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let page = f.page(PAGE).await;
    assert!(!page.contains("Google Ads is no longer connected"));

    store
        .history_failed(AD, "Connect Google Ads again.")
        .await
        .unwrap();
    let page = f.page(PAGE).await;
    assert!(page.contains("Spend was steady this week at $4,120"));
    assert!(page.contains(r#"<h2 class="notice__title">Google Ads is no longer connected</h2>"#));
    assert!(page.contains("Google no longer accepts KeySend's access, so we cannot read your Ad Accounts and your Updates have stopped."));
    assert!(page.contains(r#"<form method="post" action="/connections/google/start"><button class="button" type="submit">Connect Google Ads again</button></form>"#));

    let connection = store.google_connections().await.unwrap().remove(0).id;
    let selection = store
        .create_selection(
            &connection,
            vec![AdAccountChoice {
                customer_id: "6170254418".into(),
                name: "Northgate Physio".into(),
                login_customer_id: None,
            }],
        )
        .await
        .unwrap();
    assert!(store
        .save_selected(&selection, &["6170254418".into()])
        .await
        .unwrap());
    let list = f.page("/ad-accounts").await;
    assert!(list.contains(r#"<h2 class="notice__title">Google Ads is no longer connected</h2>"#));
    assert!(list.contains(r#"action="/connections/google/start""#));

    store
        .history_failed(AD, "Google Ads data is unavailable. Try again later.")
        .await
        .unwrap();
    assert!(!f
        .page(PAGE)
        .await
        .contains("Google Ads is no longer connected"));
    assert!(!f
        .page("/ad-accounts")
        .await
        .contains("Google Ads is no longer connected"));
}

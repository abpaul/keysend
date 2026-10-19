use std::time::Duration;

use keysend::{
    monitoring::{audit::Counts, example},
    workspace::{
        history::day_number,
        monitoring::{
            ActionDay, BreakdownDays, DeepBreakdowns, DemographicTotal, HourTotal, KeywordQuality,
            LocationTotal, NetworkDay,
        },
        WorkspaceStore,
    },
};
use sea_orm::ConnectionTrait;
use serial_test::serial;

use super::*;

const AD: &str = example::AD;

async fn harbour_dental() -> Fixture {
    harbour_dental_with_sample(false).await
}

async fn harbour_dental_with_sample(sample: bool) -> Fixture {
    let f = Fixture::monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.ctx
        .db
        .execute_unprepared(&format!(
            "UPDATE ad_accounts SET name='{}' WHERE customer_id='{AD}'",
            example::NAME
        ))
        .await
        .unwrap();
    store
        .save_ad_account_metadata(AD, example::CURRENCY, "UTC")
        .await
        .unwrap();
    let from = example::END - chrono::Days::new(example::HISTORY_DAYS - 1);
    store
        .save_history(
            AD,
            from,
            example::END,
            &example::campaigns(),
            &example::history(),
            &example::changes(),
        )
        .await
        .unwrap();

    let last = day_number(example::END);
    let days = example::days();
    for offset in 0..example::UPDATES {
        store
            .save_update(AD, &example::update(&days, offset))
            .await
            .unwrap();
    }

    let first = last - example::BREAKDOWN_DAYS + 1;
    let mut networks = Vec::new();
    let mut actions = Vec::new();
    for offset in 0..example::BREAKDOWN_DAYS {
        for demo in &example::CAMPAIGNS {
            let parts: &[(&str, f64)] = if demo.search_share.is_some() {
                &[("SEARCH", 0.86), ("SEARCH_PARTNERS", 0.14)]
            } else {
                &[("CONTENT", 1.0)]
            };
            for (network, share) in parts {
                let cost = demo.spend * share;
                networks.push(NetworkDay {
                    day: last - offset,
                    campaign_id: demo.id,
                    network: (*network).into(),
                    counts: Counts {
                        impressions: (cost / demo.click_cost * 14.0).round(),
                        clicks: (cost / demo.click_cost).round(),
                        cost,
                        conversions: cost / demo.conversion_cost,
                        value: 0.0,
                    },
                });
            }
            for (action, share) in [
                ("Booking form", 0.52),
                ("Phone call", 0.37),
                ("Directions", 0.11),
            ] {
                actions.push(ActionDay {
                    day: last - offset,
                    campaign_id: demo.id,
                    action: action.into(),
                    conversions: demo.spend / demo.conversion_cost * share,
                    value: 0.0,
                });
            }
        }
    }
    for breakdown in [
        BreakdownDays::SearchShare(example::search_share_days()),
        BreakdownDays::Networks(networks),
        BreakdownDays::ConversionActions(actions),
    ] {
        store
            .save_breakdown(AD, first, last, breakdown)
            .await
            .unwrap();
    }

    let totals = |clicks: f64, cost: f64, conversions: f64| Counts {
        impressions: clicks * 12.0,
        clicks,
        cost,
        conversions,
        value: 0.0,
    };
    let mut hours = Vec::new();
    for weekday in 0..7_i64 {
        for hour in 0..24_i64 {
            let open = (7..20).contains(&hour);
            let peak = [8, 9, 12, 13, 17, 18].contains(&hour);
            let clicks = match (open, peak, weekday < 5) {
                (true, true, true) => 44.0 + (hour % 4) as f64 * 5.0,
                (true, false, true) => 26.0 + (hour % 3) as f64 * 4.0,
                (true, _, false) => 14.0 + (hour % 4) as f64 * 2.0,
                _ => 2.0 + (hour % 3) as f64,
            };
            let rate = if open { 16.0 } else { 40.0 };
            hours.push(HourTotal {
                weekday,
                hour,
                counts: totals(clicks, clicks * 2.2, clicks / rate),
            });
        }
    }
    let demographic = |rows: &[(&str, f64, f64)]| {
        rows.iter()
            .map(|(value, clicks, rate)| DemographicTotal {
                value: (*value).into(),
                counts: totals(*clicks, clicks * 2.2, clicks / rate),
            })
            .collect::<Vec<_>>()
    };
    let keyword = |criterion_id: i64,
                   keyword: &str,
                   match_type: &str,
                   cost: f64,
                   score: i64,
                   levels: [&str; 3]| KeywordQuality {
        campaign_id: example::EMERGENCY,
        ad_group_id: 55,
        criterion_id,
        keyword: keyword.into(),
        match_type: match_type.into(),
        cost,
        score: Some(score),
        expected_ctr: Some(levels[0].into()),
        ad_relevance: Some(levels[1].into()),
        landing_page: Some(levels[2].into()),
    };
    let breakdowns = |keywords: Vec<KeywordQuality>| DeepBreakdowns {
        hours: Ok(hours.clone()),
        locations: Ok([
            ("Harbour City", 2140.0, 14.0),
            ("Northside", 1310.0, 17.0),
            ("Eastport", 980.0, 19.0),
            ("Mill Valley", 640.0, 31.0),
            ("Southbank", 420.0, 22.0),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (name, clicks, rate))| LocationTotal {
            id: 1_000_001 + index as i64,
            name: format!("{name}, New South Wales"),
            counts: totals(clicks, clicks * 2.2, clicks / rate),
        })
        .collect()),
        keywords: Ok(keywords),
        ages: Ok(demographic(&[
            ("AGE_RANGE_18_24", 410.0, 30.0),
            ("AGE_RANGE_25_34", 1680.0, 17.0),
            ("AGE_RANGE_35_44", 1920.0, 15.0),
            ("AGE_RANGE_45_54", 1240.0, 16.0),
            ("AGE_RANGE_55_64", 760.0, 21.0),
            ("AGE_RANGE_UNDETERMINED", 380.0, 26.0),
        ])),
        genders: Ok(demographic(&[
            ("FEMALE", 3420.0, 16.0),
            ("MALE", 2610.0, 19.0),
            ("UNDETERMINED", 360.0, 27.0),
        ])),
        delay: Some(Ok(vec![
            ("LESS_THAN_ONE_DAY".into(), 310.0),
            ("ONE_TO_TWO_DAYS".into(), 62.0),
            ("THREE_TO_FOUR_DAYS".into(), 35.0),
        ])),
    };
    let (below, average, above) = ("BELOW_AVERAGE", "AVERAGE", "ABOVE_AVERAGE");
    let (mut check, mut recommendations) = example::deep_check(last - 7);
    if sample {
        prepare_sample_deep_check(&mut check, &mut recommendations);
    }
    store
        .refresh_deep_check(AD, &check, &recommendations)
        .await
        .unwrap();
    store
        .save_deep_breakdowns(
            AD,
            last - 7,
            &breakdowns(vec![
                keyword(
                    901,
                    "emergency dentist",
                    "EXACT",
                    4410.0,
                    7,
                    [average, average, above],
                ),
                keyword(
                    904,
                    "24 hour dentist",
                    "PHRASE",
                    870.0,
                    8,
                    [average, above, above],
                ),
            ]),
        )
        .await
        .unwrap();
    let (mut check, mut recommendations) = example::deep_check(last);
    if sample {
        prepare_sample_deep_check(&mut check, &mut recommendations);
    }
    store
        .save_deep_breakdowns(
            AD,
            last,
            &breakdowns(vec![
                keyword(
                    901,
                    "emergency dentist",
                    "EXACT",
                    4520.0,
                    5,
                    [below, average, above],
                ),
                keyword(
                    902,
                    "dentist open now",
                    "PHRASE",
                    2260.0,
                    3,
                    [below, below, average],
                ),
                keyword(
                    904,
                    "24 hour dentist",
                    "PHRASE",
                    890.0,
                    8,
                    [average, above, above],
                ),
            ]),
        )
        .await
        .unwrap();
    store
        .refresh_deep_check(AD, &check, &recommendations)
        .await
        .unwrap();
    f
}

fn prepare_sample_deep_check(
    check: &mut keysend::monitoring::deep_check::DeepCheck,
    recommendations: &mut [keysend::workspace::monitoring::Recommendation],
) {
    use keysend::monitoring::{
        assessment, claims::Point, deep_check, metrics, recommendation::Confidence,
    };
    let days = example::days();
    let end = check.day - 1;
    let totals = metrics::sum(&days, end - 29, end, None);
    check.data.reporting = Counts {
        impressions: totals.impressions,
        clicks: totals.clicks,
        cost: totals.spend,
        conversions: totals.conversions.unwrap(),
        value: 0.0,
    };
    deep_check::compare(&mut check.data, &mut check.by_campaign, &days, end);
    let judged = assessment::judge(&check.data, true, &check.findings);
    let mut assessment = assessment::fallback(&judged);
    let emergency = &check.by_campaign[&example::EMERGENCY].campaigns[0];
    let emergency_cost = metrics::english_money(
        emergency.counts.cost / emergency.counts.conversions,
        example::CURRENCY,
    );
    assessment.worked = vec![Point {
        observation: format!("Search - Emergency brought {} conversions in the 30 days, at {emergency_cost} each.", metrics::english_count(emergency.counts.conversions)),
        explanation: Some("The move to maximize conversions may have helped reach people ready to book. The Account change alone does not prove this.".into()),
    }];
    assessment.attention = vec![Point {
        observation: check.findings[1].rule.detail.clone(),
        explanation: Some(
            "The wording may attract people looking for work rather than dental treatment.".into(),
        ),
    }];
    assessment.next = vec![
        format!("Exclude dental implants jobs first, then test a higher daily budget for Search - Emergency and compare its cost per conversion with {emergency_cost}."),
        "Keep watching Display - Remarketing. Its small conversion count can be chance, so avoid drawing a conclusion from one week.".into(),
    ];
    assessment.written_without_ai = false;
    check.assessment = Some(assessment);
    for (index, recommendation) in recommendations.iter_mut().enumerate() {
        let finding = &check.findings[index];
        recommendation.subject = finding.rule.subjects[0].clone();
        recommendation.confidence = if index == 1 {
            Confidence::High
        } else {
            Confidence::Medium
        };
        recommendation.measure = match index {
            0 => format!("Compare cost per conversion with {emergency_cost} and check the share of impressions lost to budget after the test."),
            1 => format!("Check that spend on dental implants jobs falls below {} while relevant searches keep converting.", metrics::english_money(finding.rule.spend, example::CURRENCY)),
            _ => "Compare the home page and emergency page conversion rates after sending the ads to the emergency page.".into(),
        };
    }
}

#[tokio::test]
#[serial]
#[ignore = "writes the committed public sample PDF when KEYSEND_WRITE_SAMPLE=1"]
async fn write_the_public_sample_report() {
    use keysend::{controllers::export, language::Language, settings::Settings};
    assert_eq!(std::env::var("KEYSEND_WRITE_SAMPLE").as_deref(), Ok("1"));
    let f = harbour_dental_with_sample(true).await;
    let response = f
        .request
        .get(&format!("/ad-accounts/{AD}/export.pdf"))
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(response.status_code(), 200);
    assert!(response.as_bytes().starts_with(b"%PDF"));
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let settings = Settings::load(&f.ctx).unwrap();
    let (mut document, text) = export::ad_account_export(
        &store,
        &settings,
        AD,
        &export::Params::default(),
        Language::En,
    )
    .await
    .unwrap()
    .unwrap_or_else(|_| panic!("the sample Export was refused"));
    document.sample = true;
    let pdf = keysend::monitoring::export::pdf(&document, text).unwrap();
    assert!(pdf.len() > 10_000);
    std::fs::write("assets/static/sample-report.pdf", pdf).unwrap();
}

#[tokio::test]
#[serial]
async fn a_visitor_can_open_the_sample_export_without_signing_in() {
    let f = Fixture::monitoring().await;
    let response = f.request.get("/sample-report.pdf").await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.header("content-type"), "application/pdf");
    assert_eq!(
        response.header("content-disposition"),
        "inline; filename=\"keysend-sample-report.pdf\""
    );
    assert_eq!(
        response.header("cache-control"),
        "public, max-age=0, must-revalidate"
    );
    assert_eq!(response.header("x-content-type-options"), "nosniff");
    assert_eq!(response.header("x-frame-options"), "DENY");
    assert_eq!(response.header("referrer-policy"), "same-origin");
    assert!(response
        .header("content-security-policy")
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));
    assert!(response.as_bytes().starts_with(b"%PDF"));
    assert!(response.as_bytes().len() > 10_000);
    assert_eq!(
        response.as_bytes(),
        include_bytes!("../../../assets/static/sample-report.pdf").as_slice()
    );
    let private = f
        .request
        .get("/sample-report.pdf")
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(private.header("cache-control"), "no-store");
    let query = f.request.get("/sample-report.pdf?preview=1").await;
    assert_eq!(query.header("cache-control"), "no-store");
}

fn part<'a>(page: &'a str, start: &str, end: &str) -> &'a str {
    let from = page
        .find(start)
        .unwrap_or_else(|| panic!("{start} is not in the page"));
    let length = page[from..]
        .find(end)
        .unwrap_or_else(|| panic!("{end} is not after {start}"));
    &page[from..from + length]
}

fn parts<'a>(page: &'a str, start: &str, end: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = page;
    while let Some(from) = rest.find(start) {
        let length = rest[from..].find(end).unwrap();
        found.push(&rest[from..from + length]);
        rest = &rest[from + length..];
    }
    found
}

fn words(html: &str) -> String {
    let mut text = String::new();
    let mut tag = false;
    for character in html.chars() {
        match character {
            '<' => {
                tag = true;
                text.push(' ');
            }
            '>' => tag = false,
            _ if !tag => text.push(character),
            _ => {}
        }
    }
    text.replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[tokio::test]
#[serial]
async fn the_home_page_shows_the_parts_that_the_pages_of_harbour_dental_show() {
    let f = harbour_dental().await;
    let home = f.request.get("/").await.text();
    let account = f.page(&format!("/ad-accounts/{AD}")).await;
    let campaign = f
        .page(&format!(
            "/ad-accounts/{AD}/campaigns/{}",
            example::EMERGENCY
        ))
        .await;

    let update = |page: &str| words(part(page, "<article class=\"card update\"", "</ul>"));
    assert!(update(&home).starts_with(
        "Today's Update Monday 12 October Spend was steady this week at $3,738, and conversions rose from 95 to 102 . Most of the gain came from Search - Emergency after"
    ));
    assert_eq!(update(&home), update(&account));

    let metrics = |page: &str| {
        parts(
            page,
            "<div class=\"metric\">",
            "<dd class=\"metric__link\">",
        )
        .into_iter()
        .map(|metric| words(part(metric, "<dt>", "</svg>")))
        .collect::<Vec<_>>()
    };
    assert_eq!(
        metrics(&account)[0],
        "Spend $3,738 down 1.7% from $3,801 Spend per day from 5 Oct to 11 Oct: $366 at the lowest, $639 at the highest."
    );
    assert_eq!(metrics(&account).len(), 4);
    let home_metrics = parts(&home, "<div class=\"metric\">", "</div>")
        .into_iter()
        .map(|metric| words(part(metric, "<dt>", "</svg>")))
        .collect::<Vec<_>>();
    assert_eq!(home_metrics, metrics(&account));
    let window = |page: &str| words(part(page, "<p class=\"muted\">", "</details>"));
    assert!(window(&home).starts_with("5 to 11 October, compared with the 7 days before. What"));
    assert_eq!(window(&home), window(&account));

    let trend =
        |page: &str| part(page, "<figure class=\"chart chart--trend\"", "</figure>").to_owned();
    assert!(trend(&home).contains(r#"aria-label="Sun 11 Oct: $366. Usual: $371 to $384."#));
    assert_eq!(trend(&home), trend(&account));

    for (id, first) in [
        (
            "trend",
            "Spend per day, against its usual range Spend had no unusual day in the last 7 days.",
        ),
        (
            "campaign-shares",
            "Spend and conversions by Campaign, last 7 days Search - Implants takes 23% of spend and brings 11% of conversions.",
        ),
        (
            "search-share",
            "Search impression share, last 7 days",
        ),
    ] {
        let chart = |page: &str| {
            let title = page.find(&format!(" id=\"{id}\">")).unwrap();
            words(part(
                &page[title..],
                ">",
                &format!("<span id=\"{id}-end\""),
            ))
        };
        let shown = chart(&home);
        assert!(shown.starts_with(first), "{id}: {shown}");
        assert_eq!(shown, chart(&account), "{id}");
    }

    let recommendations = |page: &str| {
        parts(page, "<dl class=\"parts\">", "</dl>")
            .into_iter()
            .map(words)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        recommendations(&home)[0],
        "Finding Search - Emergency is limited by its budget Impact Google estimates that the budget limits this Campaign's eligible Search impressions. Review its cost per conversion before testing a higher daily budget. Suggested action Increase the daily budget of Search - Emergency . Evidence Search - Emergency lost an estimated 31% of its eligible Search impressions to budget. Its cost per conversion is $42.62, with 214 conversions."
    );
    assert_eq!(recommendations(&home).len(), 3);
    assert_eq!(recommendations(&home), recommendations(&account));

    let terms = |page: &str| {
        words(part(
            page,
            "<section class=\"card deep\" aria-labelledby=\"search-terms\">",
            "</section>",
        ))
    };
    assert!(terms(&home).starts_with(
        "Search terms with the most spend 1 of the 5 search terms with the most spend has spend and no conversions: $296."
    ));
    assert_eq!(terms(&home), terms(&campaign));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
#[ignore = "serves pages for the screenshots of the public pages"]
async fn serve_harbour_dental_to_a_browser() {
    let directory = std::path::PathBuf::from(std::env::var("KEYSEND_BROWSER_CHECK_DIR").unwrap());
    let f = harbour_dental().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        directory.join("address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    std::fs::write(directory.join("cookie"), f.a.value()).unwrap();
    let router = f.router.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    while !directory.join("stop").exists() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[test]
#[ignore = "writes the localized PDF used by the home page preview"]
fn the_home_page_can_show_a_romanian_export() {
    use keysend::{
        language::Language,
        monitoring::{
            export::{
                self, CampaignRow, DeepCharts, Export, RecommendationText, Subject, UpdateLine,
                UpdateText,
            },
            format,
            metrics::{self, Window},
            view,
        },
    };
    let directory = std::path::PathBuf::from(std::env::var("KEYSEND_EXPORT_PREVIEW_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    let text = Language::Ro.text().unwrap();
    let days = example::days();
    let end = day_number(example::END);
    let window = Window::SevenDays;
    let campaigns = example::localized_campaigns(text);
    let update = example::localized_update(text, &days).unwrap();
    let recommendations = example::localized_recommendations(text).unwrap();
    let document = Export {
        sample: false,
        name: example::NAME.into(),
        accent: "#5126A6".into(),
        logo: None,
        window_days: window.days(),
        campaign_names: campaigns
            .iter()
            .map(|campaign| campaign.name.clone())
            .collect(),
        range: format::date_range(
            text,
            example::END - chrono::Days::new(6),
            example::END,
            true,
        ),
        compared: format::date_range(
            text,
            example::END - chrono::Days::new(13),
            example::END - chrono::Days::new(7),
            true,
        ),
        prepared: text
            .written
            .format
            .date_with_year(example::END.succ_opt().unwrap()),
        notice: None,
        cells: format::cells(
            text,
            &metrics::window_metrics(&days, end, window, None, true),
            example::CURRENCY,
        )
        .unwrap(),
        sparklines: Vec::new(),
        charts: Vec::new(),
        recommendations: recommendations
            .into_iter()
            .map(|found| RecommendationText {
                subject: String::new(),
                confidence: Default::default(),
                measure: String::new(),
                finding: found.finding.map(|finding| finding.title),
                action: found.recommendation.action,
                reason: found.recommendation.reason,
                evidence: found.recommendation.evidence,
            })
            .collect(),
        subject: Subject::AdAccount {
            assessment: None,
            update: Some(UpdateText {
                date: view::long_date(text, update.day).unwrap(),
                summary: update.summary,
                cause: None,
                lines: update
                    .lines
                    .into_iter()
                    .map(|line| UpdateLine {
                        campaign: campaigns
                            .iter()
                            .find(|campaign| campaign.id == line.campaign_id)
                            .unwrap()
                            .name
                            .clone(),
                        text: line.text,
                        cause: None,
                    })
                    .collect(),
                more_campaigns: 0,
            }),
            earlier: Vec::new(),
            campaign_headers: view::all_campaign_headers(text, true),
            campaigns: campaigns
                .iter()
                .map(|campaign| CampaignRow {
                    name: campaign.name.clone(),
                    sparkline: None,
                    cells: view::all_campaign_cells(
                        text,
                        &metrics::window_metrics(&days, end, window, Some(campaign.id), true),
                        example::CURRENCY,
                        true,
                    )
                    .unwrap(),
                })
                .collect(),
            deep_charts: DeepCharts {
                date: None,
                charts: Vec::new(),
            },
        },
    };
    let source = export::source(&document, text).unwrap();
    assert!(source.contains("Recomandări"));
    let pdf = export::pdf(&document, text).unwrap();
    std::fs::write(directory.join("home-ro.pdf"), pdf).unwrap();
}

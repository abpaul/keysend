use std::collections::BTreeSet;

use keysend::monitoring::campaign_settings::AllFollowAccount;
use keysend::{
    language::marker,
    monitoring::{
        alerts::{self, Alert, EventKey},
        audit::{Audit, AuditRow, Counts},
        email,
        export::{self, CampaignDeepCheck, DeepCharts, Export, Subject},
        format,
        metrics::Metrics,
    },
};

use super::{breakdowns::harbour_deep, *};

const PAGE: &str = "/ad-accounts/4829153307";
const EMERGENCY: i64 = 2;
const ORIGIN: &str = "https://keysend.example";

fn decode(text: &str) -> String {
    [
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&#x27;", "'"),
        ("&nbsp;", " "),
        ("&amp;", "&"),
    ]
    .iter()
    .fold(text.to_owned(), |text, (entity, character)| {
        text.replace(entity, character)
    })
}

fn visible(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        out.push(' ');
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else {
            return out;
        };
        let tag = &tail[..end];
        let described = tag.contains("name=\"description\"") || tag.contains("property=\"og:");
        let mut names = vec![" alt=\"", " title=\"", " aria-label=\"", " placeholder=\""];
        if described {
            names.push(" content=\"");
        }
        for name in names {
            if let Some(at) = tag.find(name) {
                out.push_str(tag[at + name.len()..].split('"').next().unwrap_or_default());
                out.push(' ');
            }
        }
        rest = &tail[end + 1..];
        for block in ["script", "style"] {
            if tag.starts_with(&format!("<{block}")) {
                let close = format!("</{block}>");
                rest = rest.find(&close).map_or("", |at| &rest[at + close.len()..]);
            }
        }
    }
    out.push_str(rest);
    out
}

fn without_chips(html: &str) -> String {
    let open = r#"<b style="padding:1px 5px; border-radius:4px; background:#EEE9F6; color:#3F1D85; font-weight:700;">"#;
    html.replace(open, "").replace("</b>", "")
}

fn without_markers(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('‹') {
        out.push_str(&rest[..start]);
        out.push(' ');
        rest = rest[start..]
            .find('›')
            .map_or("", |end| &rest[start + end + '›'.len_utf8()..]);
    }
    out.push_str(rest);
    out
}

fn json_strings(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if let Ok(inner) = serde_json::from_str::<Value>(text) {
                if inner.is_object() || inner.is_array() {
                    json_strings(&inner, found);
                    return;
                }
            }
            found.push(text.clone());
        }
        Value::Array(items) => items.iter().for_each(|item| json_strings(item, found)),
        Value::Object(map) => map.values().for_each(|item| json_strings(item, found)),
        _ => {}
    }
}

async fn stored(f: &Fixture, own: &[&str]) -> Vec<String> {
    let tables = f
        .ctx
        .db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE type='table'",
        ))
        .await
        .unwrap();
    let mut found = own
        .iter()
        .map(|text| (*text).to_owned())
        .collect::<Vec<_>>();
    for table in tables {
        let name: String = table.try_get_by_index(0).unwrap();
        let rows = f
            .ctx
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("SELECT * FROM \"{name}\""),
            ))
            .await
            .unwrap();
        for row in rows {
            for index in 0..row.column_names().len() {
                if let Ok(text) = row.try_get_by_index::<String>(index) {
                    json_strings(&Value::String(text), &mut found);
                }
            }
        }
    }
    found.retain(|text| text.chars().count() >= 8 || text.contains(' '));
    found.sort_by_key(|text| std::cmp::Reverse(text.len()));
    found.dedup();
    found
}

fn leaks(shown: &str, stored: &[String], english: &BTreeSet<String>) -> BTreeSet<String> {
    let mut text = without_markers(&decode(shown).replace('\u{200b}', ""))
        .split_whitespace()
        .filter(|word| !word.starts_with("http"))
        .collect::<Vec<_>>()
        .join(" ");
    for stored in stored {
        text = text.replace(stored.as_str(), " ");
    }
    let mut leaked = text
        .split(|character: char| !character.is_alphabetic())
        .map(str::to_lowercase)
        .filter(|word| english.contains(word))
        .collect::<BTreeSet<_>>();
    if text.replace("US$", "").contains('$') {
        leaked.insert("$ before an amount".into());
    }
    leaked
}

async fn use_marker_language(f: &Fixture) {
    f.ctx
        .db
        .execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!("UPDATE users SET language='{}'", marker::LANGUAGE.code()),
        ))
        .await
        .unwrap();
}

#[tokio::test]
#[serial]
async fn in_a_test_language_no_page_export_or_email_keeps_an_english_fixed_word() {
    let f = harbour_deep(true).await;
    use_marker_language(&f).await;
    let english = marker::english_words()
        .into_iter()
        .filter(|word| word != "keysend" && word != "english")
        .collect::<BTreeSet<_>>();
    let text = marker::text();
    assert!(english.len() > 500, "{}", english.len());
    assert_eq!(
        leaks("Spend per day, $12.50 on Monday ‹key›", &[], &english)
            .into_iter()
            .collect::<Vec<_>>(),
        ["$ before an amount", "day", "monday", "per", "spend"]
    );

    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut check = store.latest_deep_check(AD).await.unwrap().unwrap();
    check.assessment = Some(keysend::monitoring::assessment::Assessment {
        verdict: keysend::monitoring::assessment::Verdict::Mixed,
        headline: "The results are mixed.".into(),
        worked: vec![keysend::monitoring::claims::Point {
            observation: "Search - Emergency delivered 42 conversions.".into(),
            explanation: Some("Longer opening hours may have helped.".into()),
        }],
        attention: vec!["Review wasted searches.".into()],
        next: vec!["Check the search terms.".into()],
        market: Some("Demand varies in autumn.".into()),
        written_without_ai: false,
    });
    let mut recommendations = store
        .recommendations_with_findings(AD)
        .await
        .unwrap()
        .into_iter()
        .map(|found| found.recommendation)
        .collect::<Vec<_>>();
    for recommendation in &mut recommendations {
        recommendation.measure = "Compare conversions after 30 days.".into();
        recommendation.subject = "dental implants jobs".into();
        recommendation.confidence = keysend::monitoring::recommendation::Confidence::High;
    }
    f.ctx.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
        "DELETE FROM monitoring_recommendations WHERE workspace_id=? AND ad_account_customer_id=?",
        vec![signed.workspace.id().into(), AD.into()])).await.unwrap();
    store
        .refresh_deep_check(AD, &check, &recommendations)
        .await
        .unwrap();
    let mut shown: Vec<(String, String)> = Vec::new();
    for path in [
        PAGE.to_owned(),
        format!("{PAGE}?window=30"),
        format!("{PAGE}?metric=conversions"),
        format!("{PAGE}/campaigns/{EMERGENCY}"),
        format!("{PAGE}/campaigns/{EMERGENCY}?window=30"),
        format!("{PAGE}/breakdowns"),
        format!("{PAGE}/breakdowns?window=30"),
        format!("{PAGE}/settings"),
        format!("{PAGE}/campaigns/{EMERGENCY}/settings"),
        "/settings".to_owned(),
        "/subscription".to_owned(),
        format!("/remove/workspaces/{}", signed.workspace.id()),
    ] {
        let page = f.page(&path).await;
        assert!(page.contains("‹"), "{path} is not in the test language");
        shown.push((path, visible(&page)));
    }

    let last = day_number(end());
    let days = store
        .monitoring_days(
            AD,
            keysend::controllers::monitoring::first_read_day(last),
            last,
        )
        .await
        .unwrap();
    let campaigns = store.campaigns(AD).await.unwrap();
    let first_day = day_number(
        store
            .history_state(AD)
            .await
            .unwrap()
            .unwrap()
            .first_day
            .unwrap(),
    );
    let collected = keysend::controllers::monitoring::Collected {
        text,
        id: AD,
        days: &days,
        campaigns: &campaigns,
        first_day,
        end: last,
        currency: "USD",
        judged: true,
        window: Window::SevenDays,
        translations: &keysend::translation::Translations::empty(marker::LANGUAGE),
    };
    let date = |day| keysend::workspace::history::date_of(day).unwrap();
    let total = store
        .window_metrics(AD, last, Window::SevenDays, None)
        .await
        .unwrap();
    let changes = keysend::controllers::monitoring::export_changes(&store, &collected, None)
        .await
        .unwrap();
    let rules = store
        .campaign_rules(
            AD,
            keysend::monitoring::tracking::ConversionTracking::Working,
            &Default::default(),
        )
        .await
        .unwrap();
    let (charts, deep_charts) =
        keysend::controllers::monitoring::export_charts(&store, &collected, &changes, &rules)
            .await
            .unwrap();
    let export = |subject: Subject, charts, recommendations| Export {
        sample: false,
        name: "Harbour Dental".into(),
        accent: "#5126A6".into(),
        logo: None,
        window_days: 7,
        range: format::date_range(text, date(last - 6), date(last), true),
        compared: format::date_range(text, date(last - 13), date(last - 7), true),
        campaign_names: campaigns
            .iter()
            .map(|campaign| campaign.name.clone())
            .collect(),
        prepared: format::long_date(text, today()),
        notice: keysend::monitoring::view::tracking_notice(
            text,
            keysend::monitoring::tracking::ConversionTracking::NotSetUp,
            None,
            false,
        ),
        cells: format::cells(text, &total, "USD").unwrap(),
        sparklines: keysend::controllers::monitoring::metric_sparklines(&collected, None).unwrap(),
        charts,
        recommendations,
        subject,
    };
    let advice = export::RecommendationText {
        subject: "dental implants jobs".into(),
        confidence: Default::default(),
        measure: "Compare conversions after 30 days.".into(),
        finding: Some("dental implants jobs wastes spend".into()),
        action: "Add dental implants jobs as a negative keyword in Search - Implants.".into(),
        reason: "People who search for jobs are not patients.".into(),
        evidence: "dental implants jobs: $186 spend, 74 clicks, 0 conversions.".into(),
    };
    let account = export(
        Subject::AdAccount {
            assessment: Some(Box::new(
                keysend::monitoring::view::assessment_text(
                    text,
                    check.day,
                    check.assessment.as_ref().unwrap(),
                    &keysend::translation::Translations::english(),
                )
                .unwrap(),
            )),
            update: Some(export::UpdateText {
                date: format::long_date(text, today()),
                summary: "Spend was steady at $2,765.".into(),
                cause: None,
                lines: vec![export::UpdateLine {
                    campaign: "Search - Emergency".into(),
                    text: "42 conversions, up from 28.".into(),
                    cause: None,
                }],
                more_campaigns: 2,
            }),
            earlier: vec![export::EarlierUpdate {
                date: format::long_date(text, today()),
                summary: "Spend was steady.".into(),
            }],
            campaign_headers: keysend::monitoring::view::all_campaign_headers(text, true),
            campaigns: vec![export::CampaignRow {
                name: "Search - Emergency".into(),
                sparkline: None,
                cells: keysend::monitoring::view::all_campaign_cells(text, &total, "USD", true)
                    .unwrap(),
            }],
            deep_charts,
        },
        charts,
        vec![advice],
    );
    shown.push((
        "the Export of the Ad Account".into(),
        typst_text(&export::source(&account, text).unwrap()),
    ));
    let campaign = campaigns
        .iter()
        .find(|campaign| campaign.id == EMERGENCY)
        .unwrap();
    let campaign_changes =
        keysend::controllers::monitoring::export_changes(&store, &collected, Some(campaign))
            .await
            .unwrap();
    let campaign_charts = keysend::controllers::monitoring::campaign_export_charts(
        &store,
        &collected,
        campaign,
        &campaign_changes,
        &rules,
    )
    .await
    .unwrap();
    let row = |name: &str, id: Option<i64>, cost: f64, clicks: f64, conversions: f64| AuditRow {
        name: name.into(),
        id,
        counts: Counts {
            impressions: clicks * 10.0,
            clicks,
            conversions,
            cost,
            value: 0.0,
        },
        ..Default::default()
    };
    let full = Audit {
        campaigns: vec![row(
            "Search - Emergency",
            Some(EMERGENCY),
            9368.0,
            3760.0,
            186.0,
        )],
        search_terms: vec![
            row("emergency dentist", None, 1240.0, 520.0, 28.0),
            row("free dental care", None, 96.0, 61.0, 0.0),
        ],
        keywords: vec![
            row("emergency dentist", None, 3960.0, 1560.0, 82.0),
            row("tooth pain", None, 500.0, 300.0, 0.0),
            row("dentist open now", None, 900.0, 400.0, 2.0),
        ],
        landing_pages: vec![
            row("https://harbourdental.example/", None, 2958.0, 1150.0, 34.0),
            row(
                "https://harbourdental.example/emergency",
                None,
                6410.0,
                2610.0,
                152.0,
            ),
        ],
        ads: vec![
            AuditRow {
                strength: "GOOD".into(),
                ..row(
                    "Same-day emergency appointments",
                    Some(9001),
                    6100.0,
                    1930.0,
                    104.0,
                )
            },
            AuditRow {
                strength: "POOR".into(),
                ..row(" (9002)", None, 3200.0, 1240.0, 58.0)
            },
        ],
        ..Default::default()
    };
    let one_kind = Audit {
        campaigns: full.campaigns.clone(),
        keywords: full.keywords[..1].to_vec(),
        ..Default::default()
    };
    for (label, audit, judged) in [
        ("judged", &full, true),
        ("not judged", &full, false),
        ("one kind", &one_kind, true),
    ] {
        let deep = keysend::controllers::monitoring::campaign_deep_check(
            &store, &collected, campaign, judged,
        )
        .await
        .unwrap()
        .unwrap();
        let quality = deep.sections.into_iter().next().unwrap();
        let sections = keysend::monitoring::deep_view::sections(
            text,
            quality,
            Some(audit),
            EMERGENCY,
            "USD",
            judged,
        )
        .unwrap();
        let campaign_export = Export {
            ..export(
                Subject::Campaign {
                    name: campaign.name.clone(),
                    lines: vec![(
                        format::long_date(text, today()),
                        "42 conversions, up from 28.".into(),
                    )],
                    findings: vec!["Search - Emergency spends on free dental care.".into()],
                    deep_check: Some(CampaignDeepCheck {
                        date: deep.date,
                        sections,
                    }),
                },
                Vec::new(),
                Vec::new(),
            )
        };
        shown.push((
            format!("the Export of the Campaign, {label}"),
            typst_text(&export::source(&campaign_export, text).unwrap()),
        ));
    }
    let campaign_export = export(
        Subject::Campaign {
            name: campaign.name.clone(),
            lines: Vec::new(),
            findings: Vec::new(),
            deep_check: None,
        },
        campaign_charts,
        Vec::new(),
    );
    shown.push((
        "the Export of the Campaign".into(),
        typst_text(&export::source(&campaign_export, text).unwrap()),
    ));
    let without_deep = export(
        Subject::AdAccount {
            assessment: None,
            update: None,
            earlier: Vec::new(),
            campaign_headers: Vec::new(),
            campaigns: Vec::new(),
            deep_charts: DeepCharts {
                date: None,
                charts: Vec::new(),
            },
        },
        Vec::new(),
        Vec::new(),
    );
    shown.push((
        "the Export of an Ad Account without a Deep check".into(),
        typst_text(&export::source(&without_deep, text).unwrap()),
    ));

    let metrics =
        keysend::monitoring::metrics::window_metrics(&days, last, Window::SevenDays, None, true);
    let updates = vec![serde_json::from_value(json!({
        "day": day_number(today()),
        "summary": "Spend was steady.",
        "lines": [],
        "more_campaigns": 0,
        "metrics": metrics,
        "not_useful": false,
        "detection": keysend::monitoring::detection::detect(&days, &campaigns, &[], last, first_day, AllFollowAccount { judged: true }, "USD"),
    }))
    .unwrap()];
    let parts = email::campaign_parts(&days, &campaigns, &metrics);
    let recommendation = Recommendation {
        finding_key: "wasted-term:3:dental implants jobs".into(),
        action: "Add dental implants jobs as a negative keyword in Search - Implants.".into(),
        reason: "People who search for jobs are not patients.".into(),
        evidence: "dental implants jobs: $186 spend, 74 clicks, 0 conversions.".into(),
        measure: String::new(),
        intent: String::new(),
        subject: String::new(),
        confidence: Default::default(),
        campaign_id: Some(3),
        state: RecommendationState::Open,
    };
    let weekly = email::content(
        day_number(today()),
        vec![email::block(
            AD.into(),
            "Harbour Dental".into(),
            "USD".into(),
            metrics,
            updates,
            Some(recommendation),
            parts,
        )],
        vec!["Lakeshore Dental".into()],
    )
    .unwrap();
    let message = email::message(
        marker::LANGUAGE,
        &weekly,
        ORIGIN,
        &keysend::translation::Translations::empty(marker::LANGUAGE),
    )
    .unwrap();
    shown.push((
        "the Weekly email, HTML".into(),
        visible(&without_chips(&message.html)),
    ));
    shown.push(("the Weekly email, text".into(), message.text));
    shown.push(("the Weekly email, subject".into(), message.subject));
    for event_key in [
        EventKey::CampaignStopped(EMERGENCY),
        EventKey::SpendDoubled,
        EventKey::ConversionsZero,
    ] {
        let label = format!("the Alert {event_key:?}");
        let alert = Alert {
            day: last,
            event_key,
            campaign_id: Some(EMERGENCY),
            name: "Search - Emergency".into(),
            metrics: Metrics {
                spend: 300.0,
                clicks: 12.0,
                impressions: 400.0,
                conversions: Some(0.0),
                conversion_value: None,
            },
            normal_value: 120.0,
            body: "Harbour Dental: Search - Emergency stopped serving.".into(),
        };
        let message = alerts::message(
            marker::LANGUAGE,
            &alert,
            AD,
            "Harbour Dental",
            "USD",
            ORIGIN,
        )
        .unwrap();
        shown.push((
            format!("{label}, HTML"),
            visible(&without_chips(&message.html)),
        ));
        shown.push((format!("{label}, text"), message.text));
        shown.push((format!("{label}, subject"), message.subject));
    }

    let own = [
        "dental implants jobs",
        "Compare conversions after 30 days.",
        "Add dental implants jobs as a negative keyword in Search - Implants.",
        "People who search for jobs are not patients.",
        "dental implants jobs: $186 spend, 74 clicks, 0 conversions.",
        "dental implants jobs wastes spend",
        "Search - Emergency spends on free dental care.",
        "Spend was steady at $2,765.",
        "Spend was steady.",
        "Search - Emergency: 42 conversions, up from 28.",
        "42 conversions, up from 28.",
        "Harbour Dental: Search - Emergency stopped serving.",
        "Harbour Dental",
        "Lakeshore Dental",
        "emergency dentist",
        "free dental care",
        "tooth pain",
        "dentist open now",
        "Same-day emergency appointments",
        "harbourdental.example/emergency",
        "harbourdental.example",
        "Daily budget changed from $300.00 to $340.00",
        "Daily budget changed from $45.00 to $20.00",
        "Bid strategy changed from maximize clicks to maximize conversions",
    ];
    let stored = stored(&f, &own).await;
    let mut report = Vec::new();
    for (name, text) in &shown {
        let leaked = leaks(text, &stored, &english);
        if !leaked.is_empty() {
            report.push(format!(
                "{name}: {}",
                leaked.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    assert!(
        report.is_empty(),
        "English fixed words are left in the test language:\n{}",
        report.join("\n")
    );
}

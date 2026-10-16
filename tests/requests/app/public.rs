use super::*;
use chrono::{Days, NaiveDate};
use keysend::{
    monitoring::{
        alerts::Alert,
        deep_check::{DeepCheck, Finding},
        detection,
        metrics::Metrics,
        update::{self, CampaignLine},
        worker::{self, EmailJob},
    },
    monitoring::{
        audit::{Audit, Counts, Data},
        findings,
    },
    settings::Settings,
    workspace::{
        history::{day_number, Campaign, CampaignDay},
        monitoring::{AccountSettings, Recommendation, RecommendationState},
    },
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

const AD: &str = "4829153307";
const LINK: &str = "http://localhost:5150/ad-accounts/4829153307";

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
}

fn end() -> NaiveDate {
    today().pred_opt().unwrap()
}

fn campaigns() -> Vec<Campaign> {
    [
        (701, "Search - Brand"),
        (702, "Search - Emergency"),
        (703, "Search - Implants"),
        (704, "Display - Remarketing"),
    ]
    .into_iter()
    .map(|(id, name)| Campaign {
        id,
        name: name.into(),
        status: "ENABLED".into(),
        channel_type: "SEARCH".into(),
        bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
        budget_micros: Some(80_000_000),
    })
    .collect()
}

fn history() -> Vec<CampaignDay> {
    let oldest = end().checked_sub_days(Days::new(70)).unwrap();
    let mut days = Vec::new();
    for offset in 0..=70 {
        let day = oldest.checked_add_days(Days::new(offset)).unwrap();
        let this_week = day > end().checked_sub_days(Days::new(7)).unwrap();
        for (id, impressions, clicks, cost, conversions) in [
            (701, 400.0, 40.0, 80.0, 6.0),
            (702, 300.0, 30.0, 150.0, if this_week { 6.0 } else { 4.0 }),
            (703, 200.0, 20.0, 120.0, 1.0),
            (704, 1000.0, 10.0, 45.0, 1.0),
        ] {
            days.push(CampaignDay {
                day,
                campaign_id: id,
                device: 1,
                counts: Counts {
                    impressions,
                    clicks,
                    cost,
                    conversions,
                    value: conversions * 5.0,
                },
            });
        }
    }
    days
}

fn data() -> Data {
    Data {
        currency: "USD".into(),
        reporting: Counts::default(),
        audit: Audit {
            conversion_actions: Some(1),
            ..Default::default()
        },
    }
}

fn evidence(judged: bool) -> String {
    if judged {
        "dental implants jobs: $186 spend, 74 clicks, 0 conversions in the last 30 days.".into()
    } else {
        "dental implants jobs: $186 spend, 74 clicks in the last 30 days.".into()
    }
}

struct Harbour {
    f: Fixture,
    workspace: identity::SignedIn,
}

impl Harbour {
    fn store(&self) -> WorkspaceStore<'_> {
        WorkspaceStore::new(&self.f.ctx.db, &self.workspace.workspace)
    }

    async fn sql(&self, query: &str, values: Vec<sea_orm::Value>) {
        self.f
            .ctx
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                query,
                values,
            ))
            .await
            .unwrap();
    }

    async fn new(judged: bool) -> Self {
        let f =
            Fixture::configured_clock(false, false, true, "google", 15_000, "2026-10-12T12:00:00Z")
                .await;
        let workspace = identity::authenticate(&f.ctx.db, f.a.value())
            .await
            .unwrap()
            .unwrap();
        let harbour = Self { f, workspace };
        harbour
            .sql(
                "UPDATE ad_accounts SET name='Harbour Dental' WHERE workspace_id=? AND customer_id=?",
                vec![harbour.workspace.workspace.id().into(), AD.into()],
            )
            .await;
        harbour
            .sql(
                "UPDATE workspace_trials SET started_at=? WHERE workspace_id=?",
                vec![
                    today()
                        .checked_sub_days(Days::new(6))
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc()
                        .timestamp()
                        .into(),
                    harbour.workspace.workspace.id().into(),
                ],
            )
            .await;
        let store = harbour.store();
        store
            .save_ad_account_metadata(AD, "USD", "UTC")
            .await
            .unwrap();
        let days = history();
        store
            .save_history(AD, days[0].day, end(), &campaigns(), &days, &[])
            .await
            .unwrap();
        if !judged {
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
        }
        let finding = Finding {
            campaign_id: Some(703),
            rule: findings::Finding {
                key: "search-term-waste:703".into(),
                title: "dental implants jobs wastes spend".into(),
                detail: evidence(judged),
                subjects: vec!["Search - Implants".into()],
                spend: 186.0,
            },
        };
        let recommendation = Recommendation {
            finding_key: "search-term-waste:703".into(),
            action: "Add dental implants jobs as a negative keyword in Search - Implants.".into(),
            reason: "People who search for jobs are not patients.".into(),
            evidence: evidence(judged),
            campaign_id: Some(703),
            state: RecommendationState::Open,
        };
        store
            .refresh_deep_check(
                AD,
                &DeepCheck {
                    day: day_number(today()) - 1,
                    data: data(),
                    findings: vec![finding],
                    judge_by_conversions: judged,
                    recommendations_generated: true,
                    by_campaign: Default::default(),
                },
                &[recommendation],
            )
            .await
            .unwrap();
        let metric_days = store
            .monitoring_days(AD, day_number(end()) - 62, day_number(end()))
            .await
            .unwrap();
        let detected = detection::detect(
            &metric_days,
            &campaigns(),
            &[],
            day_number(end()),
            day_number(days[0].day),
            keysend::monitoring::campaign_settings::AllFollowAccount { judged },
            "USD",
        );
        let mut note = update::fallback(&detected);
        note.day = day_number(today());
        note.summary = if judged {
            "Spend was steady at $2,765, and conversions rose from 84 to 98. Most of the gain came from Search - Emergency.".into()
        } else {
            "Spend was steady at $2,765.".into()
        };
        note.lines = vec![CampaignLine {
            campaign_id: 702,
            text: if judged {
                "42 conversions, up from 28.".into()
            } else {
                "210 clicks, the same as the week before.".into()
            },
            cause: None,
        }];
        store.save_update(AD, &note).await.unwrap();
        note.day = day_number(today()) - 3;
        note.summary = "Display - Remarketing spent $45 a day, as usual.".into();
        note.lines = vec![];
        store.save_update(AD, &note).await.unwrap();
        harbour
    }

    fn emails(&self, subject_start: &str) -> Vec<Value> {
        self.f
            .calls
            .lock()
            .unwrap()
            .emails
            .iter()
            .filter(|email| {
                email["subject"]
                    .as_str()
                    .unwrap()
                    .starts_with(subject_start)
            })
            .cloned()
            .collect()
    }

    async fn deliver(&self, alert: Option<Alert>) {
        if let Some(alert) = alert {
            self.store()
                .save_alerts(
                    AD,
                    &[alert],
                    Settings::load(&self.f.ctx).unwrap().now().timestamp(),
                )
                .await
                .unwrap();
        }
        worker::send(
            &self.f.ctx,
            &EmailJob {
                workspace_id: self.workspace.workspace.id().into(),
            },
        )
        .await
        .unwrap();
    }
}

fn stopped(judged: bool) -> Alert {
    Alert {
        day: day_number(end()),
        event_key: keysend::monitoring::alerts::EventKey::CampaignStopped(702),
        campaign_id: Some(702),
        name: "Search - Emergency".into(),
        metrics: Metrics {
            conversions: judged.then_some(0.0),
            ..Default::default()
        },
        normal_value: 0.0,
        body: "Harbour Dental: Search - Emergency stopped serving on 2026-10-11.".into(),
    }
}

fn chip(name: &str) -> String {
    format!(
        r#"<b style="padding:1px 5px; border-radius:4px; background:#EEE9F6; color:#3F1D85; font-weight:700;">{name}</b>"#
    )
}

#[tokio::test]
#[serial]
async fn the_weekly_email_is_html_with_absolute_links_and_a_text_part() {
    let h = Harbour::new(true).await;
    h.deliver(None).await;
    let sent = h.emails("Your week in Google Ads");
    assert_eq!(sent.len(), 1);
    let email = &sent[0];
    assert_eq!(email["subject"], "Your week in Google Ads: Harbour Dental");
    assert_eq!(email["to"], json!(["a@example.com"]));
    let html = email["html"].as_str().unwrap();
    keep("email-weekly.html", html.as_bytes());
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains(">Your week in Google Ads</h1>"));
    assert!(html.contains(">Harbour Dental</h2>"));
    assert!(html.contains("5 to 11 October"));
    assert!(html.contains("Spend was steady at $2,765, and conversions rose from 84 to 98."));
    assert!(html.contains(&format!(
        "{}: 42 conversions, up from 28.",
        chip("Search - Emergency")
    )));
    assert!(html.contains(&format!(
        "Most of the gain came from {}.",
        chip("Search - Emergency")
    )));
    assert!(html.contains(">Cost per conversion</div>"));
    assert!(html.contains(">$28.21</div>"));
    assert!(html.contains(">down 14% from $32.92</div>"));
    assert!(html.contains("Earlier Updates this week"));
    assert!(html.contains("Friday 9 October"));
    assert!(html.contains(&format!(
        "Add dental implants jobs as a negative keyword in {}.",
        chip("Search - Implants")
    )));
    assert!(html.contains(
        "Finding: dental implants jobs: $186 spend, 74 clicks, 0 conversions in the last 30 days."
    ));
    assert!(html.contains(&format!("href=\"{LINK}\"")));
    assert!(html.contains("We only read your Google Ads. We never change the Ad Account."));
    assert!(!html.contains("<img"));
    assert!(!html.contains("src="));
    assert!(!html.contains("href=\"/"));
    assert!(!html.contains("{{") && !html.contains("{%"));
    let text = email["text"].as_str().unwrap();
    assert_eq!(
        text,
        format!(
            "Your week in Google Ads\nWeekly email of Monday 12 October. Each Ad Account is compared with the 7 days before.\n\nHarbour Dental\n5 to 11 October, compared with the 7 days before\n\nMonday 12 October\nSpend was steady at $2,765, and conversions rose from 84 to 98. Most of the gain came from Search - Emergency.\n- Search - Emergency: 42 conversions, up from 28.\n\n\nMetrics\nSpend: $2,765 (no change)\nConversions: 98 (up 17% from 84)\nCost per conversion: $28.21 (down 14% from $32.92)\n\nSpend and conversions by Campaign\nSearch - Implants takes 30% of spend and brings 7% of conversions.\n- Search - Emergency: 38% of spend, 43% of conversions\n- Search - Implants: 30% of spend, 7% of conversions\n- Search - Brand: 20% of spend, 43% of conversions\n- Display - Remarketing: 11% of spend, 7% of conversions\n\nEarlier Updates this week\n\nFriday 9 October\nDisplay - Remarketing spent $45 a day, as usual.\n\nRecommendation\nAdd dental implants jobs as a negative keyword in Search - Implants.\nPeople who search for jobs are not patients.\nFinding: dental implants jobs: $186 spend, 74 clicks, 0 conversions in the last 30 days.\n\nOpen Harbour Dental: {LINK}\n\n---\nWe only read your Google Ads. We never change the Ad Account.\nThis is your Weekly email from KeySend. To leave an Ad Account out of it, set Weekly email to Off in the Settings of that Ad Account.",
        )
    );
}

#[tokio::test]
#[serial]
async fn the_alert_email_states_one_event_with_an_absolute_link() {
    let h = Harbour::new(true).await;
    h.deliver(Some(stopped(true))).await;
    let sent = h.emails("Alert: ");
    assert_eq!(sent.len(), 1);
    let email = &sent[0];
    assert_eq!(
        email["subject"],
        "Alert: Search - Emergency stopped in Harbour Dental"
    );
    let html = email["html"].as_str().unwrap();
    keep("email-alert.html", html.as_bytes());
    assert!(html.contains(">Alert for Harbour Dental, Sunday 11 October</td>"));
    assert!(html.contains(&format!("{} stopped</h1>", chip("Search - Emergency"))));
    assert!(html.contains(&format!("{} had no impressions, clicks or spend on Sunday 11 October, on a day when it normally serves.", chip("Search - Emergency"))));
    assert!(
        html.contains("Sunday 11 October: Spend $0.00, Clicks 0, Impressions 0, Conversions 0.")
    );
    assert!(html.contains("What to do: Open Google Ads and check the status, the daily budget and the payment method of this Campaign. We cannot change it for you."));
    assert!(html.contains(&format!("href=\"{LINK}\"")));
    assert!(html.contains("We send an Alert the next morning when a Campaign stops or spend doubles. We send an Alert about conversions that go to zero two days later, because Google Ads can still add conversions to the newest days."));
    assert!(!html.contains("<img"));
    assert_eq!(
        email["text"].as_str().unwrap(),
        format!(
            "Alert for Harbour Dental, Sunday 11 October\nSearch - Emergency stopped\n\nSearch - Emergency had no impressions, clicks or spend on Sunday 11 October, on a day when it normally serves.\n\nSunday 11 October: Spend $0.00, Clicks 0, Impressions 0, Conversions 0.\n\nWhat to do: Open Google Ads and check the status, the daily budget and the payment method of this Campaign. We cannot change it for you.\n\nOpen Harbour Dental: {LINK}\n\n---\nWe only read your Google Ads. We never change the Ad Account.\nWe send an Alert the next morning when a Campaign stops or spend doubles. We send an Alert about conversions that go to zero two days later, because Google Ads can still add conversions to the newest days."
        )
    );
}

#[tokio::test]
#[serial]
async fn emails_of_an_ad_account_not_judged_by_conversions_state_it_once_and_show_no_conversion_metric(
) {
    let h = Harbour::new(false).await;
    h.deliver(Some(stopped(false))).await;
    let weekly = &h.emails("Your week in Google Ads")[0];
    let alert = &h.emails("Alert: ")[0];
    for part in [
        weekly["html"].as_str().unwrap(),
        weekly["text"].as_str().unwrap(),
    ] {
        assert!(part.contains("Cost per click"));
        assert!(
            !part.to_lowercase().contains("conversion"),
            "conversion text in {part}"
        );
    }
    for part in [
        alert["html"].as_str().unwrap(),
        alert["text"].as_str().unwrap(),
    ] {
        assert!(part.contains("Spend $0.00, Clicks 0, Impressions 0."));
        assert!(!part.contains("Conversions"), "conversion Metric in {part}");
    }
    assert_eq!(
        weekly["text"]
            .as_str()
            .unwrap()
            .matches("Results cannot be judged.")
            .count(),
        1
    );
    assert_eq!(
        weekly["html"]
            .as_str()
            .unwrap()
            .matches("Results cannot be judged.")
            .count(),
        1
    );
    assert!(weekly["text"].as_str().unwrap().contains(
        "Spend: $2,765 (no change)\nClicks: 700 (no change)\nCost per click: $3.95 (no change)\n"
    ));
}

#[tokio::test]
#[serial]
async fn a_weekly_email_names_every_ad_account_in_its_subject() {
    let h = Harbour::new(true).await;
    h.sql("INSERT INTO ad_accounts(id,workspace_id,customer_id,name,google_connection_id) SELECT 'second',workspace_id,'1111111111','Northgate Physio',google_connection_id FROM ad_accounts WHERE workspace_id=? AND customer_id=?", vec![h.workspace.workspace.id().into(), AD.into()]).await;
    h.store()
        .save_ad_account_metadata("1111111111", "USD", "UTC")
        .await
        .unwrap();
    let mut note = h.store().updates(AD, 0, i64::MAX).await.unwrap().remove(0);
    note.summary = "Nothing notable changed in the last 7 days.".into();
    note.lines = vec![];
    h.store().save_update("1111111111", &note).await.unwrap();
    h.deliver(None).await;
    let sent = h.emails("Your week in Google Ads");
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["subject"],
        "Your week in Google Ads: Harbour Dental and Northgate Physio"
    );
    let html = sent[0]["html"].as_str().unwrap();
    assert!(html.contains(">Northgate Physio</h2>"));
    assert!(html.contains("href=\"http://localhost:5150/ad-accounts/1111111111\""));
}

fn png() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(64, 64, image::Rgba([81, 38, 166, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn keep(name: &str, bytes: &[u8]) {
    if let Ok(directory) = env::var("KEYSEND_EXPORT_TEST_ARTIFACTS") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(format!("{directory}/{name}"), bytes).unwrap();
    }
}

#[tokio::test]
#[serial]
async fn the_export_is_a_pdf_of_one_ad_account_for_one_window() {
    let h = Harbour::new(true).await;
    h.store()
        .save_branding(AD, "Harbour Dental Group", "#5126A6")
        .await
        .unwrap();
    h.store().save_logo(AD, &png()).await.unwrap();
    for (query, name) in [
        ("?window=7", "harbour-dental-group-last-7-days.pdf"),
        ("?window=30", "harbour-dental-group-last-30-days.pdf"),
        ("", "harbour-dental-group-last-7-days.pdf"),
    ] {
        let response =
            h.f.request
                .get(&format!("/ad-accounts/{AD}/export.pdf{query}"))
                .add_cookie(h.f.a.clone())
                .await;
        assert_eq!(response.status_code(), 200, "{query}");
        assert_eq!(response.header("content-type"), "application/pdf");
        assert_eq!(
            response.header("content-disposition"),
            format!("attachment; filename=\"{name}\"").as_str()
        );
        assert_eq!(response.header("cache-control"), "no-store");
        let bytes = response.as_bytes();
        assert!(bytes.starts_with(b"%PDF-"), "{query}");
        assert!(bytes.len() > 2_000, "{query}");
        keep(
            &format!("export{}.pdf", query.replace(['?', '='], "-")),
            bytes,
        );
    }
    let wrong =
        h.f.request
            .get(&format!("/ad-accounts/{AD}/export.pdf?window=14"))
            .add_cookie(h.f.a.clone())
            .await;
    assert_eq!(wrong.status_code(), 422);
}

#[tokio::test]
#[serial]
async fn an_export_is_in_romanian_for_a_romanian_user_and_in_english_for_any_other_language() {
    let h = Harbour::new(true).await;
    let campaign = h.store().campaigns(AD).await.unwrap().remove(0).id;
    h.store()
        .save_branding(AD, "Harbour Dental Group", "#5126A6")
        .await
        .unwrap();
    h.translate_into_romanian().await;
    for language in keysend::language::Language::ALL {
        let code = language.code();
        h.sql(
            "UPDATE users SET language=? WHERE email='a@example.com'",
            vec![code.into()],
        )
        .await;
        let signed = identity::authenticate(&h.f.ctx.db, h.f.a.value())
            .await
            .unwrap()
            .unwrap();
        let (metrics, file) = if code == "ro" {
            ("Indicatori\n", "harbour-dental-group-ultimele-7-zile.pdf")
        } else {
            ("Metrics\n", "harbour-dental-group-last-7-days.pdf")
        };
        for campaign in [None, Some(campaign)] {
            let words = export_words(&h.f, &signed, AD, campaign, "7").await;
            assert!(words.contains(metrics), "{code} {campaign:?}\n{words}");
        }
        let path = format!("/ad-accounts/{AD}/export.pdf?window=7");
        let plain = h.f.request.get(&path).add_cookie(h.f.a.clone()).await;
        assert_eq!(
            plain.header("content-disposition"),
            format!("attachment; filename=\"{file}\"").as_str(),
            "{code}"
        );
        for other in ["en", "de", "ro", "xx"] {
            let asked =
                h.f.request
                    .get(&format!("{path}&language={other}"))
                    .add_cookie(h.f.a.clone())
                    .await;
            assert_eq!(asked.status_code(), 200, "{code} {other}");
            assert!(
                asked.as_bytes() == plain.as_bytes(),
                "{code}: ?language={other} changed the PDF"
            );
        }
    }
}

#[tokio::test]
#[serial]
async fn a_romanian_export_is_refused_until_its_stored_text_is_translated_and_then_is_romanian() {
    let h = Harbour::new(true).await;
    h.sql(
        "UPDATE users SET language='ro' WHERE email='a@example.com'",
        vec![],
    )
    .await;
    let refusal = "Raportul conține text pe care KeySend nu l-a tradus încă în română. KeySend traduce textul nou după ce citește datele din Google Ads. Încercați din nou mai târziu sau alegeți engleza în Setări pentru a descărca acum un PDF în engleză.";
    let paths = [
        format!("/ad-accounts/{AD}/export.pdf?window=7"),
        format!("/ad-accounts/{AD}/campaigns/703/export.pdf?window=7"),
        format!("/ad-accounts/{AD}/export.pdf?window=7&language=en"),
    ];
    for path in &paths {
        let refused = h.f.request.get(path).add_cookie(h.f.a.clone()).await;
        assert_eq!(refused.status_code(), 409, "{path}");
        assert_eq!(refused.text(), refusal, "{path}");
        assert!(!refused.as_bytes().starts_with(b"%PDF"), "{path}");
    }

    h.translate_into_romanian().await;
    let signed = identity::authenticate(&h.f.ctx.db, h.f.a.value())
        .await
        .unwrap()
        .unwrap();
    let words = export_words(&h.f, &signed, AD, None, "7").await;
    for expected in [
        "Indicatori\n",
        "RO Add dental implants jobs as a negative keyword in Search - Implants.\n",
        "RO Display - Remarketing spent $45 a day, as usual.\n",
    ] {
        assert!(words.contains(expected), "{expected}\n{words}");
    }
    assert!(!words.contains("\nAdd dental implants jobs"), "{words}");
    let campaign = export_words(&h.f, &signed, AD, Some(703), "7").await;
    assert!(
        campaign
            .contains("RO Add dental implants jobs as a negative keyword in Search - Implants.\n"),
        "{campaign}"
    );
    for (path, name) in paths
        .iter()
        .zip(["export-ro.pdf", "campaign-export-ro.pdf", ""])
    {
        let response = h.f.request.get(path).add_cookie(h.f.a.clone()).await;
        assert_eq!(response.status_code(), 200, "{path}");
        assert!(response.as_bytes().starts_with(b"%PDF-"), "{path}");
        if !name.is_empty() {
            keep(name, response.as_bytes());
        }
    }
}

#[tokio::test]
#[serial]
async fn the_export_of_an_ad_account_not_judged_by_conversions_is_still_a_pdf() {
    let h = Harbour::new(false).await;
    let response =
        h.f.request
            .get(&format!("/ad-accounts/{AD}/export.pdf?window=30"))
            .add_cookie(h.f.a.clone())
            .await;
    assert_eq!(response.status_code(), 200);
    assert!(response.as_bytes().starts_with(b"%PDF-"));
    keep("export-unjudged.pdf", response.as_bytes());
}

#[tokio::test]
#[serial]
async fn the_export_is_refused_for_another_workspace_a_stranger_and_ended_access() {
    let h = Harbour::new(true).await;
    let path = format!("/ad-accounts/{AD}/export.pdf?window=7");
    assert_eq!(
        h.f.request
            .get(&path)
            .add_cookie(h.f.b.clone())
            .await
            .status_code(),
        404
    );
    let stranger = h.f.request.get(&path).await;
    assert_eq!(stranger.status_code(), 303);
    assert_eq!(stranger.header("location"), "/sign-in");
    assert_eq!(
        h.f.request
            .get("/ad-accounts/9999999999/export.pdf")
            .add_cookie(h.f.a.clone())
            .await
            .status_code(),
        404
    );
    h.sql(
        "UPDATE workspace_trials SET started_at=started_at-2592000 WHERE workspace_id=?",
        vec![h.workspace.workspace.id().into()],
    )
    .await;
    let ended = h.f.request.get(&path).add_cookie(h.f.a.clone()).await;
    assert_eq!(ended.status_code(), 402);
    assert_eq!(
        ended.text(),
        "Your Trial has ended. Start the Subscription to export."
    );
    assert!(!ended.as_bytes().starts_with(b"%PDF"));
}

#[tokio::test]
#[serial]
async fn the_campaign_export_is_a_pdf_for_the_owner_and_has_the_name_of_the_ad_account_and_the_campaign(
) {
    let h = Harbour::new(true).await;
    let campaign = h.store().campaigns(AD).await.unwrap().remove(0);
    let path = format!("/ad-accounts/{AD}/campaigns/{}/export.pdf", campaign.id);
    for (query, days) in [("?window=7", 7), ("?window=30", 30), ("", 7)] {
        let response =
            h.f.request
                .get(&format!("{path}{query}"))
                .add_cookie(h.f.a.clone())
                .await;
        assert_eq!(response.status_code(), 200, "{query}");
        assert_eq!(response.header("content-type"), "application/pdf");
        assert_eq!(
            response.header("content-disposition"),
            format!(
                "attachment; filename=\"harbour-dental-{}-last-{days}-days.pdf\"",
                campaign
                    .name
                    .to_lowercase()
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("-")
            )
            .as_str()
        );
        assert_eq!(response.header("cache-control"), "no-store");
        assert!(response.as_bytes().starts_with(b"%PDF-"), "{query}");
        assert!(response.as_bytes().len() > 2_000, "{query}");
        keep(
            &format!("campaign-export{}.pdf", query.replace(['?', '='], "-")),
            response.as_bytes(),
        );
    }
    let wrong =
        h.f.request
            .get(&format!("{path}?window=14"))
            .add_cookie(h.f.a.clone())
            .await;
    assert_eq!(wrong.status_code(), 422);
}

#[tokio::test]
#[serial]
async fn the_campaign_export_is_refused_like_the_ad_account_export() {
    let h = Harbour::new(true).await;
    let campaign = h.store().campaigns(AD).await.unwrap().remove(0).id;
    let path = format!("/ad-accounts/{AD}/campaigns/{campaign}/export.pdf?window=7");
    assert_eq!(
        h.f.request
            .get(&path)
            .add_cookie(h.f.b.clone())
            .await
            .status_code(),
        404
    );
    let stranger = h.f.request.get(&path).await;
    assert_eq!(stranger.status_code(), 303);
    assert_eq!(stranger.header("location"), "/sign-in");
    for unknown in [
        format!("/ad-accounts/{AD}/campaigns/999999/export.pdf"),
        format!("/ad-accounts/9999999999/campaigns/{campaign}/export.pdf"),
    ] {
        assert_eq!(
            h.f.request
                .get(&unknown)
                .add_cookie(h.f.a.clone())
                .await
                .status_code(),
            404,
            "{unknown}"
        );
    }
    h.sql(
        "UPDATE workspace_trials SET started_at=started_at-2592000 WHERE workspace_id=?",
        vec![h.workspace.workspace.id().into()],
    )
    .await;
    let ended = h.f.request.get(&path).add_cookie(h.f.a.clone()).await;
    assert_eq!(ended.status_code(), 402);
    assert_eq!(
        ended.text(),
        "Your Trial has ended. Start the Subscription to export."
    );
    assert!(!ended.as_bytes().starts_with(b"%PDF"));
}

struct Plan {
    f: Fixture,
    workspace: identity::SignedIn,
    now: i64,
}

impl Plan {
    async fn new() -> Self {
        let f = Fixture::new(false).await;
        let workspace = identity::authenticate(&f.ctx.db, f.a.value())
            .await
            .unwrap()
            .unwrap();
        let now = Settings::load(&f.ctx).unwrap().now().timestamp();
        Self { f, workspace, now }
    }

    async fn sql(&self, query: &str, values: Vec<sea_orm::Value>) {
        self.f
            .ctx
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                query,
                values,
            ))
            .await
            .unwrap();
    }

    async fn trial_started(&self, days_ago: i64) {
        self.sql(
            "UPDATE workspace_trials SET started_at=? WHERE workspace_id=?",
            vec![
                (self.now - days_ago * 86_400).into(),
                self.workspace.workspace.id().into(),
            ],
        )
        .await;
    }

    async fn page(&self) -> String {
        let response = self
            .f
            .request
            .get("/subscription")
            .add_cookie(self.f.a.clone())
            .await;
        assert_eq!(response.status_code(), 200);
        assert_eq!(response.header("cache-control"), "no-store");
        response.text()
    }
}

#[tokio::test]
#[serial]
async fn the_subscription_page_of_a_trial_counts_the_days_left() {
    let plan = Plan::new().await;
    plan.trial_started(10).await;
    let html = plan.page().await;
    keep("subscription-trial.html", html.as_bytes());
    keep("home.html", plan.f.request.get("/").await.as_bytes());
    assert!(html.contains("<h1 class=\"heading-xl\">Subscription</h1>"));
    assert!(html.contains("Your Trial has 4 days left. It ends on 15 October."));
    assert!(html.contains("<dd class=\"num\">$49 per month</dd>"));
    assert!(html.contains("<dd>Up to 3. You use 1.</dd>"));
    assert!(html.contains("On the day you start the Subscription"));
    assert!(html.contains("action=\"/subscription\""));
    assert!(html.contains(">Start the Subscription</button>"));
    assert!(!html.contains("disabled"));
    assert!(html.contains("<a href=\"/subscription\" aria-current=\"page\">Subscription</a>"));
    plan.trial_started(13).await;
    assert!(plan
        .page()
        .await
        .contains("Your Trial has 1 day left. It ends on 12 October."));
}

#[tokio::test]
#[serial]
async fn the_subscription_page_after_the_trial_offers_to_start_it() {
    let plan = Plan::new().await;
    plan.trial_started(20).await;
    let html = plan.page().await;
    keep("subscription-trial-ended.html", html.as_bytes());
    assert!(
        html.contains("Your Trial ended on 5 October. We have stopped reading your Ad Accounts.")
    );
    assert!(html.contains("Your Account history and earlier Updates are still here."));
    assert!(html.contains("<dt>First payment</dt><dd>Today</dd>"));
    assert!(html.contains(">Start the Subscription</button>"));
    assert!(!html.contains("Change the payment card"));
}

#[tokio::test]
#[serial]
async fn the_subscription_page_of_an_active_subscription_keeps_the_card_and_the_cancel_actions() {
    let plan = Plan::new().await;
    plan.trial_started(30).await;
    plan.sql(
        "INSERT INTO subscriptions(workspace_id,stripe_subscription_id,created_at,status,period_start,period_end) VALUES(?,?,?,?,?,?)",
        vec![
            plan.workspace.workspace.id().into(),
            "sub_fixture".into(),
            (plan.now - 18 * 86_400).into(),
            "active".into(),
            (plan.now - 18 * 86_400).into(),
            (plan.now + 12 * 86_400).into(),
        ],
    )
    .await;
    let html = plan.page().await;
    keep("subscription-active.html", html.as_bytes());
    assert!(html.contains("Your Subscription is active. The next payment is $49 on 23 October."));
    assert!(html.contains("<dt>Started</dt><dd>23 September 2026</dd>"));
    assert_eq!(html.matches("action=\"/subscription/portal\"").count(), 2);
    assert!(html.contains(">Change the payment card</button>"));
    assert!(html.contains(">Cancel the Subscription</button>"));
    assert!(!html.contains("Start the Subscription"));
    let portal = plan
        .f
        .request
        .post("/subscription/portal")
        .add_header("origin", ORIGIN)
        .add_cookie(plan.f.a.clone())
        .await;
    assert_eq!(portal.status_code(), 404);
    plan.sql(
        "UPDATE subscriptions SET status='canceled' WHERE workspace_id=?",
        vec![plan.workspace.workspace.id().into()],
    )
    .await;
    let html = plan.page().await;
    assert!(
        html.contains("Your Subscription is not active. We have stopped reading your Ad Accounts.")
    );
    assert!(html.contains(">Start the Subscription</button>"));
}

#[tokio::test]
#[serial]
async fn more_than_three_ad_accounts_cannot_start_a_subscription() {
    let plan = Plan::new().await;
    for number in 1..=3 {
        plan.sql("INSERT INTO ad_accounts(id,workspace_id,customer_id,name,google_connection_id) SELECT ?,workspace_id,?,?,google_connection_id FROM ad_accounts WHERE workspace_id=? AND customer_id='4829153307'", vec![format!("extra{number}").into(), format!("{number:010}").into(), format!("Extra {number}").into(), plan.workspace.workspace.id().into()]).await;
    }
    let html = plan.page().await;
    assert!(html.contains("A Subscription covers up to 3 Ad Accounts, and you have 4."));
    assert!(html.contains("Remove 1 Ad Account to start the Subscription."));
    assert!(html.contains(
        "<button class=\"button\" type=\"submit\" disabled>Start the Subscription</button>"
    ));
    let response = plan
        .f
        .request
        .post("/subscription")
        .add_header("origin", ORIGIN)
        .add_cookie(plan.f.a.clone())
        .await;
    assert_eq!(response.status_code(), 422);
    assert_eq!(
        response.text(),
        "A Subscription supports up to 3 Ad Accounts."
    );
    assert!(plan.f.calls.lock().unwrap().stripe.requests.is_empty());
}

#[tokio::test]
#[serial]
async fn starting_the_subscription_asks_stripe_for_the_monthly_price_and_returns_to_the_page() {
    let plan = Plan::new().await;
    let response = plan
        .f
        .request
        .post("/subscription")
        .add_header("origin", ORIGIN)
        .add_cookie(plan.f.a.clone())
        .await;
    assert_eq!(response.status_code(), 303, "{}", response.text());
    assert!(response
        .header("location")
        .to_str()
        .unwrap()
        .starts_with("https://checkout.stripe.com/c/pay/"));
    {
        let calls = plan.f.calls.lock().unwrap();
        let form = &calls
            .stripe
            .requests
            .iter()
            .find(|(kind, _)| kind == "checkout/sessions")
            .unwrap()
            .1;
        assert_eq!(form["mode"], "subscription");
        assert_eq!(form["line_items[0][price]"], "price_fixture");
        assert_eq!(
            form["success_url"],
            format!("{ORIGIN}/subscription?checkout=returned")
        );
        assert_eq!(form["cancel_url"], format!("{ORIGIN}/subscription"));
    }
    let returned = plan
        .f
        .request
        .get("/subscription?checkout=returned")
        .add_cookie(plan.f.a.clone())
        .await
        .text();
    assert!(returned.contains("We are waiting for Stripe to confirm your payment."));
    assert!(!plan.page().await.contains("We are waiting for Stripe"));
}

#[tokio::test]
#[serial]
async fn the_old_billing_address_leads_to_the_subscription_and_each_workspace_sees_its_own() {
    let plan = Plan::new().await;
    let moved = plan
        .f
        .request
        .get("/billing")
        .add_cookie(plan.f.a.clone())
        .await;
    assert_eq!(moved.status_code(), 303);
    assert_eq!(moved.header("location"), "/subscription");
    plan.trial_started(10).await;
    let other = plan
        .f
        .request
        .get("/subscription")
        .add_cookie(plan.f.b.clone())
        .await;
    assert_eq!(other.status_code(), 200);
    assert!(other.text().contains("<dd>Up to 3. You use 0.</dd>"));
    assert_eq!(plan.f.request.get("/subscription").await.status_code(), 303);
}

impl Harbour {
    async fn translate_into_romanian(&self) {
        let now = Settings::load(&self.f.ctx).unwrap().now();
        let translated = keysend::translation::collect::shown(&self.store(), AD, now)
            .await
            .unwrap()
            .iter()
            .map(|item| {
                (
                    keysend::translation::hash(&item.text),
                    format!("RO {}", item.text),
                )
            })
            .collect::<Vec<_>>();
        self.store()
            .save_translations(
                AD,
                keysend::language::Language::Ro,
                &translated,
                now.timestamp(),
            )
            .await
            .unwrap();
    }

    async fn export_words(&self, campaign: Option<i64>, window: &str) -> String {
        export_words(&self.f, &self.workspace, AD, campaign, window).await
    }
}

#[tokio::test]
#[serial]
async fn the_export_has_the_update_the_campaigns_and_the_earlier_updates_of_the_ad_account_page() {
    let h = Harbour::new(true).await;
    h.sql(
        "UPDATE updates SET data=json_set(data,'$.lines[0].text','42 conversions, up from 28.') WHERE workspace_id=? AND day=?",
        vec![
            h.workspace.workspace.id().into(),
            day_number(today()).into(),
        ],
    )
    .await;
    let words = h.export_words(None, "7").await;
    for expected in [
        "Monday 12 October\nSpend was steady at $2,765, and conversions rose from 84 to 98. Most of the gain came from Search - Emergency.\n",
        "Search - Emergency: 42 conversions, up from 28.\n",
        "An Update always compares the last 7 days with the 7 days before.\n",
        "Campaign\n30 days\nSpend\nConversions\nCost per conversion\nClicks\nCost per click\nImpressions\n",
        "Search - Brand\nstretch\n$560\nwas $560\n42\nwas 42\n$13.33\nwas $13.33\n280\nwas 280\n$2.00\nwas $2.00\n2,800\nwas 2,800\n",
        "Earlier Updates\nbold\nFriday 9 October\nDisplay - Remarketing spent $45 a day, as usual.\n",
    ] {
        assert!(words.contains(expected), "{expected}\n{words}");
    }
    for label in ["Finding", "Impact", "Evidence"] {
        assert!(words.contains(&format!("\n{label}\n")), "{label}\n{words}");
    }
    assert!(
        words.contains("dental implants jobs wastes spend\nPeople who search for jobs are not patients.\nAdd dental implants jobs as a negative keyword in Search - Implants.\ndental implants jobs: $186 spend, 74 clicks, 0 conversions in the last 30 days.\n"),
        "{words}"
    );
    for trend in [
        "Spend per day",
        "Conversions per day",
        "Cost per conversion per day",
        "Clicks per day",
    ] {
        assert!(words.contains(trend), "{trend}\n{words}");
    }
    assert!(
        words.find("Update\n").unwrap() < words.find("Metrics\n").unwrap()
            && words.find("Metrics\n").unwrap() < words.find("Recommendations\n").unwrap()
            && words.find("Recommendations\n").unwrap() < words.find("Campaigns\n").unwrap()
            && words.find("Campaigns\n").unwrap() < words.find("Charts\n").unwrap()
            && words.find("Charts\n").unwrap() < words.find("Earlier Updates\n").unwrap(),
        "{words}"
    );
}

#[tokio::test]
#[serial]
async fn the_export_shows_earlier_quiet_days_in_a_row_as_one_nothing_notable_changed() {
    let h = Harbour::new(true).await;
    let today = day_number(today());
    for day in [today - 1, today - 2] {
        h.sql(
            "INSERT INTO updates(workspace_id,ad_account_customer_id,day,data) SELECT workspace_id,ad_account_customer_id,?2,json_set(data,'$.day',?2,'$.summary','Nothing notable changed in the last 7 days.','$.lines',json('[]'),'$.detection.movements',json('[]'),'$.detection.yesterday',json('[]'),'$.detection.campaigns',json('[]')) FROM updates WHERE workspace_id=?1 AND day=?3 ON CONFLICT(workspace_id,ad_account_customer_id,day) DO UPDATE SET data=excluded.data",
            vec![
                h.workspace.workspace.id().into(),
                day.into(),
                today.into(),
            ],
        )
        .await;
    }
    let words = h.export_words(None, "7").await;
    assert!(
        words.contains("Earlier Updates\nbold\n10 to 11 October\nNothing notable changed.\n"),
        "{words}"
    );
    let earlier = words.split("Earlier Updates\n").nth(1).unwrap();
    assert!(!earlier.contains("in the last 7 days."), "{earlier}");
}

#[tokio::test]
#[serial]
async fn the_export_lists_each_account_change_with_its_follow_up_and_states_the_tracking_notice() {
    let h = Harbour::new(false).await;
    h.sql(
        &format!("INSERT INTO account_changes(workspace_id,ad_account_customer_id,changed_at,resource_name,resource_type,operation,campaign_id,changed_fields,old_value,new_value) SELECT workspace_id,customer_id,'2026-10-08 10:00:00','customers/{AD}/changeEvents/1','CAMPAIGN_BUDGET','UPDATE',701,'','{{\"campaignBudget\":{{\"amountMicros\":\"50000000\"}}}}','{{\"campaignBudget\":{{\"amountMicros\":\"80000000\"}}}}' FROM ad_accounts WHERE workspace_id=?"),
        vec![h.workspace.workspace.id().into()],
    )
    .await;
    let words = h.export_words(None, "7").await;
    for expected in [
        "We do not judge this Ad Account by conversions\nThe Settings of this Ad Account turn off judging by conversions, so results cannot be judged. We show spend and clicks only.\n",
        "Account changes in the last 8 weeks\n8 October: Search - Brand: Daily budget changed from $50.00 to $80.00.\nKeySend checks the result on 24 October.\n",
    ] {
        assert!(words.contains(expected), "{expected}\n{words}");
    }
    assert!(!words.contains("Conversions"), "{words}");
    assert_eq!(words.matches("cannot be judged").count(), 1, "{words}");
    assert!(!words.contains("Results cannot be judged."), "{words}");

    let campaign = h.export_words(Some(701), "7").await;
    assert!(
        campaign.contains("8 October: Daily budget changed from $50.00 to $80.00.\nKeySend checks the result on 24 October.\n"),
        "{campaign}"
    );
}

#[tokio::test]
#[serial]
async fn the_export_states_the_cause_of_the_update_with_its_level_under_the_summary() {
    let h = Harbour::new(true).await;
    for (level, label, evidence) in [
        (
            "likely",
            "Likely cause",
            "Daily budget changed from $50.00 to $80.00 on 7 October.",
        ),
        (
            "contribution",
            "Contribution",
            "Mobile accounts for 70% of the rise in Spend: $70.00 of $100.00.",
        ),
    ] {
        h.sql(
            "UPDATE updates SET data=json_set(data,'$.cause',json(?)) WHERE workspace_id=?",
            vec![
                json!({"level":level,"evidence":evidence})
                    .to_string()
                    .into(),
                h.workspace.workspace.id().into(),
            ],
        )
        .await;
        let words = h.export_words(None, "7").await;
        assert!(
            words.contains(&format!(
                "Most of the gain came from Search - Emergency.\n{label}: {evidence}\n"
            )),
            "{words}"
        );
    }
}

#[tokio::test]
#[serial]
async fn the_export_ends_on_the_last_collected_day_like_the_page() {
    let h = Harbour::new(true).await;
    let behind = day_number(end()) - 2;
    h.sql(
        "DELETE FROM campaign_days WHERE workspace_id=? AND day>?",
        vec![h.workspace.workspace.id().into(), behind.into()],
    )
    .await;
    h.sql(
        "UPDATE account_history SET last_day=? WHERE workspace_id=?",
        vec![behind.into(), h.workspace.workspace.id().into()],
    )
    .await;
    let page = h.f.page(&format!("/ad-accounts/{AD}")).await;
    assert!(page.contains("3 to 9 October"), "{page}");

    let words = h.export_words(None, "7").await;
    assert!(
        words.contains("Last 7 days, 3 to 9 October 2026. Compared with 26 September to 2 October 2026. Prepared on 12 October 2026."),
        "{words}"
    );
}

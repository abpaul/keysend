use super::*;
use chrono::{Days, NaiveDate};
use keysend::monitoring::campaign_settings::AllFollowAccount;
use keysend::{
    monitoring::{
        audit::{Audit, Counts, Data},
        findings,
    },
    monitoring::{
        deep_check::{DeepCheck, Finding},
        detection,
        metrics::Window,
        tracking::ConversionTracking,
        update,
        worker::{self, EmailJob, MonitoringJob},
    },
    settings::Settings,
    workspace::{
        history::{day_number, Campaign, CampaignDay},
        monitoring::{
            AccountSettings, Breakdown, BreakdownDays, Recommendation, RecommendationState,
        },
    },
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

const AD: &str = "4829153307";
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
}
fn end() -> NaiveDate {
    today().pred_opt().unwrap()
}
fn oldest() -> NaiveDate {
    NaiveDate::from_ymd_opt(2024, 10, 12).unwrap()
}
fn campaign() -> Campaign {
    Campaign {
        id: 701,
        name: "Search - Real".into(),
        status: "ENABLED".into(),
        channel_type: "SEARCH".into(),
        bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
        budget_micros: Some(80_000_000),
    }
}
fn google_days(conversions: f64) -> Vec<Value> {
    (0..=(end()-oldest()).num_days()).map(|offset| {
        let date=oldest().checked_add_days(Days::new(offset as u64)).unwrap();
        json!({"campaign":{"id":"701"},"segments":{"date":date.to_string(),"device":"MOBILE"},"metrics":{"impressions":"100","clicks":"10","costMicros":"10000000","conversions":conversions,"conversionsValue":conversions*5.0}})
    }).collect()
}
fn data() -> Data {
    Data {
        currency: "EUR".into(),
        reporting: Counts {
            impressions: 3000.0,
            clicks: 300.0,
            cost: 300.0,
            conversions: 30.0,
            value: 150.0,
        },
        audit: Audit {
            conversion_actions: Some(1),
            ..Default::default()
        },
    }
}
fn check(day: i64) -> DeepCheck {
    DeepCheck {
        day,
        data: data(),
        by_campaign: Default::default(),
        findings: vec![],
        judge_by_conversions: true,
        recommendations_generated: true,
        assessment: None,
        research: None,
    }
}
async fn signed(f: &Fixture) -> identity::SignedIn {
    identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap()
}

impl Fixture {
    pub(super) async fn monitoring() -> Self {
        Self::monitoring_limit(15_000).await
    }

    pub(super) async fn monitoring_limit(daily_limit: u32) -> Self {
        let f = Self::configured_clock(
            false,
            false,
            true,
            "google",
            daily_limit,
            "2026-10-12T12:00:00Z",
        )
        .await;
        {
            let mut calls = f.calls.lock().unwrap();
            calls.google.monitoring_days = Some(google_days(1.0));
            calls.google.monitoring_zone = Some("UTC".into());
        }
        let signed = signed(&f).await;
        f.ctx
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
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
                    signed.workspace.id().into(),
                ],
            ))
            .await
            .unwrap();
        f
    }

    async fn prepare_monitoring(&self) -> MonitoringJob {
        let signed = signed(self).await;
        let store = WorkspaceStore::new(&self.ctx.db, &signed.workspace);
        store
            .save_ad_account_metadata(AD, "EUR", "UTC")
            .await
            .unwrap();
        let days = (0..63)
            .map(|offset| CampaignDay {
                day: end().checked_sub_days(Days::new(offset)).unwrap(),
                campaign_id: 701,
                device: 1,
                counts: Counts {
                    impressions: 100.0,
                    clicks: 10.0,
                    cost: 10.0,
                    conversions: 1.0,
                    value: 5.0,
                },
            })
            .collect::<Vec<_>>();
        store
            .save_history(AD, oldest(), end(), &[campaign()], &days, &[])
            .await
            .unwrap();
        store
            .refresh_deep_check(AD, &check(day_number(today()) - 1), &[])
            .await
            .unwrap();
        for days in [
            BreakdownDays::SearchShare(vec![]),
            BreakdownDays::ConversionActions(vec![]),
            BreakdownDays::Networks(vec![]),
        ] {
            store
                .save_breakdown(AD, day_number(oldest()), day_number(end()), days)
                .await
                .unwrap();
        }
        MonitoringJob {
            workspace_id: signed.workspace.id().into(),
            ad_account: AD.into(),
            immediate: false,
        }
    }
}

#[tokio::test]
#[serial]
async fn a_quiet_daily_update_uses_six_google_requests_and_no_ai() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let updates = store
        .updates(AD, day_number(today()), day_number(today()))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].summary, update::QUIET);
    assert!(!store
        .claim_monitoring_run(
            AD,
            day_number(today()),
            Settings::load(&f.ctx).unwrap().now().timestamp() + 601
        )
        .await
        .unwrap());
    assert!(updates[0].lines.is_empty());
    {
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.google.requests.len(), 6);
        assert_eq!(calls.google.refresh.len(), 1);
        assert!(calls.commentary.is_empty());
        assert_eq!(calls.emails.len(), 1);
        assert_eq!(
            calls.emails[0]["subject"],
            "Your week in Google Ads: Northwind Dental"
        );
        assert!(calls.emails[0]["text"]
            .as_str()
            .unwrap()
            .contains("Spend: EUR 70.00 (no change)\nConversions: 7 (no change)"));
        assert!(calls.emails[0]["html"]
            .as_str()
            .unwrap()
            .contains("Open Northwind Dental"));
    }
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(f.calls.lock().unwrap().google.requests.len(), 6);
    assert_eq!(f.calls.lock().unwrap().emails.len(), 1);
}

#[tokio::test]
#[serial]
async fn a_campaign_movement_uses_one_ai_request_and_stores_an_alert() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    for row in f
        .calls
        .lock()
        .unwrap()
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
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let updates = store
        .updates(AD, day_number(today()), day_number(today()))
        .await
        .unwrap();
    assert_eq!(
        updates[0].summary,
        "In the last 7 days, spend was EUR 140 against EUR 70.00."
    );
    assert!(!updates[0].written_without_ai);
    assert_eq!(updates[0].lines[0].campaign_id, 701);
    assert_eq!(updates[0].metrics.current.spend, 140.0);
    let alerts = store
        .alerts(AD, day_number(end()) - 2, day_number(end()))
        .await
        .unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(
        alerts[0].event_key,
        keysend::monitoring::alerts::EventKey::SpendDoubled
    );
    assert!(alerts[0].body.contains("EUR 20.00"));
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.commentary.len(), 1);
    assert_eq!(
        calls.commentary[0]["text"]["format"]["name"],
        "monitoring_update"
    );
    assert_eq!(calls.google.requests.len(), 6);
    assert!(calls
        .emails
        .iter()
        .any(|email| email["subject"] == "Alert: Spend doubled in Northwind Dental"));
}

#[tokio::test]
#[serial]
async fn an_invented_ai_number_is_rejected_but_an_alert_still_reaches_email() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    {
        let mut calls = f.calls.lock().unwrap();
        calls.invented = true;
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
    }
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let updates = store.updates(AD, 0, i64::MAX).await.unwrap();
    assert_eq!(
        updates[0].summary,
        "Spend was EUR 140 against EUR 70.00 in the previous 7 days. Yesterday spend was EUR 20.00, against a usual EUR 10.00 for that weekday."
    );
    assert!(updates[0].written_without_ai);
    assert!(updates[0].lines[0].text.contains("EUR 140"));
    assert_eq!(store.alerts(AD, 0, i64::MAX).await.unwrap().len(), 1);
    assert_eq!(f.calls.lock().unwrap().commentary.len(), 1);
    assert_eq!(
        f.calls.lock().unwrap().emails[0]["subject"],
        "Alert: Spend doubled in Northwind Dental"
    );
    f.calls.lock().unwrap().invented = false;
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(store.updates(AD, 0, i64::MAX).await.unwrap().len(), 1);
    assert_eq!(store.alerts(AD, 0, i64::MAX).await.unwrap().len(), 1);
}

#[tokio::test]
#[serial]
async fn an_unlisted_campaign_in_the_ai_answer_is_rejected() {
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
        calls.override_monitoring = Some(
            json!({"summary":"Spend changed.","lines":[{"campaign_id":999,"text":"Spend changed."}]}),
        );
    }
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let updates = WorkspaceStore::new(&f.ctx.db, &signed.workspace)
        .updates(AD, 0, i64::MAX)
        .await
        .unwrap();
    assert!(updates[0].written_without_ai);
    assert_eq!(updates[0].lines[0].campaign_id, 701);
    assert_eq!(f.calls.lock().unwrap().commentary.len(), 1);
}

#[tokio::test]
#[serial]
async fn first_session_stores_sixty_days_then_older_history_then_a_deep_check_and_update() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let job = MonitoringJob {
        workspace_id: signed.workspace.id().into(),
        ad_account: AD.into(),
        immediate: false,
    };
    keysend::monitoring::schedule::collect_history(&f.ctx, &signed.workspace)
        .await
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let history = store.history_state(AD).await.unwrap().unwrap();
    assert_eq!(history.first_day, Some(oldest()));
    assert_eq!(history.last_day, Some(end()));
    assert_eq!(
        store
            .window_metrics(AD, day_number(end()), Window::ThirtyDays, None)
            .await
            .unwrap()
            .current
            .spend,
        300.0
    );
    assert!(store.latest_deep_check(AD).await.unwrap().is_some());
    assert_eq!(
        store.updates(AD, 0, i64::MAX).await.unwrap()[0].summary,
        update::QUIET
    );
    {
        let calls = f.calls.lock().unwrap();
        let queries = calls
            .google
            .requests
            .iter()
            .map(|(_, body)| body["query"].as_str().unwrap())
            .collect::<Vec<_>>();
        let history_queries = queries
            .iter()
            .filter(|query| query.contains("segments.date, segments.device"))
            .copied()
            .collect::<Vec<_>>();
        let from = end().checked_sub_days(Days::new(59)).unwrap();
        assert!(history_queries[0].contains(&format!("BETWEEN '{from}' AND '{}'", end())));
        let last_history = queries
            .iter()
            .rposition(|query| query.contains("segments.date, segments.device"))
            .unwrap();
        let first_deep = queries
            .iter()
            .position(|query| query.contains("FROM conversion_action"))
            .unwrap();
        assert!(last_history < first_deep);
        assert_eq!(
            calls
                .commentary
                .iter()
                .map(|call| call["text"]["format"]["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["monitoring_review"]
        );
    }
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(store.updates(AD, 0, i64::MAX).await.unwrap().len(), 1);
}

#[tokio::test]
#[serial]
async fn first_sixty_days_remain_available_when_older_history_collection_fails() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.calls.lock().unwrap().google.failed_query = Some("AND '2026-08-12'");
    assert!(
        keysend::google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, AD)
            .await
            .is_err()
    );
    assert_eq!(
        store.history_state(AD).await.unwrap().unwrap().first_day,
        Some(end().checked_sub_days(Days::new(59)).unwrap())
    );
    assert_eq!(
        store
            .window_metrics(AD, day_number(end()), Window::ThirtyDays, None)
            .await
            .unwrap()
            .previous
            .spend,
        300.0
    );
    f.calls.lock().unwrap().google.failed_query = None;
    keysend::google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, AD)
        .await
        .unwrap();
    assert_eq!(
        store.history_state(AD).await.unwrap().unwrap().first_day,
        Some(oldest())
    );
}

#[tokio::test]
#[serial]
async fn updates_metrics_settings_findings_and_feedback_are_workspace_scoped() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let other = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    let a = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let b = WorkspaceStore::new(&f.ctx.db, &other.workspace);
    assert!(b.updates(AD, 0, i64::MAX).await.unwrap().is_empty());
    assert!(b.monitoring_days(AD, 0, i64::MAX).await.unwrap().is_empty());
    assert!(b.latest_deep_check(AD).await.unwrap().is_none());
    assert!(!b
        .mark_update_not_useful(AD, day_number(today()))
        .await
        .unwrap());
    b.save_monitoring_settings(
        AD,
        &AccountSettings {
            judge_by_conversions: Some(false),
            weekly_email: false,
            alerts: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        a.monitoring_settings(AD).await.unwrap(),
        AccountSettings::default()
    );
    assert!(a
        .mark_update_not_useful(AD, day_number(today()))
        .await
        .unwrap());
    assert!(a.updates(AD, 0, i64::MAX).await.unwrap()[0].not_useful);
    assert!(b.monitoring_findings(AD).await.unwrap().is_empty());
    assert!(b.monitoring_recommendations(AD).await.unwrap().is_empty());
    assert!(b.weekly_email(day_number(today())).await.unwrap().is_none());
}

#[tokio::test]
#[serial]
async fn account_settings_hide_conversion_metrics_and_turn_weekly_email_off() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: Some(false),
                weekly_email: false,
                alerts: true,
            },
        )
        .await
        .unwrap();
    worker::run(&f.ctx, &job).await.unwrap();
    let window = store
        .window_metrics(AD, day_number(end()), Window::SevenDays, None)
        .await
        .unwrap();
    assert_eq!(window.current.conversions, None);
    let campaign = store
        .window_metrics(AD, day_number(end()), Window::SevenDays, Some(701))
        .await
        .unwrap();
    assert_eq!(campaign.current.spend, 70.0);
    assert_eq!(campaign.current.conversions, None);
    let update = store.updates(AD, 0, i64::MAX).await.unwrap().remove(0);
    assert!(!update.summary.contains("conversions"));
    assert!(f.calls.lock().unwrap().commentary.is_empty());
    assert!(f.calls.lock().unwrap().emails.is_empty());
    assert!(store
        .weekly_email(day_number(today()))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[serial]
async fn an_ad_account_not_judged_by_conversions_states_the_reason_once_and_not_in_each_update() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    WorkspaceStore::new(&f.ctx.db, &signed.workspace)
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: Some(false),
                weekly_email: true,
                alerts: true,
            },
        )
        .await
        .unwrap();
    worker::run(&f.ctx, &job).await.unwrap();
    let page = ad_account_page(&f).await;
    assert_eq!(
        page.matches("The Settings of this Ad Account turn off judging by conversions, so results cannot be judged. We show spend and clicks only.")
            .count(),
        1
    );
    assert!(!page.contains("Results cannot be judged."));
}

#[tokio::test]
#[serial]
async fn no_conversions_and_absent_conversion_actions_have_distinct_tracking_states() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.calls.lock().unwrap().google.monitoring_days = Some(google_days(0.0));
    keysend::google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, AD)
        .await
        .unwrap();
    assert_eq!(
        store
            .conversion_tracking(AD, day_number(end()))
            .await
            .unwrap(),
        (ConversionTracking::TooFewResults, false)
    );
    let mut check = check(day_number(today()));
    check.data.audit.conversion_actions = Some(0);
    store.refresh_deep_check(AD, &check, &[]).await.unwrap();
    assert_eq!(
        store
            .conversion_tracking(AD, day_number(end()))
            .await
            .unwrap(),
        (ConversionTracking::NotSetUp, false)
    );
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: Some(true),
                weekly_email: true,
                alerts: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .conversion_tracking(AD, day_number(end()))
            .await
            .unwrap(),
        (ConversionTracking::NotSetUp, true)
    );
}

fn standing_finding() -> Finding {
    Finding {
        campaign_id: Some(701),
        rule: findings::Finding {
            key: "campaign-cost-high:701".into(),
            title: "Search - Real cost is high".into(),
            detail: "Search - Real spent EUR 600 for 1 conversion.".into(),
            subjects: vec!["Search - Real".into()],
            spend: 600.0,
        },
    }
}
fn standing_recommendation() -> Recommendation {
    Recommendation {
        finding_key: "campaign-cost-high:701".into(),
        action: "Review Search - Real.".into(),
        reason: "Its cost is high.".into(),
        evidence: "Search - Real spent EUR 600 for 1 conversion.".into(),
        measure: String::new(),
        intent: String::new(),
        subject: String::new(),
        confidence: Default::default(),
        campaign_id: Some(701),
        state: RecommendationState::Open,
    }
}

#[tokio::test]
#[serial]
async fn closed_recommendations_stay_closed_for_ninety_days_across_absence() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let other = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let other = WorkspaceStore::new(&f.ctx.db, &other.workspace);
    let mut check = check(day_number(today()));
    check.findings.push(standing_finding());
    for state in [RecommendationState::Done, RecommendationState::NotUseful] {
        store
            .refresh_deep_check(AD, &check, &[standing_recommendation()])
            .await
            .unwrap();
        assert!(!other
            .close_recommendation(AD, "campaign-cost-high:701", state.clone(), check.day)
            .await
            .unwrap());
        assert!(store
            .close_recommendation(AD, "campaign-cost-high:701", state.clone(), check.day)
            .await
            .unwrap());
        check.day += 7;
        store
            .refresh_deep_check(AD, &check, &[standing_recommendation()])
            .await
            .unwrap();
        assert_eq!(
            store.monitoring_recommendations(AD).await.unwrap()[0].state,
            state
        );
        check.day += 7;
        check.findings.clear();
        store.refresh_deep_check(AD, &check, &[]).await.unwrap();
        assert_eq!(
            store.monitoring_recommendations(AD).await.unwrap()[0].state,
            state
        );
        assert!(store.monitoring_findings(AD).await.unwrap().is_empty());
        check.day += 7;
        check.findings.push(standing_finding());
        store
            .refresh_deep_check(AD, &check, &[standing_recommendation()])
            .await
            .unwrap();
        assert_eq!(
            store.monitoring_recommendations(AD).await.unwrap()[0].state,
            state
        );
        check.day += 69;
        store
            .refresh_deep_check(AD, &check, &[standing_recommendation()])
            .await
            .unwrap();
        assert_eq!(
            store.monitoring_recommendations(AD).await.unwrap()[0].state,
            RecommendationState::Open
        );
    }
}

#[tokio::test]
#[serial]
async fn weekly_deep_check_refreshes_findings_and_validated_recommendations() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "DELETE FROM deep_checks WHERE workspace_id=? AND ad_account_customer_id=?",
            vec![signed.workspace.id().into(), AD.into()],
        ))
        .await
        .unwrap();
    store
        .refresh_deep_check(AD, &check(day_number(today()) - 8), &[])
        .await
        .unwrap();
    {
        let mut calls = f.calls.lock().unwrap();
        for row in calls
            .google
            .monitoring_days
            .as_mut()
            .unwrap()
            .iter_mut()
            .rev()
            .take(30)
        {
            row["metrics"]["conversions"] = json!(0.0);
        }
    }
    worker::run(&f.ctx, &job).await.unwrap();
    let check = store.latest_deep_check(AD).await.unwrap().unwrap();
    assert_eq!(check.day, day_number(today()));
    assert!(check
        .findings
        .iter()
        .any(|finding| finding.rule.key == "tracking-no-results"));
    let recommendations = store.monitoring_recommendations(AD).await.unwrap();
    assert_eq!(
        recommendations[0].action,
        "Check that the conversion actions in Google Ads still record conversions."
    );
    assert!(f
        .calls
        .lock()
        .unwrap()
        .commentary
        .iter()
        .all(|call| call["text"]["format"]["name"] == "monitoring_update"));
}

#[tokio::test]
#[serial]
async fn failed_email_delivery_can_retry_without_duplicate_stored_events() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    f.calls.lock().unwrap().email_refused = true;
    assert!(worker::run(&f.ctx, &job).await.is_err());
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(store.pending_weekly_emails().await.unwrap().len(), 1);
    f.calls.lock().unwrap().email_refused = false;
    worker::send(
        &f.ctx,
        &EmailJob {
            workspace_id: job.workspace_id,
        },
    )
    .await
    .unwrap();
    assert!(store.pending_weekly_emails().await.unwrap().is_empty());
    assert_eq!(f.calls.lock().unwrap().emails.len(), 2);
    assert_eq!(f.calls.lock().unwrap().google.requests.len(), 6);
}

#[tokio::test]
#[serial]
async fn a_workspace_keeps_access_for_its_trial_and_subscription_with_up_to_five_ad_accounts() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let start = store.trial_started_at().await.unwrap();
    assert!(store
        .monitoring_access(start + 14 * 86_400 - 1)
        .await
        .unwrap());
    assert!(!store.monitoring_access(start + 14 * 86_400).await.unwrap());
    f.ctx.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,"INSERT INTO subscriptions(workspace_id,stripe_subscription_id,created_at,status,period_start,period_end) VALUES(?,'sub_monitoring',0,'active',0,?)",vec![signed.workspace.id().into(),(start+30*86_400).into()])).await.unwrap();
    assert!(store
        .monitoring_can_select_accounts(
            &[
                "1111111111".into(),
                "2222222222".into(),
                "3333333333".into(),
                "4444444444".into()
            ],
            start + 15 * 86_400
        )
        .await
        .unwrap());
    assert!(!store
        .monitoring_can_select_accounts(
            &[
                "1111111111".into(),
                "2222222222".into(),
                "3333333333".into(),
                "4444444444".into(),
                "5555555555".into()
            ],
            start + 15 * 86_400
        )
        .await
        .unwrap());
}

#[tokio::test]
#[serial]
async fn campaign_day_queries_use_the_workspace_account_day_primary_key() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let rows=f.ctx.db.query_all_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,"EXPLAIN QUERY PLAN SELECT SUM(cost_micros) FROM campaign_days WHERE workspace_id=? AND ad_account_customer_id=? AND day BETWEEN ? AND ?",vec![job.workspace_id.into(),AD.into(),(day_number(end())-59).into(),day_number(end()).into()])).await.unwrap();
    let plan = rows
        .into_iter()
        .map(|row| row.try_get::<String>("", "detail").unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        plan.contains("SEARCH campaign_days USING PRIMARY KEY"),
        "{plan}"
    );
    assert!(
        plan.contains("workspace_id=? AND ad_account_customer_id=? AND day>? AND day<?"),
        "{plan}"
    );
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let days = store
        .monitoring_days(AD, day_number(end()) - 62, day_number(end()))
        .await
        .unwrap();
    assert!(detection::detect(
        &days,
        &store.campaigns(AD).await.unwrap(),
        &[],
        day_number(end()),
        day_number(oldest()),
        AllFollowAccount { judged: true },
        "EUR"
    )
    .quiet());
}

#[tokio::test]
#[serial]
async fn standing_recommendations_use_the_ai_stub_and_reject_invented_evidence() {
    let f = Fixture::monitoring().await;
    let mut check = check(day_number(today()));
    check.findings.push(standing_finding());
    let recommendations = keysend::monitoring::deep_check::review(
        &Settings::load(&f.ctx).unwrap(),
        &check,
        &Default::default(),
        AllFollowAccount { judged: true },
    )
    .await
    .unwrap()
    .recommendations;
    assert_eq!(recommendations.len(), 1);
    assert_eq!(recommendations[0].action, "Review Search - Real.");
    assert_eq!(recommendations[0].campaign_id, Some(701));
    assert_eq!(
        recommendations[0].evidence,
        "Search - Real spent EUR 600 for 1 conversion."
    );
    f.calls.lock().unwrap().override_monitoring = Some(
        json!({"recommendations":[{"finding":"campaign-cost-high:701","intent":"investigate","action":"Review Search - Real.","reason":"Spend was 999999."}]}),
    );
    assert!(keysend::monitoring::deep_check::review(
        &Settings::load(&f.ctx).unwrap(),
        &check,
        &Default::default(),
        AllFollowAccount { judged: true },
    )
    .await
    .unwrap()
    .recommendations
    .is_empty());
    assert_eq!(f.calls.lock().unwrap().commentary.len(), 2);
}

#[tokio::test]
#[serial]
async fn a_weekly_email_opt_out_cancels_a_previously_queued_email() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    f.calls.lock().unwrap().email_refused = true;
    assert!(worker::run(&f.ctx, &job).await.is_err());
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: None,
                weekly_email: false,
                alerts: true,
            },
        )
        .await
        .unwrap();
    f.calls.lock().unwrap().email_refused = false;
    worker::send(
        &f.ctx,
        &EmailJob {
            workspace_id: job.workspace_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(f.calls.lock().unwrap().emails.len(), 1);
    assert!(store.pending_weekly_emails().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn monitoring_backfills_a_gap_after_more_than_thirty_days_of_downtime() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let old_end = end().checked_sub_days(Days::new(90)).unwrap();
    store
        .save_history(AD, oldest(), old_end, &[campaign()], &[], &[])
        .await
        .unwrap();
    keysend::google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, AD)
        .await
        .unwrap();
    assert_eq!(
        store
            .window_metrics(AD, day_number(end()), Window::ThirtyDays, None)
            .await
            .unwrap()
            .previous
            .spend,
        300.0
    );
    assert_eq!(
        store
            .monitoring_days(AD, day_number(old_end) + 1, day_number(end()) - 60)
            .await
            .unwrap()
            .len(),
        30
    );
    assert_eq!(
        store.history_state(AD).await.unwrap().unwrap().first_day,
        Some(oldest())
    );
}

#[tokio::test]
#[serial]
async fn removing_an_ad_account_cascades_monitoring_rows() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut check = check(day_number(today()));
    check.findings.push(standing_finding());
    store
        .refresh_deep_check(AD, &check, &[standing_recommendation()])
        .await
        .unwrap();
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: Some(false),
                weekly_email: false,
                alerts: true,
            },
        )
        .await
        .unwrap();
    store.remove_ad_account(AD).await.unwrap();
    assert!(store.updates(AD, 0, i64::MAX).await.unwrap().is_empty());
    assert!(store.latest_deep_check(AD).await.unwrap().is_none());
    assert!(store.monitoring_findings(AD).await.unwrap().is_empty());
    assert!(store
        .monitoring_recommendations(AD)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.monitoring_settings(AD).await.unwrap(),
        AccountSettings::default()
    );
    assert!(store.alerts(AD, 0, i64::MAX).await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn a_failed_recommendation_request_keeps_findings_and_the_daily_update_and_can_retry() {
    let mut f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut check = check(day_number(today()));
    check.recommendations_generated = false;
    check.findings.push(standing_finding());
    store.refresh_deep_check(AD, &check, &[]).await.unwrap();
    f.calls.lock().unwrap().override_recommendations =
        Some(json!({"recommendations":"invalid response"}));
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(
        store.monitoring_findings(AD).await.unwrap()[0].rule.key,
        "campaign-cost-high:701"
    );
    assert_eq!(
        store.updates(AD, 0, i64::MAX).await.unwrap()[0].summary,
        update::QUIET
    );
    assert!(
        !store
            .latest_deep_check(AD)
            .await
            .unwrap()
            .unwrap()
            .recommendations_generated
    );
    f.calls.lock().unwrap().override_recommendations = None;
    f.ctx.config.settings.as_mut().unwrap()["services"]["test_now"] = json!("2026-10-13T12:00:00Z");
    {
        let mut calls = f.calls.lock().unwrap();
        let rows = calls.google.monitoring_days.as_mut().unwrap();
        let mut row = rows.last().unwrap().clone();
        row["segments"]["date"] = json!("2026-10-12");
        rows.push(row);
    }
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(
        store.monitoring_recommendations(AD).await.unwrap()[0].action,
        "Review Search - Real."
    );
    assert!(
        store
            .latest_deep_check(AD)
            .await
            .unwrap()
            .unwrap()
            .recommendations_generated
    );
    assert_eq!(store.updates(AD, 0, i64::MAX).await.unwrap().len(), 2);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.commentary.len(), 2);
    assert!(calls
        .commentary
        .iter()
        .all(|call| call["text"]["format"]["name"] == "monitoring_review"));
    assert!(calls.google.requests.iter().all(|(_, body)| !body["query"]
        .as_str()
        .unwrap()
        .contains("FROM conversion_action")));
}

#[tokio::test]
#[serial]
async fn a_workspace_trial_starts_at_creation_and_does_not_restart_on_sign_in() {
    let f = Fixture::new(false).await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let started = store.trial_started_at().await.unwrap();
    assert_eq!(started, Settings::load(&f.ctx).unwrap().now().timestamp());
    identity::sign_in(
        &f.ctx.db,
        "a",
        "a@example.com",
        "User A",
        started + 86400,
        None,
    )
    .await
    .unwrap();
    assert_eq!(store.trial_started_at().await.unwrap(), started);
}

#[tokio::test]
#[serial]
async fn monitoring_waits_until_four_in_the_account_time_zone() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let workspace = signed.workspace;
    let store = WorkspaceStore::new(&f.ctx.db, &workspace);
    store
        .save_ad_account_metadata(AD, "EUR", "Pacific/Honolulu")
        .await
        .unwrap();
    f.calls.lock().unwrap().google.monitoring_zone = Some("Pacific/Honolulu".into());
    worker::run(&f.ctx, &job).await.unwrap();
    assert!(store.updates(AD, 0, i64::MAX).await.unwrap().is_empty());
    assert!(f.calls.lock().unwrap().google.requests.is_empty());
}

#[tokio::test]
#[serial]
async fn deep_check_failure_leaves_daily_output_and_retries_next_day() {
    let mut f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let workspace = signed.workspace;
    let store = WorkspaceStore::new(&f.ctx.db, &workspace);
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "DELETE FROM deep_checks WHERE workspace_id=? AND ad_account_customer_id=?",
            vec![workspace.id().into(), AD.into()],
        ))
        .await
        .unwrap();
    store
        .refresh_deep_check(AD, &check(day_number(today()) - 8), &[])
        .await
        .unwrap();
    f.calls.lock().unwrap().google.failed_query = Some("FROM keyword_view");
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(
        store.updates(AD, 0, i64::MAX).await.unwrap()[0].summary,
        update::QUIET
    );
    let failure=f.ctx.db.query_one_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,"SELECT day,error FROM deep_check_failures WHERE workspace_id=? AND ad_account_customer_id=?",vec![workspace.id().into(),AD.into()])).await.unwrap().unwrap();
    assert_eq!(
        failure.try_get::<i64>("", "day").unwrap(),
        day_number(today())
    );
    assert!(!failure.try_get::<String>("", "error").unwrap().is_empty());
    let requests = f.calls.lock().unwrap().google.requests.len();
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(f.calls.lock().unwrap().google.requests.len(), requests);
    f.calls.lock().unwrap().google.failed_query = None;
    f.ctx.config.settings.as_mut().unwrap()["services"]["test_now"] = json!("2026-10-13T12:00:00Z");
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(
        store.latest_deep_check(AD).await.unwrap().unwrap().day,
        day_number(today()) + 1
    );
    let calls = f.calls.lock().unwrap();
    for (_, body) in &calls.google.requests {
        let query = body["query"].as_str().unwrap();
        for (resource, limit) in [
            ("search_term_view", 200),
            ("keyword_view", 200),
            ("landing_page_view", 50),
            ("ad_group_ad", 50),
        ] {
            if query.contains(&format!("FROM {resource}")) {
                assert!(
                    query.ends_with(&format!("ORDER BY metrics.cost_micros DESC LIMIT {limit}")),
                    "{query}"
                );
            }
        }
    }
}

#[tokio::test]
#[serial]
async fn weekly_email_skips_unready_accounts_and_waits_five_days() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let workspace = signed.workspace;
    let store = WorkspaceStore::new(&f.ctx.db, &workspace);
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started_at=? WHERE workspace_id=?",
            vec![
                Settings::load(&f.ctx).unwrap().now().timestamp().into(),
                workspace.id().into(),
            ],
        ))
        .await
        .unwrap();
    let detected = detection::detect(
        &store
            .monitoring_days(AD, day_number(end()) - 62, day_number(end()))
            .await
            .unwrap(),
        &[campaign()],
        &[],
        day_number(end()),
        day_number(oldest()),
        AllFollowAccount { judged: true },
        "EUR",
    );
    store
        .save_update(
            AD,
            &update::generate(
                &Settings::load(&f.ctx).unwrap(),
                detected,
                AllFollowAccount { judged: true },
            )
            .await
            .unwrap(),
        )
        .await
        .unwrap();
    worker::weekly_content(&Settings::load(&f.ctx).unwrap(), &store)
        .await
        .unwrap();
    assert!(store
        .weekly_email(day_number(today()))
        .await
        .unwrap()
        .is_none());
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started_at=started_at-518400 WHERE workspace_id=?",
            vec![workspace.id().into()],
        ))
        .await
        .unwrap();
    f.ctx.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,"INSERT INTO ad_accounts(id,workspace_id,customer_id,name,google_connection_id) SELECT 'unready',workspace_id,'1111111111','Unready',google_connection_id FROM ad_accounts WHERE workspace_id=? AND customer_id=?",vec![workspace.id().into(),AD.into()])).await.unwrap();
    worker::weekly_content(&Settings::load(&f.ctx).unwrap(), &store)
        .await
        .unwrap();
    assert_eq!(
        store
            .weekly_email(day_number(today()))
            .await
            .unwrap()
            .unwrap()
            .blocks
            .len(),
        1
    );
}

const CHICAGO: &str = "1111111111";

async fn weekly_with_a_later_time_zone() -> (Fixture, Settings) {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.ctx.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,"INSERT INTO ad_accounts(id,workspace_id,customer_id,name,google_connection_id) SELECT 'chicago',workspace_id,?,'Lakeshore Dental',google_connection_id FROM ad_accounts WHERE workspace_id=? AND customer_id=?",vec![CHICAGO.into(),signed.workspace.id().into(),AD.into()])).await.unwrap();
    store
        .save_ad_account_metadata(CHICAGO, "USD", "America/Chicago")
        .await
        .unwrap();
    let settings = Settings::load(&f.ctx).unwrap();
    let detected = detection::detect(
        &store
            .monitoring_days(AD, day_number(end()) - 62, day_number(end()))
            .await
            .unwrap(),
        &[campaign()],
        &[],
        day_number(end()),
        day_number(oldest()),
        AllFollowAccount { judged: true },
        "EUR",
    );
    let mut stored = update::generate(&settings, detected, AllFollowAccount { judged: true })
        .await
        .unwrap();
    store.save_update(AD, &stored).await.unwrap();
    stored.day = day_number(end());
    store.save_update(CHICAGO, &stored).await.unwrap();
    (f, settings)
}

#[tokio::test]
#[serial]
async fn the_weekly_email_waits_until_every_ad_account_has_its_update() {
    let (f, mut settings) = weekly_with_a_later_time_zone().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    for clock in ["2026-10-12T06:00:00Z", "2026-10-12T17:59:59Z"] {
        settings.services.test_now = Some(clock.parse().unwrap());
        worker::weekly_content(&settings, &store).await.unwrap();
        assert!(
            store
                .weekly_email(day_number(today()))
                .await
                .unwrap()
                .is_none(),
            "{clock}"
        );
    }
    let mut chicago = store
        .updates(CHICAGO, day_number(end()), day_number(end()))
        .await
        .unwrap()
        .remove(0);
    chicago.day = day_number(today());
    store.save_update(CHICAGO, &chicago).await.unwrap();
    settings.services.test_now = Some("2026-10-12T14:00:00Z".parse().unwrap());
    worker::weekly_content(&settings, &store).await.unwrap();
    let email = store
        .weekly_email(day_number(today()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        email
            .blocks
            .iter()
            .map(|block| block.name.as_str())
            .collect::<Vec<_>>(),
        ["Lakeshore Dental", "Northwind Dental"]
    );
    assert!(email.left_out.is_empty());
}

#[tokio::test]
#[serial]
async fn at_the_cut_off_the_weekly_email_leaves_out_an_ad_account_without_an_update_and_says_so() {
    let (f, mut settings) = weekly_with_a_later_time_zone().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    settings.services.test_now = Some("2026-10-12T18:00:00Z".parse().unwrap());
    worker::weekly_content(&settings, &store).await.unwrap();
    let email = store
        .weekly_email(day_number(today()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(email.blocks.len(), 1);
    assert_eq!(email.blocks[0].name, "Northwind Dental");
    assert_eq!(email.left_out, ["Lakeshore Dental"]);
    let message = keysend::monitoring::email::message(
        keysend::language::Language::En,
        &email,
        "http://localhost:5150",
        &keysend::translation::Translations::english(),
    )
    .unwrap();
    assert_eq!(message.subject, "Your week in Google Ads: Northwind Dental");
    let note = "Not in this email: Lakeshore Dental. Its Update was not ready when we prepared this email.";
    assert!(message.text.contains(note), "{}", message.text);
    assert!(message.html.contains(note));
    let mut linked = email;
    linked.blocks[0].name = "Northwind shop.example.com".into();
    let message = keysend::monitoring::email::message(
        keysend::language::Language::En,
        &linked,
        "http://localhost:5150",
        &keysend::translation::Translations::english(),
    )
    .unwrap();
    assert_eq!(
        message.subject,
        "Your week in Google Ads: Northwind shop[.]example[.]com"
    );
    assert!(!message.html.contains("shop.example.com"));

    linked.left_out = vec!["Lakeshore shop.example.com".into()];
    linked.blocks[0].campaigns = vec![
        keysend::monitoring::email::CampaignPart {
            name: "Search - shop.example.com".into(),
            spend: 40.0,
            results: 4.0,
        },
        keysend::monitoring::email::CampaignPart {
            name: "Search - Brand".into(),
            spend: 20.0,
            results: 3.0,
        },
    ];
    let message = keysend::monitoring::email::message(
        keysend::language::Language::En,
        &linked,
        "http://localhost:5150",
        &keysend::translation::Translations::english(),
    )
    .unwrap();
    assert!(
        message
            .text
            .contains("Not in this email: Lakeshore shop[.]example[.]com."),
        "{}",
        message.text
    );
    assert!(message.html.contains("Search - shop[.]example[.]com"));
    assert!(!message.html.contains("shop.example.com"));
    assert!(!message.text.contains("shop.example.com"));
}

#[tokio::test]
#[serial]
async fn without_an_ai_key_the_conversion_tracking_recommendations_are_kept() {
    let f = Fixture::monitoring().await;
    let mut settings = Settings::load(&f.ctx).unwrap();
    settings.openai_key = None;
    let mut check = check(day_number(today()));
    check.findings = vec![
        Finding {
            campaign_id: None,
            rule: findings::Finding {
                key: "tracking-no-results".into(),
                title: "Conversion tracking has too few results".into(),
                detail: "Google Ads recorded 4 conversions in the last 30 days.".into(),
                subjects: vec![],
                spend: 0.0,
            },
        },
        standing_finding(),
    ];
    let recommendations = keysend::monitoring::deep_check::review(
        &settings,
        &check,
        &Default::default(),
        AllFollowAccount { judged: true },
    )
    .await
    .unwrap()
    .recommendations;
    assert_eq!(
        recommendations
            .iter()
            .map(|recommendation| recommendation.finding_key.as_str())
            .collect::<Vec<_>>(),
        ["tracking-no-results"]
    );
    assert!(f.calls.lock().unwrap().commentary.is_empty());
}

#[tokio::test]
#[serial]
async fn recommendations_skip_bad_entries_and_bound_spend_and_count() {
    let f = Fixture::monitoring().await;
    let mut check = check(day_number(today()));
    for index in 0..20 {
        let mut finding = standing_finding();
        finding.rule.key = format!("campaign-cost-high:{index}");
        finding.rule.spend = 100.0 * index as f64;
        check.findings.push(finding);
    }
    let mut entries = vec![
        json!({"finding":"campaign-cost-high:19","intent":"investigate","action":"Review Search - Real.","reason":"Spend was 999999."}),
    ];
    entries.push(json!({"finding":"campaign-cost-high:0","intent":"investigate","action":"Review Search - Real.","reason":"Its cost is high."}));
    for index in (8..20).rev() {
        entries.push(json!({"finding":format!("campaign-cost-high:{index}"),"intent":"investigate","action":"Overall, review Search - Real.","reason":"Its cost is high."}));
    }
    f.calls.lock().unwrap().override_recommendations = Some(json!({"recommendations":entries}));
    let recommendations = keysend::monitoring::deep_check::review(
        &Settings::load(&f.ctx).unwrap(),
        &check,
        &Default::default(),
        AllFollowAccount { judged: true },
    )
    .await
    .unwrap()
    .recommendations;
    assert_eq!(recommendations.len(), 5);
    assert_eq!(recommendations[0].finding_key, "campaign-cost-high:19");
    assert_eq!(recommendations[4].finding_key, "campaign-cost-high:15");
    let calls = f.calls.lock().unwrap();
    let input: Value =
        serde_json::from_str(calls.commentary[0]["input"].as_str().unwrap()).unwrap();
    assert_eq!(input["findings"].as_array().unwrap().len(), 12);
    assert_eq!(input["findings"][0]["key"], "campaign-cost-high:19");
    assert_eq!(input["findings"][11]["key"], "campaign-cost-high:8");
}

#[tokio::test]
#[serial]
async fn conversion_tracking_reads_the_count_without_parsing_deep_check_json() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.ctx.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,"UPDATE deep_checks SET data='broken JSON',conversion_actions=0 WHERE workspace_id=? AND ad_account_customer_id=?",vec![signed.workspace.id().into(),AD.into()])).await.unwrap();
    assert_eq!(
        store
            .conversion_tracking(AD, day_number(end()))
            .await
            .unwrap(),
        (ConversionTracking::NotSetUp, false)
    );
    assert_eq!(
        store
            .window_metrics(AD, day_number(end()), Window::SevenDays, None)
            .await
            .unwrap()
            .current
            .conversions,
        None
    );
    assert!(store
        .window_metrics(AD, day_number(end()), Window::SevenDays, Some(701))
        .await
        .unwrap()
        .current
        .conversions
        .is_none());
}

async fn age_trial(f: &Fixture, ended_days_ago: i64) -> (identity::SignedIn, i64) {
    let signed = signed(f).await;
    let now = Settings::load(&f.ctx).unwrap().now().timestamp();
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started_at=? WHERE workspace_id=?",
            vec![
                (now - ended_days_ago * 86400 - keysend::monitoring::access::TRIAL_SECONDS).into(),
                signed.workspace.id().into(),
            ],
        ))
        .await
        .unwrap();
    (signed, now)
}

#[tokio::test]
#[serial]
async fn account_history_collection_survives_an_expired_trial() {
    let f = Fixture::monitoring().await;
    let (signed, _) = age_trial(&f, 29).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    keysend::monitoring::schedule::collect_history(&f.ctx, &signed.workspace)
        .await
        .unwrap();
    assert_eq!(
        store.history_state(AD).await.unwrap().unwrap().last_day,
        Some(end())
    );
    assert_eq!(
        store.history_state(AD).await.unwrap().unwrap().first_day,
        Some(oldest())
    );
    assert!(store.updates(AD, 0, i64::MAX).await.unwrap().is_empty());
    assert!(!f.calls.lock().unwrap().google.requests.is_empty());
}

#[tokio::test]
#[serial]
async fn account_history_collection_stops_thirty_days_after_access_ended_and_resumes_with_a_subscription(
) {
    let f = Fixture::monitoring().await;
    let (signed, now) = age_trial(&f, 31).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_ad_account_metadata(AD, "EUR", "UTC")
        .await
        .unwrap();
    keysend::monitoring::schedule::collect_history(&f.ctx, &signed.workspace)
        .await
        .unwrap();
    assert!(store.history_state(AD).await.unwrap().is_none());
    assert!(f.calls.lock().unwrap().google.requests.is_empty());
    f.ctx.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,"INSERT INTO subscriptions(workspace_id,stripe_subscription_id,created_at,status,period_start,period_end) VALUES (?,?,?,?,?,?)",vec![signed.workspace.id().into(),"sub_1".into(),(now - 40 * 86400).into(),"active".into(),(now - 10 * 86400).into(),(now + 20 * 86400).into()])).await.unwrap();
    f.ctx
        .db
        .execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "DELETE FROM monitoring_runs",
        ))
        .await
        .unwrap();
    keysend::monitoring::schedule::collect_history(&f.ctx, &signed.workspace)
        .await
        .unwrap();
    assert_eq!(
        store.history_state(AD).await.unwrap().unwrap().last_day,
        Some(end())
    );
    assert!(!store.updates(AD, 0, i64::MAX).await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn alerts_catch_up_three_days_and_deduplicate_each_kind_for_seven() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    {
        let mut calls = f.calls.lock().unwrap();
        let rows = calls.google.monitoring_days.as_mut().unwrap();
        for row in rows.iter_mut().rev().skip(2).take(1) {
            row["metrics"]["costMicros"] = json!("30000000");
        }
    }
    worker::run(&f.ctx, &job).await.unwrap();
    let alerts = store.alerts(AD, 0, i64::MAX).await.unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].day, day_number(end()) - 2);
    assert_eq!(
        alerts[0].event_key,
        keysend::monitoring::alerts::EventKey::SpendDoubled
    );
    assert!(alerts[0].body.contains("2026-10-09"));
    assert!(!alerts[0].body.contains("yesterday"));
    for day in day_number(end()) - 2..=day_number(end()) {
        assert!(store.alert_evaluated(AD, day).await.unwrap());
    }
    let mut repeated = alerts[0].clone();
    repeated.day += 6;
    store
        .save_alerts(
            AD,
            &[repeated.clone()],
            Settings::load(&f.ctx).unwrap().now().timestamp(),
        )
        .await
        .unwrap();
    assert_eq!(store.alerts(AD, 0, i64::MAX).await.unwrap().len(), 1);
    repeated.day += 1;
    store
        .save_alerts(
            AD,
            &[repeated],
            Settings::load(&f.ctx).unwrap().now().timestamp(),
        )
        .await
        .unwrap();
    assert_eq!(store.alerts(AD, 0, i64::MAX).await.unwrap().len(), 2);
}

#[tokio::test]
#[serial]
async fn an_email_claimed_before_a_crash_is_not_sent_again_and_the_log_says_so() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = Settings::load(&f.ctx).unwrap().now().timestamp();
    let alert = keysend::monitoring::alerts::Alert {
        day: day_number(end()),
        event_key: keysend::monitoring::alerts::EventKey::SpendDoubled,
        campaign_id: None,
        name: "Dental".into(),
        metrics: Default::default(),
        normal_value: 10.0,
        body: "Dental: spend doubled.".into(),
    };
    store
        .save_alerts(AD, std::slice::from_ref(&alert), now - 3600)
        .await
        .unwrap();
    assert!(store.claim_alert(AD, &alert, now - 1200).await.unwrap());
    worker::send(
        &f.ctx,
        &EmailJob {
            workspace_id: job.workspace_id,
        },
    )
    .await
    .unwrap();
    assert!(f.calls.lock().unwrap().emails.is_empty());
    assert!(store.pending_alerts(AD).await.unwrap().is_empty());
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("ERROR")
            && logs.contains("Email sending was not confirmed; it is not sent again")
            && logs.contains(r#"kind="alert""#)
            && logs.contains("ad_account=4829153307"),
        "{logs}"
    );
}

#[tokio::test]
#[serial]
async fn a_failed_ai_or_email_request_logs_its_kind_and_status_without_a_secret() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let mut f = Fixture::monitoring().await;
    let ai_url = f.ctx.config.settings.as_ref().unwrap()["services"]["ai_url"]
        .as_str()
        .unwrap()
        .to_owned();
    f.ctx.config.settings.as_mut().unwrap()["services"]["ai_url"] =
        json!(ai_url.replace("/commentary", "/missing"));
    let settings = Settings::load(&f.ctx).unwrap();
    let metrics = keysend::monitoring::metrics::WindowMetrics {
        window: Window::SevenDays,
        end_day: day_number(end()),
        current: Default::default(),
        previous: Default::default(),
    };
    let moved = detection::Detection {
        day: day_number(today()),
        currency: "EUR".into(),
        metrics,
        movements: vec![detection::Movement {
            metric: detection::MetricName::Spend,
            current: 140.0,
            previous: 70.0,
            normal_variation: 10.0,
        }],
        yesterday: vec![],
        campaigns: vec![],
        judge_by_conversions: true,
        causes: vec![],
    };
    assert!(
        update::generate(&settings, moved, AllFollowAccount { judged: true })
            .await
            .unwrap()
            .written_without_ai
    );
    f.calls.lock().unwrap().email_refused = true;
    assert!(keysend::monitoring::email::deliver(
        &settings,
        "a@example.com",
        "Subject",
        "Text",
        None
    )
    .await
    .is_err());
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("AI request failed") && logs.contains("http_status=404"),
        "{logs}"
    );
    assert!(logs.contains("Email service refused the email"), "{logs}");
    for secret in ["stub-openai-secret", "stub-email-secret"] {
        assert!(!logs.contains(secret));
    }
}

#[tokio::test]
#[serial]
async fn pending_alerts_expire_after_two_days_and_respect_opt_out() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = Settings::load(&f.ctx).unwrap().now().timestamp();
    let alert = keysend::monitoring::alerts::Alert {
        day: day_number(end()),
        event_key: keysend::monitoring::alerts::EventKey::SpendDoubled,
        campaign_id: None,
        name: "Dental".into(),
        metrics: Default::default(),
        normal_value: 10.0,
        body: "Dental: spend doubled.".into(),
    };
    store
        .save_alerts(AD, std::slice::from_ref(&alert), now - 172801)
        .await
        .unwrap();
    assert!(store.claim_alert(AD, &alert, now - 172801).await.unwrap());
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                weekly_email: false,
                alerts: false,
                judge_by_conversions: None,
            },
        )
        .await
        .unwrap();
    worker::send(
        &f.ctx,
        &EmailJob {
            workspace_id: job.workspace_id.clone(),
        },
    )
    .await
    .unwrap();
    assert!(store.pending_alerts(AD).await.unwrap().is_empty());
    assert_eq!(store.alerts(AD, 0, i64::MAX).await.unwrap().len(), 1);
    assert!(f.calls.lock().unwrap().emails.is_empty());
    let mut fresh = alert.clone();
    fresh.event_key = keysend::monitoring::alerts::EventKey::ConversionsZero;
    store.save_alerts(AD, &[fresh.clone()], now).await.unwrap();
    worker::send(
        &f.ctx,
        &EmailJob {
            workspace_id: job.workspace_id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(store.pending_alerts(AD).await.unwrap().len(), 1);
    assert!(f.calls.lock().unwrap().emails.is_empty());
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                weekly_email: false,
                alerts: true,
                judge_by_conversions: None,
            },
        )
        .await
        .unwrap();
    worker::send(
        &f.ctx,
        &EmailJob {
            workspace_id: job.workspace_id,
        },
    )
    .await
    .unwrap();
    assert!(store.pending_alerts(AD).await.unwrap().is_empty());
    assert_eq!(
        f.calls.lock().unwrap().emails[0]["subject"],
        "Alert: Conversions went to zero in Northwind Dental"
    );
}

#[tokio::test]
#[serial]
async fn zero_conversions_are_judged_only_on_days_that_are_two_days_old() {
    for (zero_days, expected) in [(2, None), (3, Some(day_number(end()) - 1))] {
        let f = Fixture::monitoring().await;
        let job = f.prepare_monitoring().await;
        let signed = signed(&f).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        {
            let mut calls = f.calls.lock().unwrap();
            let rows = calls.google.monitoring_days.as_mut().unwrap();
            for row in rows.iter_mut() {
                row["metrics"]["conversions"] = json!(3.0);
            }
            for row in rows.iter_mut().rev().take(zero_days) {
                row["metrics"]["conversions"] = json!(0.0);
            }
        }
        for (query, values) in [
            ("UPDATE campaign_days SET conversions=3", vec![]),
            (
                "UPDATE campaign_days SET conversions=0 WHERE day>?",
                vec![(day_number(end()) - zero_days as i64).into()],
            ),
        ] {
            f.ctx
                .db
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    query,
                    values,
                ))
                .await
                .unwrap();
        }
        worker::run(&f.ctx, &job).await.unwrap();
        let alerts = store.alerts(AD, 0, i64::MAX).await.unwrap();
        assert_eq!(
            alerts
                .iter()
                .map(|alert| (alert.event_key, alert.day))
                .collect::<Vec<_>>(),
            expected
                .map(|day| (keysend::monitoring::alerts::EventKey::ConversionsZero, day))
                .into_iter()
                .collect::<Vec<_>>(),
            "{zero_days} zero days"
        );
    }
}

#[tokio::test]
#[serial]
async fn budget_increase_prevents_account_spend_alerts() {
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
            row["metrics"]["costMicros"] = json!("30000000");
        }
        calls.google.monitoring_changes = vec![
            json!({"changeEvent":{"changeDateTime":"2026-10-07 10:00:00","changeResourceType":"CAMPAIGN_BUDGET","resourceChangeOperation":"UPDATE","resourceName":"customers/4829153307/changeEvents/1","campaign":"customers/4829153307/campaigns/701","changedFields":"amountMicros","oldResource":{"campaignBudget":{"amountMicros":"10000000"}},"newResource":{"campaignBudget":{"amountMicros":"30000000"}}}}),
        ];
    }
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert!(store.alerts(AD, 0, i64::MAX).await.unwrap().is_empty());
    assert_eq!(store.updates(AD, 0, i64::MAX).await.unwrap().len(), 1);
}

const BUDGET_CAUSE: &str = "701/account-change-budget/spend/2026-10-07";

fn budget_raised_before_the_spend_rose(f: &Fixture) {
    budget_raised_from(f, 10, 30);
}

fn budget_raised_from(f: &Fixture, old: u32, new: u32) {
    let micros = |euros: u32| format!("{euros}000000");
    let mut calls = f.calls.lock().unwrap();
    for row in calls
        .google
        .monitoring_days
        .as_mut()
        .unwrap()
        .iter_mut()
        .rev()
        .take(4)
    {
        row["metrics"]["costMicros"] = json!("20000000");
    }
    calls.google.monitoring_changes = vec![
        json!({"changeEvent":{"changeDateTime":"2026-10-07 10:00:00","changeResourceType":"CAMPAIGN_BUDGET","resourceChangeOperation":"UPDATE","resourceName":"customers/4829153307/changeEvents/1","campaign":"customers/4829153307/campaigns/701","changedFields":"amountMicros","oldResource":{"campaignBudget":{"amountMicros":micros(old)}},"newResource":{"campaignBudget":{"amountMicros":micros(new)}}}}),
    ];
}

async fn ad_account_page(f: &Fixture) -> String {
    f.request
        .get(&format!("/ad-accounts/{AD}"))
        .add_cookie(f.a.clone())
        .await
        .text()
}

const BUDGET_EVIDENCE: &str = "Daily budget changed from EUR 10.00 to EUR 30.00 on 7 October. Spend averaged EUR 20.00 a day after it, against EUR 10.00 a day on the same weekdays of the week before.";

#[tokio::test]
#[serial]
async fn the_ad_account_page_shows_the_budget_change_that_explains_the_spend_rise_with_its_evidence(
) {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    budget_raised_before_the_spend_rose(&f);
    f.calls.lock().unwrap().override_monitoring = Some(json!({
        "summary":"Spend rose to EUR 110 because the daily budget of Search - Real changed on 7 October.",
        "cause":format!("account/{BUDGET_CAUSE}"),
        "lines":[{"campaign_id":701,"text":"Spend rose to EUR 110 after the daily budget went up on 7 October.","cause":BUDGET_CAUSE}]
    }));
    worker::run(&f.ctx, &job).await.unwrap();
    {
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.commentary.len(), 1);
        let input: Value =
            serde_json::from_str(calls.commentary[0]["input"].as_str().unwrap()).unwrap();
        assert_eq!(input["causes"][0]["level"], "confirmed");
        assert_eq!(input["campaigns"][0]["causes"][0]["id"], BUDGET_CAUSE);
        assert_eq!(
            input["campaigns"][0]["causes"][0]["evidence"],
            BUDGET_EVIDENCE
        );
        let schema = &calls.commentary[0]["text"]["format"]["schema"];
        assert_eq!(
            schema["properties"]["lines"]["items"]["properties"]["cause"]["enum"],
            json!([BUDGET_CAUSE, null])
        );
    }
    let signed = signed(&f).await;
    let stored = WorkspaceStore::new(&f.ctx.db, &signed.workspace)
        .updates(AD, 0, i64::MAX)
        .await
        .unwrap();
    assert!(!stored[0].written_without_ai);
    let page = ad_account_page(&f).await;
    assert!(page.contains("because the daily budget of"), "{page}");
    assert!(!page.contains("Likely cause"), "{page}");
    assert!(page.contains("Confirmed cause"), "{page}");
    assert!(page.contains(BUDGET_EVIDENCE), "{page}");
    let calls = f.calls.lock().unwrap();
    let weekly = calls
        .emails
        .iter()
        .find(|email| email["subject"] == "Your week in Google Ads: Northwind Dental")
        .expect("the Weekly email is sent");
    assert!(
        weekly["text"].as_str().unwrap().contains(&format!(
            "Confirmed cause: Search - Real: {BUDGET_EVIDENCE}"
        )),
        "{weekly}"
    );
}

#[tokio::test]
#[serial]
async fn without_the_ai_the_ad_account_page_states_the_first_cause_with_its_level() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    budget_raised_before_the_spend_rose(&f);
    f.calls.lock().unwrap().override_monitoring = Some(json!({
        "summary":"Spend rose to EUR 110 because the budget went up.",
        "cause":null,
        "lines":[{"campaign_id":701,"text":"Spend rose to EUR 110.","cause":null}]
    }));
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let stored = WorkspaceStore::new(&f.ctx.db, &signed.workspace)
        .updates(AD, 0, i64::MAX)
        .await
        .unwrap();
    assert!(stored[0].written_without_ai);
    let page = ad_account_page(&f).await;
    assert!(page.contains("Confirmed cause"), "{page}");
    assert!(page.contains(BUDGET_EVIDENCE), "{page}");
    assert!(!page.contains("because the budget went up"), "{page}");
    assert!(
        page.contains("Spend rose because of the Account change on 7 October."),
        "{page}"
    );
}

fn mobile_spend_rise_beside_a_steady_desktop(f: &Fixture) {
    let mut calls = f.calls.lock().unwrap();
    let rows = calls.google.monitoring_days.as_mut().unwrap();
    let desktop = rows
        .iter()
        .map(|row| {
            let mut row = row.clone();
            row["segments"]["device"] = json!("DESKTOP");
            row["metrics"]["costMicros"] = json!("1000000");
            row
        })
        .collect::<Vec<_>>();
    for row in rows.iter_mut().rev().take(4) {
        row["metrics"]["costMicros"] = json!("20000000");
    }
    rows.extend(desktop);
}

const MOBILE_CONTRIBUTION: &str =
    "Mobile accounts for 100% of the rise in Spend: EUR 40.00 of EUR 40.00.";

#[tokio::test]
#[serial]
async fn a_share_of_a_movement_is_shown_as_a_contribution_and_never_as_a_cause() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    mobile_spend_rise_beside_a_steady_desktop(&f);
    f.calls.lock().unwrap().override_monitoring = Some(json!({
        "summary":"Spend rose.",
        "cause":null,
        "lines":[{"campaign_id":701,"text":"Spend rose, mostly on Mobile.","cause":"701/concentration/spend/device-1"}]
    }));
    worker::run(&f.ctx, &job).await.unwrap();
    let page = ad_account_page(&f).await;
    assert!(page.contains("Spend rose, mostly on Mobile."), "{page}");
    assert!(page.contains("Contribution"), "{page}");
    assert!(page.contains(MOBILE_CONTRIBUTION), "{page}");
    assert!(!page.contains("Confirmed cause"), "{page}");
    assert!(!page.contains("Likely cause"), "{page}");
}

#[tokio::test]
#[serial]
async fn a_line_that_gives_a_share_as_the_reason_is_replaced_and_still_shows_a_contribution() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    mobile_spend_rise_beside_a_steady_desktop(&f);
    f.calls.lock().unwrap().override_monitoring = Some(json!({
        "summary":"Spend rose.",
        "cause":null,
        "lines":[{"campaign_id":701,"text":"Spend rose because the daily budget was lowered.","cause":"701/concentration/spend/device-1"}]
    }));
    worker::run(&f.ctx, &job).await.unwrap();
    let signed = signed(&f).await;
    let stored = WorkspaceStore::new(&f.ctx.db, &signed.workspace)
        .updates(AD, 0, i64::MAX)
        .await
        .unwrap();
    assert!(stored[0].written_without_ai);
    let page = ad_account_page(&f).await;
    assert!(!page.contains("lowered"), "{page}");
    assert!(page.contains("Contribution"), "{page}");
    assert!(page.contains(MOBILE_CONTRIBUTION), "{page}");
    assert!(!page.contains("Confirmed cause"), "{page}");
}

#[tokio::test]
#[serial]
async fn a_budget_raise_that_the_spend_never_reached_is_only_a_likely_cause_on_the_page() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    budget_raised_from(&f, 50, 80);
    f.calls.lock().unwrap().override_monitoring = Some(json!({
        "summary":"Spend rose to EUR 110, most likely because the daily budget of Search - Real changed on 7 October.",
        "cause":format!("account/{BUDGET_CAUSE}"),
        "lines":[{"campaign_id":701,"text":"Spend rose to EUR 110 after the daily budget went up on 7 October.","cause":BUDGET_CAUSE}]
    }));
    worker::run(&f.ctx, &job).await.unwrap();
    {
        let calls = f.calls.lock().unwrap();
        let input: Value =
            serde_json::from_str(calls.commentary[0]["input"].as_str().unwrap()).unwrap();
        assert_eq!(input["causes"][0]["level"], "likely");
    }
    let page = ad_account_page(&f).await;
    assert!(
        page.contains("most likely because the daily budget of"),
        "{page}"
    );
    assert!(page.contains("Likely cause"), "{page}");
    assert!(!page.contains("Confirmed cause"), "{page}");
    assert!(
        page.contains("Daily budget changed from EUR 50.00 to EUR 80.00 on 7 October."),
        "{page}"
    );
}

#[tokio::test]
#[serial]
async fn weekly_email_runs_only_on_monday_after_six_utc() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut settings = Settings::load(&f.ctx).unwrap();
    let detected = detection::detect(
        &store
            .monitoring_days(AD, day_number(end()) - 62, day_number(end()))
            .await
            .unwrap(),
        &[campaign()],
        &[],
        day_number(end()),
        day_number(oldest()),
        AllFollowAccount { judged: true },
        "EUR",
    );
    store
        .save_update(
            AD,
            &update::generate(&settings, detected, AllFollowAccount { judged: true })
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    for clock in ["2026-10-12T05:59:59Z", "2026-10-13T12:00:00Z"] {
        settings.services.test_now = Some(clock.parse().unwrap());
        worker::weekly_content(&settings, &store).await.unwrap();
        assert!(store
            .weekly_email(day_number(today()))
            .await
            .unwrap()
            .is_none());
    }
    settings.services.test_now = Some("2026-10-12T06:00:00Z".parse().unwrap());
    worker::weekly_content(&settings, &store).await.unwrap();
    assert_eq!(
        store
            .weekly_email(day_number(today()))
            .await
            .unwrap()
            .unwrap()
            .blocks
            .len(),
        1
    );
}

#[tokio::test]
#[serial]
async fn a_user_cannot_choose_a_sixth_ad_account_during_the_trial() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let connection = store.ad_accounts().await.unwrap()[0]
        .google_connection_id
        .clone();
    let choices = (1..=5)
        .map(|index| AdAccountChoice {
            customer_id: format!("{index:010}"),
            name: format!("Choice {index}"),
            login_customer_id: None,
        })
        .collect();
    let selection = store.create_selection(&connection, choices).await.unwrap();
    let response = f
        .request
        .post(&format!("/ad-accounts/choose/{selection}"))
        .add_cookie(f.a.clone())
        .add_header("origin", ORIGIN)
        .form(&[
            ("ad_account", "0000000001"),
            ("ad_account", "0000000002"),
            ("ad_account", "0000000003"),
            ("ad_account", "0000000004"),
            ("ad_account", "0000000005"),
        ])
        .await;
    response.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response
        .text()
        .contains("You would have 6 Ad Accounts. Choose up to 5 in all."));
    assert!(store
        .monitoring_can_select_accounts(
            &["0000000001".into(), "0000000002".into()],
            Settings::load(&f.ctx).unwrap().now().timestamp()
        )
        .await
        .unwrap());
}

#[tokio::test]
#[serial]
async fn missing_ai_key_still_stores_a_factual_movement_update() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut settings = Settings::load(&f.ctx).unwrap();
    settings.openai_key = None;
    let mut rows = store
        .monitoring_days(AD, day_number(end()) - 62, day_number(end()))
        .await
        .unwrap();
    for row in rows
        .iter_mut()
        .filter(|row| row.day >= day_number(end()) - 6)
    {
        row.metrics.spend = 20.0;
    }
    let detected = detection::detect(
        &rows,
        &[campaign()],
        &[],
        day_number(end()),
        day_number(oldest()),
        AllFollowAccount { judged: true },
        "EUR",
    );
    let update = update::generate(&settings, detected, AllFollowAccount { judged: true })
        .await
        .unwrap();
    store.save_update(AD, &update).await.unwrap();
    let saved = store
        .updates(AD, day_number(today()), day_number(today()))
        .await
        .unwrap();
    assert_eq!(
        saved[0].summary,
        "Spend was EUR 140 against EUR 70.00 in the previous 7 days. Yesterday spend was EUR 20.00, against a usual EUR 10.00 for that weekday."
    );
    assert_eq!(
        saved[0].lines[0].text,
        "Spend was EUR 140 against EUR 70.00 in the previous 7 days."
    );
    assert!(saved[0].written_without_ai);
    assert!(f.calls.lock().unwrap().commentary.is_empty());
    assert_eq!(job.ad_account, AD);
}

#[tokio::test]
#[serial]
async fn deep_check_stores_only_the_largest_rows_with_fixed_limits() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    {
        let mut calls = f.calls.lock().unwrap();
        for resource in [
            "keyword_view",
            "search_term_view",
            "landing_page_view",
            "ad_group_ad",
        ] {
            let rows=(0..250).map(|index|json!({"campaign":{"id":"701"},"adGroupCriterion":{"keyword":{"text":format!("Keyword {index}")}},"searchTermView":{"searchTerm":format!("Term {index}")},"landingPageView":{"unexpandedFinalUrl":format!("https://example.com/{index}")},"adGroupAd":{"ad":{"id":format!("{index}"),"name":format!("Ad {index}")},"adStrength":"GOOD"},"metrics":{"costMicros":format!("{}",(index+1)*1_000_000),"clicks":"100","impressions":"1000","conversions":1.0,"conversionsValue":5.0}})).collect();
            calls
                .google
                .monitoring_audit_rows
                .insert(resource.into(), rows);
        }
    }
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let check = keysend::monitoring::deep_check::collect(
        &Settings::load(&f.ctx).unwrap(),
        &store,
        AD,
        day_number(today()),
    )
    .await
    .unwrap();
    store.refresh_deep_check(AD, &check, &[]).await.unwrap();
    let stored = store.latest_deep_check(AD).await.unwrap().unwrap();
    for (rows, limit, minimum) in [
        (&stored.data.audit.search_terms, 200, 51.0),
        (&stored.data.audit.keywords, 200, 51.0),
        (&stored.data.audit.landing_pages, 50, 201.0),
        (&stored.data.audit.ads, 50, 201.0),
    ] {
        assert_eq!(rows.len(), limit);
        assert!(rows.iter().all(|row| row.counts.cost >= minimum));
        assert_eq!(
            rows.iter().map(|row| row.counts.cost).fold(0.0, f64::max),
            250.0
        );
    }
}

#[tokio::test]
#[serial]
async fn deep_check_names_each_ad_and_keeps_the_campaign_id_out_of_the_names() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let ad = |id: &str, ad: Value| {
        let mut ad = ad;
        ad["id"] = json!(id);
        json!({"campaign":{"id":"701"},"adGroup":{"name":"Emergency"},"adGroupAd":{"ad":ad,"adStrength":"POOR"},"metrics":{"costMicros":"5000000","clicks":"134","impressions":"720","conversions":1.0,"conversionsValue":5.0}})
    };
    f.calls.lock().unwrap().google.monitoring_audit_rows.insert(
        "ad_group_ad".into(),
        vec![
            ad("9001", json!({"type":"RESPONSIVE_SEARCH_AD","responsiveSearchAd":{"headlines":[{"text":"Emergency Dentist","pinnedField":"HEADLINE_1"},{"text":"Open Today"},{"text":"Harbour Dental"},{"text":"Book Online"}]}})),
            ad("9002", json!({"type":"EXPANDED_TEXT_AD","expandedTextAd":{"headlinePart1":"Same-Day Appointments","headlinePart2":"Call Harbour Dental"}})),
            ad("9003", json!({"type":"IMAGE_AD","name":"Autumn banner"})),
            ad("9004", json!({"type":"RESPONSIVE_DISPLAY_AD"})),
            ad("9005", json!({"type":"RESPONSIVE_DISPLAY_AD"})),
        ],
    );
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.calls.lock().unwrap().google.requests.clear();
    let check = keysend::monitoring::deep_check::collect(
        &Settings::load(&f.ctx).unwrap(),
        &store,
        AD,
        day_number(today()),
    )
    .await
    .unwrap();
    let ad_queries = f
        .calls
        .lock()
        .unwrap()
        .google
        .requests
        .iter()
        .filter_map(|(_, body)| body["query"].as_str().map(str::to_owned))
        .filter(|query| query.contains("FROM ad_group_ad "))
        .collect::<Vec<_>>();
    assert_eq!(ad_queries.len(), 1);
    assert!(ad_queries[0].starts_with("SELECT campaign.id, ad_group_ad.ad.id, ad_group_ad.ad.name, ad_group_ad.ad.type, ad_group_ad.ad.responsive_search_ad.headlines, ad_group_ad.ad.expanded_text_ad.headline_part1, ad_group_ad.ad.expanded_text_ad.headline_part2, ad_group.name, ad_group_ad.ad_strength, metrics."), "{}", ad_queries[0]);
    let mut ads = check.by_campaign[&701]
        .ads
        .iter()
        .map(|row| (row.id, row.name.as_str()))
        .collect::<Vec<_>>();
    ads.sort();
    assert_eq!(
        ads,
        [
            (
                Some(9001),
                "Emergency Dentist | Open Today | Harbour Dental"
            ),
            (Some(9002), "Same-Day Appointments | Call Harbour Dental"),
            (Some(9003), "Autumn banner"),
            (Some(9004), "Responsive display ad in ad group Emergency"),
            (Some(9005), "Responsive display ad in ad group Emergency"),
        ]
    );
    let campaign = &check.data.audit.campaigns[0];
    assert_eq!(
        (campaign.id, campaign.name.as_str()),
        (Some(701), "Search - Real")
    );
    let poor = check
        .findings
        .iter()
        .find(|finding| finding.rule.key == "ads-poor-strength:701")
        .unwrap();
    assert_eq!(poor.rule.detail, "In the 30 days before the Deep check, 5 ads in Search - Real with EUR 25.00 spend had the ad strength Poor: Autumn banner, Emergency Dentist | Open Today | Harbour Dental, Responsive display ad in ad group Emergency, Responsive display ad in ad group Emergency, Same-Day Appointments | Call Harbour Dental.");
    assert!(check.findings.iter().all(|finding| {
        !finding.rule.detail.contains("(701)") && !finding.rule.title.contains("(701)")
    }));
}

#[tokio::test]
#[serial]
async fn alert_delivery_cooldown_uses_sent_time_after_a_delayed_email() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = Settings::load(&f.ctx).unwrap().now().timestamp();
    let mut alert = keysend::monitoring::alerts::Alert {
        day: day_number(end()),
        event_key: keysend::monitoring::alerts::EventKey::SpendDoubled,
        campaign_id: None,
        name: "Dental".into(),
        metrics: Default::default(),
        normal_value: 10.0,
        body: "Dental: spend doubled.".into(),
    };
    store
        .save_alerts(AD, std::slice::from_ref(&alert), now)
        .await
        .unwrap();
    assert!(store
        .claim_alert(AD, &alert, now + 2 * 86400)
        .await
        .unwrap());
    store
        .finish_alert_email(AD, &alert, now + 2 * 86400, true)
        .await
        .unwrap();
    alert.day += 7;
    store
        .save_alerts(AD, std::slice::from_ref(&alert), now + 7 * 86400)
        .await
        .unwrap();
    assert!(!store
        .claim_alert(AD, &alert, now + 7 * 86400)
        .await
        .unwrap());
    assert!(store
        .claim_alert(AD, &alert, now + 9 * 86400)
        .await
        .unwrap());
}

#[tokio::test]
#[serial]
async fn review_b1_three_ticks_collect_history_once_per_local_day() {
    let mut f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = signed(&f).await;
    for time in [
        "2026-10-12T12:00:00Z",
        "2026-10-12T13:00:00Z",
        "2026-10-12T14:00:00Z",
    ] {
        f.ctx.config.settings.as_mut().unwrap()["services"]["test_now"] = json!(time);
        keysend::monitoring::schedule::collect_history(&f.ctx, &signed.workspace)
            .await
            .unwrap();
    }
    assert_eq!(f.calls.lock().unwrap().google.requests.len(), 6);
}

#[tokio::test]
#[serial]
async fn review_b2_two_stopped_campaigns_have_separate_cooldowns() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = Settings::load(&f.ctx).unwrap().now().timestamp();
    let first = keysend::monitoring::alerts::Alert {
        day: day_number(end()) - 2,
        event_key: keysend::monitoring::alerts::EventKey::CampaignStopped(701),
        campaign_id: Some(701),
        name: "Campaign A".into(),
        metrics: Default::default(),
        normal_value: 10.0,
        body: "Campaign A stopped.".into(),
    };
    store
        .save_alerts(AD, std::slice::from_ref(&first), now - 172800)
        .await
        .unwrap();
    assert!(store.claim_alert(AD, &first, now - 172800).await.unwrap());
    store
        .finish_alert_email(AD, &first, now - 172800, true)
        .await
        .unwrap();
    let second = keysend::monitoring::alerts::Alert {
        day: first.day + 2,
        event_key: keysend::monitoring::alerts::EventKey::CampaignStopped(702),
        campaign_id: Some(702),
        name: "Campaign B".into(),
        body: "Campaign B stopped.".into(),
        ..first.clone()
    };
    store
        .save_alerts(AD, std::slice::from_ref(&second), now)
        .await
        .unwrap();
    assert_eq!(store.alerts(AD, 0, i64::MAX).await.unwrap().len(), 2);
    assert!(store.claim_alert(AD, &second, now).await.unwrap());
    let repeated = keysend::monitoring::alerts::Alert {
        day: first.day + 3,
        ..first
    };
    store.save_alerts(AD, &[repeated], now).await.unwrap();
    assert_eq!(store.alerts(AD, 0, i64::MAX).await.unwrap().len(), 2);
}

#[tokio::test]
#[serial]
async fn review_b6_failed_weekly_email_expires_before_next_monday() {
    let mut f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    f.calls.lock().unwrap().email_refused = true;
    assert!(worker::run(&f.ctx, &job).await.is_err());
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(store.pending_weekly_emails().await.unwrap().len(), 1);
    let now = Settings::load(&f.ctx).unwrap().now().timestamp();
    f.ctx.config.settings.as_mut().unwrap()["services"]["test_now"] = json!("2026-10-19T12:00:00Z");
    f.calls.lock().unwrap().email_refused = false;
    f.calls.lock().unwrap().emails.clear();
    worker::send(
        &f.ctx,
        &EmailJob {
            workspace_id: job.workspace_id,
        },
    )
    .await
    .unwrap();
    assert!(f.calls.lock().unwrap().emails.is_empty());
    assert!(store.pending_weekly_emails().await.unwrap().is_empty());
    assert!(!store
        .claim_weekly_email(day_number(today()), now + 7 * 86400)
        .await
        .unwrap());
}

#[tokio::test]
#[serial]
async fn the_first_run_and_a_later_sign_in_keep_the_trial_start_of_the_first_sign_in() {
    let f = Fixture::configured_clock(false, false, true, "google", 15000, "2026-10-12T12:00:00Z")
        .await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let start = store.trial_started_at().await.unwrap();
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(store.trial_started_at().await.unwrap(), start);
    identity::sign_in(
        &f.ctx.db,
        "a",
        "a@example.com",
        "User A",
        start + 86400,
        None,
    )
    .await
    .unwrap();
    assert_eq!(store.trial_started_at().await.unwrap(), start);
}

#[tokio::test]
#[serial]
async fn removing_the_workspace_and_signing_in_again_keeps_the_first_trial_start() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let start = store.trial_started_at().await.unwrap();
    store.delete_user().await.unwrap();
    let token = identity::sign_in(
        &f.ctx.db,
        "a",
        "a@example.com",
        "User A",
        start + 20 * 86400,
        None,
    )
    .await
    .unwrap();
    let again = identity::authenticate(&f.ctx.db, &token)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(again.workspace.id(), signed.workspace.id());
    let store = WorkspaceStore::new(&f.ctx.db, &again.workspace);
    assert_eq!(store.trial_started_at().await.unwrap(), start);
    assert!(!store.monitoring_access(start + 14 * 86400).await.unwrap());
    let other = identity::sign_in(
        &f.ctx.db,
        "someone-else",
        "else@example.com",
        "Someone Else",
        start + 20 * 86400,
        None,
    )
    .await
    .unwrap();
    let other = identity::authenticate(&f.ctx.db, &other)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        WorkspaceStore::new(&f.ctx.db, &other.workspace)
            .trial_started_at()
            .await
            .unwrap(),
        start + 20 * 86400
    );
    let stored = f
        .ctx
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT subject_hash FROM trial_history",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "subject_hash")
        .unwrap();
    assert_eq!(stored, identity::hash_token("a"));
}

async fn manual_monitoring_fixture(no_conversions: bool) -> (Fixture, MonitoringJob) {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_ad_account_metadata(AD, "EUR", "UTC")
        .await
        .unwrap();
    let campaigns = [
        (701, "Search Growth", 30.0, 4.0),
        (702, "Search Budget", 45.0, 3.0),
        (703, "Search Steady", 25.0, 3.0),
        (704, "Search Costly", 60.0, 0.5),
    ];
    let definitions = campaigns
        .iter()
        .map(|(id, name, cost, _)| Campaign {
            id: *id,
            name: (*name).into(),
            budget_micros: Some((cost * 1_000_000.0) as i64),
            ..campaign()
        })
        .collect::<Vec<_>>();
    let mut daily = Vec::new();
    let mut google_rows = Vec::new();
    for offset in 0..70 {
        let date = end().checked_sub_days(Days::new(offset)).unwrap();
        for (id, _, normal_cost, normal_conversions) in campaigns {
            let cost = if id == 702 && offset < 7 {
                20.0
            } else {
                normal_cost
            };
            let conversions = if no_conversions {
                0.0
            } else if id == 701 && offset < 7 {
                10.0
            } else {
                normal_conversions
            };
            let counts = Counts {
                impressions: 1000.0,
                clicks: 100.0,
                cost,
                conversions,
                value: conversions * 50.0,
            };
            daily.push(CampaignDay {
                day: date,
                campaign_id: id,
                device: 1,
                counts: counts.clone(),
            });
            google_rows.push(json!({"campaign":{"id":id.to_string()},"segments":{"date":date.to_string(),"device":"MOBILE"},"metrics":{"impressions":"1000","clicks":"100","costMicros":((cost*1_000_000.0) as i64).to_string(),"conversions":conversions,"conversionsValue":conversions*50.0}}));
        }
    }
    store
        .save_history(AD, oldest(), end(), &definitions, &daily, &[])
        .await
        .unwrap();
    let mut prior = check(day_number(today()) - 8);
    prior.data.audit.conversion_actions = Some(if no_conversions { 0 } else { 1 });
    store.refresh_deep_check(AD, &prior, &[]).await.unwrap();
    let audit_metrics = |cost: f64, clicks: u32, conversions: f64| json!({"impressions":"10000","clicks":clicks.to_string(),"costMicros":((cost*1_000_000.0) as i64).to_string(),"conversions":if no_conversions {0.0} else {conversions},"conversionsValue":if no_conversions {0.0} else {conversions*50.0}});
    {
        let mut calls = f.calls.lock().unwrap();
        let google = &mut calls.google;
        google.monitoring_days = Some(google_rows);
        google.monitoring_changes = vec![
            json!({"changeEvent":{"changeDateTime":"2026-10-05 10:00:00","changeResourceType":"CAMPAIGN","resourceChangeOperation":"UPDATE","resourceName":"customers/4829153307/changeEvents/strategy","campaign":"customers/4829153307/campaigns/701","changedFields":"biddingStrategyType","oldResource":{"campaign":{"biddingStrategyType":"MANUAL_CPC"}},"newResource":{"campaign":{"biddingStrategyType":"MAXIMIZE_CONVERSIONS"}}}}),
            json!({"changeEvent":{"changeDateTime":"2026-10-05 11:00:00","changeResourceType":"CAMPAIGN_BUDGET","resourceChangeOperation":"UPDATE","resourceName":"customers/4829153307/changeEvents/budget","campaign":"customers/4829153307/campaigns/702","changedFields":"amountMicros","oldResource":{"campaignBudget":{"amountMicros":"45000000"}},"newResource":{"campaignBudget":{"amountMicros":"20000000"}}}}),
        ];
        google.monitoring_audit_rows.insert("campaign".into(), campaigns.iter().map(|(id, name, cost, conversions)| json!({"campaign":{"id":id.to_string(),"name":name,"status":"ENABLED","advertisingChannelType":"SEARCH","biddingStrategyType":"MAXIMIZE_CONVERSIONS"},"campaignBudget":{"amountMicros":((if *id==702 {20.0} else {*cost} *1_000_000.0) as i64).to_string()},"metrics":audit_metrics(cost*30.0,3000,conversions*30.0)})).collect());
        google.monitoring_audit_rows.insert("search_term_view".into(),vec![json!({"campaign":{"id":"701"},"searchTermView":{"searchTerm":"free dental training"},"metrics":audit_metrics(300.0,150,0.0)})]);
        google.monitoring_audit_rows.insert("landing_page_view".into(),vec![json!({"campaign":{"id":"703"},"landingPageView":{"unexpandedFinalUrl":"https://example.com/book"},"metrics":audit_metrics(250.0,1000,1.0)})]);
        google
            .monitoring_audit_rows
            .insert("keyword_view".into(), vec![]);
        google
            .monitoring_audit_rows
            .insert("ad_group_ad".into(), vec![]);
        google.monitoring_audit_rows.insert(
            "conversion_action".into(),
            if no_conversions {
                vec![]
            } else {
                vec![json!({"conversionAction":{"id":"9"}})]
            },
        );
    }
    let job = MonitoringJob {
        workspace_id: signed.workspace.id().into(),
        ad_account: AD.into(),
        immediate: false,
    };
    (f, job)
}

#[tokio::test]
#[serial]
async fn manual_monitoring_fixture_detects_movements_and_three_deep_findings() {
    for no_conversions in [false, true] {
        let (f, job) = manual_monitoring_fixture(no_conversions).await;
        worker::run(&f.ctx, &job).await.unwrap();
        let signed = signed(&f).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let stored = store.updates(AD, 0, i64::MAX).await.unwrap();
        assert_eq!(stored.len(), 1);
        if no_conversions {
            assert_eq!(
                store
                    .conversion_tracking(AD, day_number(end()))
                    .await
                    .unwrap(),
                (ConversionTracking::NotSetUp, false)
            );
            assert_eq!(stored[0].metrics.current.conversions, None);
            continue;
        }
        let campaigns = &stored[0].detection.campaigns;
        assert!(!campaigns
            .iter()
            .any(|campaign| [703, 704].contains(&campaign.campaign_id)));
        assert!(campaigns.iter().any(|campaign| campaign.campaign_id == 701
            && campaign.movements.iter().any(|movement| movement.metric
                == detection::MetricName::Conversions
                && movement.current == 64.0
                && movement.previous == 28.0)));
        assert!(campaigns.iter().any(|campaign| campaign.campaign_id == 702
            && campaign
                .movements
                .iter()
                .any(|movement| movement.metric == detection::MetricName::Spend
                    && movement.current == 140.0
                    && movement.previous == 315.0)));
        let findings = store.monitoring_findings(AD).await.unwrap();
        for rule in [
            "search-terms-no-conversions:",
            "campaign-return-low:",
            "landing-page-low-rate:",
        ] {
            assert!(
                findings
                    .iter()
                    .any(|finding| finding.rule.key.starts_with(rule)),
                "missing {rule}: {findings:?}"
            );
        }
    }
}

#[derive(Clone)]
struct ManualWarnings(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for ManualWarnings {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
#[serial]
#[ignore = "calls the real AI; needs KEYSEND_REAL_AI_KEY"]
async fn real_ai_writes_a_monitoring_update_and_recommendations() {
    assert!(env::var(REAL_AI_KEY).is_ok(), "set {REAL_AI_KEY}");
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let writer = ManualWarnings(warnings.clone());
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    for no_conversions in [false, true] {
        println!(
            "\nAd Account in EUR: {}",
            if no_conversions {
                "no conversions; Conversion tracking not set up"
            } else {
                "Conversion tracking working"
            }
        );
        let (f, job) = manual_monitoring_fixture(no_conversions).await;
        if let Err(error) = worker::run(&f.ctx, &job).await {
            println!("Daily job or delivery error: {error}");
        }
        let signed = signed(&f).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let stored = store.updates(AD, 0, i64::MAX).await.unwrap();
        let update = stored.first().expect("an Update must be stored");
        let mut printed = format!(
            "Update: {}\nWritten without AI: {}\n",
            update.summary, update.written_without_ai
        );
        for line in &update.lines {
            use std::fmt::Write;
            writeln!(printed, "Campaign {}: {}", line.campaign_id, line.text).unwrap();
        }
        println!("{printed}");
        assert!(printed.contains(&update.summary));
        println!(
            "Conversion tracking: {:?}",
            store
                .conversion_tracking(AD, day_number(end()))
                .await
                .unwrap()
                .0
        );
        let findings = store.monitoring_findings(AD).await.unwrap();
        for finding in &findings {
            println!(
                "Finding {}: {} | {}",
                finding.rule.key, finding.rule.title, finding.rule.detail
            );
        }
        for recommendation in store.monitoring_recommendations(AD).await.unwrap() {
            println!(
                "Recommendation {}: {} | {} | Evidence: {}",
                recommendation.finding_key,
                recommendation.action,
                recommendation.reason,
                recommendation.evidence
            );
            if let Some(finding) = findings
                .iter()
                .find(|finding| finding.rule.key == recommendation.finding_key)
            {
                println!(
                    "Recommendation Finding: {} | {}",
                    finding.rule.title, finding.rule.detail
                );
            }
        }
        let logged = std::mem::take(&mut *warnings.lock().unwrap());
        println!(
            "Validator and job warnings:\n{}",
            String::from_utf8_lossy(&logged)
        );
    }
}

mod breakdowns;
mod campaign_settings;
mod charts;
mod email;
mod follow_up;
mod marker;
mod onboarding;
mod pages;
mod translation;

#[tokio::test]
#[serial]
async fn old_jobs_leave_the_queue_and_a_failed_job_stays_longer() {
    let f = Fixture::configured(false, true, true).await;
    for (id, status, age_days) in [
        ("completed-old", "completed", 8),
        ("completed-new", "completed", 6),
        ("failed-old", "failed", 31),
        ("failed-new", "failed", 8),
        ("queued-old", "queued", 40),
    ] {
        f.ctx
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO sqlt_loco_queue(id,name,task_data,status,run_at,created_at) VALUES(?,'Job','{}',?,DATETIME('now','+1 year'),DATETIME('now',?))",
                vec![id.into(), status.into(), format!("-{age_days} days").into()],
            ))
            .await
            .unwrap();
    }
    keysend::monitoring::schedule::remove_old_jobs(&f.ctx)
        .await
        .unwrap();
    let kept: Vec<String> = f
        .ctx
        .db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT id FROM sqlt_loco_queue WHERE name='Job' ORDER BY id",
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get("", "id").unwrap())
        .collect();
    assert_eq!(kept, ["completed-new", "failed-new", "queued-old"]);
}

#[tokio::test]
#[serial]
async fn the_hourly_loop_queues_the_daily_run_of_each_ad_account() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let store_signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &store_signed.workspace);
    assert!(store.updates(AD, 0, i64::MAX).await.unwrap().is_empty());
    let hourly = tokio::spawn(keysend::monitoring::schedule::hourly(f.ctx.clone()));
    let mut stored = Vec::new();
    for _ in 0..100 {
        stored = store.updates(AD, 0, i64::MAX).await.unwrap();
        if !stored.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    hourly.abort();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].day, day_number(today()));
    assert_eq!(job.ad_account, AD);
}

#[tokio::test]
#[serial]
#[ignore = "calls the real AI; needs KEYSEND_REAL_AI_KEY"]
async fn real_ai_translates_the_stored_texts_of_a_romanian_and_a_german_user() {
    assert!(env::var(REAL_AI_KEY).is_ok(), "set {REAL_AI_KEY}");
    for language in ["ro", "de"] {
        let (f, job) = manual_monitoring_fixture(false).await;
        f.ctx
            .db
            .execute_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("UPDATE users SET language='{language}'"),
            ))
            .await
            .unwrap();
        if let Err(error) = worker::run(&f.ctx, &job).await {
            println!("Daily job or delivery error: {error}");
        }
        let signed = signed(&f).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let update = store.updates(AD, 0, i64::MAX).await.unwrap().remove(0);
        println!("\n[{language}] English Update: {}", update.summary);
        for line in &update.lines {
            println!("[{language}] English line: {}", line.text);
        }
        let rows = f
            .ctx
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT text FROM translations ORDER BY created_at, text".to_owned(),
            ))
            .await
            .unwrap();
        assert!(!rows.is_empty(), "{language}: no translation was stored");
        for row in rows {
            println!(
                "[{language}] Translation: {}",
                row.try_get::<String>("", "text").unwrap()
            );
        }
    }
}

#[tokio::test]
#[serial]
async fn the_business_is_researched_from_its_names_alone_and_the_research_is_kept_with_the_deep_check(
) {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut check = check(day_number(today()));
    check.recommendations_generated = false;
    check.findings.push(standing_finding());
    store.refresh_deep_check(AD, &check, &[]).await.unwrap();
    worker::run(&f.ctx, &job).await.unwrap();
    {
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.research.len(), 1);
        let request = &calls.research[0];
        assert_eq!(request["tools"], json!([{"type":"web_search"}]));
        assert_eq!(request["max_tool_calls"], 3);
        let input: Value = serde_json::from_str(request["input"].as_str().unwrap()).unwrap();
        let mut keys = input
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys,
            [
                "campaigns",
                "goal",
                "locations",
                "name",
                "owner_note",
                "website"
            ]
        );
    }
    assert!(store
        .latest_deep_check(AD)
        .await
        .unwrap()
        .unwrap()
        .research
        .is_some());
}

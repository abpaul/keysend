use super::*;

const CAMPAIGN_PAGE: &str = "/ad-accounts/4829153307/campaigns/701";
const WORKED: &str = "It worked. In the 14 days after the change, against the 14 days before it. Spend EUR 280 (before EUR 140), conversions 126 (before 70), cost per conversion EUR 2.22 (before EUR 2.00). The added spend brought more conversions at a similar cost per conversion.";
const AI_LINE: &str =
    "Conversions rose from 70 to 126 after the daily budget change on 26 September, and it worked.";

fn budget_raised(date: &str) -> Value {
    json!({"changeEvent":{
        "changeDateTime":format!("{date} 10:00:00"),
        "changeResourceType":"CAMPAIGN_BUDGET",
        "resourceChangeOperation":"UPDATE",
        "resourceName":format!("customers/{AD}/changeEvents/{date}"),
        "campaign":format!("customers/{AD}/campaigns/701"),
        "changedFields":"amountMicros",
        "oldResource":{"campaignBudget":{"amountMicros":"10000000"}},
        "newResource":{"campaignBudget":{"amountMicros":"20000000"}}
    }})
}

async fn raised_budget_on(date: &str, busier_days: (&str, &str)) -> (Fixture, MonitoringJob) {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    {
        let mut calls = f.calls.lock().unwrap();
        for row in calls.google.monitoring_days.as_mut().unwrap() {
            let day = row["segments"]["date"].as_str().unwrap().to_owned();
            row["metrics"]["conversions"] = json!(5.0);
            if day.as_str() >= busier_days.0 && day.as_str() <= busier_days.1 {
                row["metrics"]["costMicros"] = json!("20000000");
                row["metrics"]["conversions"] = json!(9.0);
            }
        }
        calls.google.monitoring_changes = vec![budget_raised(date)];
    }
    f.ctx
        .db
        .execute_unprepared("UPDATE campaign_days SET conversions=5.0")
        .await
        .unwrap();
    (f, job)
}

async fn latest_update(f: &Fixture) -> update::Update {
    let signed = signed(f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .updates(AD, day_number(today()), day_number(today()))
        .await
        .unwrap()
        .remove(0)
}

fn answer(line: &str) -> Value {
    json!({"summary":"Spend rose after the budget change.","lines":[{"campaign_id":701,"text":line}]})
}

#[tokio::test]
#[serial]
async fn on_its_due_day_the_update_and_the_campaign_page_say_whether_a_raised_budget_worked() {
    let (f, job) = raised_budget_on("2026-09-26", ("2026-09-27", "2026-10-10")).await;
    f.calls.lock().unwrap().override_monitoring = Some(answer(AI_LINE));
    worker::run(&f.ctx, &job).await.unwrap();

    let update = latest_update(&f).await;
    assert!(!update.detection.quiet());
    assert!(!update.written_without_ai);
    assert_eq!(update.lines.len(), 1);
    assert_eq!(update.lines[0].text, AI_LINE);

    {
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.commentary.len(), 1);
        let input: Value =
            serde_json::from_str(calls.commentary[0]["input"].as_str().unwrap()).unwrap();
        let follow_up = &input["campaigns"][0]["follow_ups"][0];
        assert_eq!(
            follow_up["account_change"],
            "Daily budget changed from EUR 10.00 to EUR 20.00 on 26 September"
        );
        assert_eq!(follow_up["verdict"], "It worked.");
        assert_eq!(follow_up["after"]["Conversions"], "126");
        assert_eq!(follow_up["before"]["Conversions"], "70");
        assert!(calls.emails.iter().any(|email| email["subject"]
            == "Your week in Google Ads: Northwind Dental"
            && email["text"].as_str().unwrap().contains(AI_LINE)));
    }

    let page = f.page(CAMPAIGN_PAGE).await;
    assert!(
        page.contains("26 September: Daily budget changed from EUR 10.00 to EUR 20.00."),
        "{page}"
    );
    assert!(page.contains(WORKED), "{page}");

    f.ctx
        .db
        .execute_unprepared("UPDATE users SET language='ro' WHERE email='a@example.com'")
        .await
        .unwrap();
    let romanian = f.page(CAMPAIGN_PAGE).await;
    assert!(romanian.contains("A funcționat."), "{romanian}");
    assert!(!romanian.contains("It worked."));
}

#[tokio::test]
#[serial]
async fn an_ai_line_that_changes_the_verdict_is_replaced_by_the_code_written_follow_up() {
    let (f, job) = raised_budget_on("2026-09-26", ("2026-09-27", "2026-10-10")).await;
    f.calls.lock().unwrap().override_monitoring = Some(answer(
        "Conversions rose from 70 to 126 after the daily budget change on 26 September, but it has not worked yet.",
    ));
    worker::run(&f.ctx, &job).await.unwrap();
    let update = latest_update(&f).await;
    assert!(update.written_without_ai);
    assert_eq!(
        update.lines[0].text,
        "Daily budget changed from EUR 10.00 to EUR 20.00 on 26 September: It worked. Spend EUR 280 (before EUR 140), conversions 126 (before 70), cost per conversion EUR 2.22 (before EUR 2.00)."
    );
    assert_eq!(f.calls.lock().unwrap().commentary.len(), 1);
}

#[tokio::test]
#[serial]
async fn a_change_younger_than_its_due_day_shows_the_day_when_the_result_comes() {
    let (f, job) = raised_budget_on("2026-10-05", ("2026-10-06", "2026-10-11")).await;
    worker::run(&f.ctx, &job).await.unwrap();
    let update = latest_update(&f).await;
    assert!(update
        .detection
        .campaigns
        .iter()
        .all(|campaign| campaign.follow_ups.is_empty()));
    let page = f.page(CAMPAIGN_PAGE).await;
    assert!(
        page.contains("5 October: Daily budget changed from EUR 10.00 to EUR 20.00."),
        "{page}"
    );
    assert!(
        page.contains("KeySend checks the result on 21 October."),
        "{page}"
    );
    assert!(!page.contains("It worked."));
}

const STOPPED_EVIDENCE: &str = "Campaign stopped serving on 8 October, and Spend was EUR 60.00 in the last 7 days, against EUR 140 in the previous 7 days.";

#[tokio::test]
#[serial]
async fn a_campaign_line_and_its_page_show_a_cause_and_a_follow_up_of_the_same_day() {
    let (f, job) = raised_budget_on("2026-09-26", ("2026-09-27", "2026-10-10")).await;
    {
        let mut calls = f.calls.lock().unwrap();
        for row in calls.google.monitoring_days.as_mut().unwrap() {
            if row["segments"]["date"].as_str().unwrap() >= "2026-10-08" {
                row["metrics"]["costMicros"] = json!("0");
                row["metrics"]["impressions"] = json!("0");
                row["metrics"]["clicks"] = json!("0");
                row["metrics"]["conversions"] = json!(0.0);
            }
        }
        calls.override_monitoring = Some(json!({
            "summary":"Spend fell after the Campaign stopped.",
            "cause":null,
            "lines":[{"campaign_id":701,"text":"Spend fell because the Campaign stopped serving on 8 October.","cause":"701/stopped/spend"}]
        }));
    }
    worker::run(&f.ctx, &job).await.unwrap();

    let update = latest_update(&f).await;
    assert!(!update.written_without_ai);
    assert_eq!(update.detection.campaigns[0].follow_ups.len(), 1);
    assert_eq!(
        update.lines[0].text,
        "Spend fell because the Campaign stopped serving on 8 October."
    );
    assert_eq!(
        update.lines[0].cause,
        Some(keysend::monitoring::causes::Cause {
            level: keysend::monitoring::causes::Level::Confirmed,
            evidence: STOPPED_EVIDENCE.into()
        })
    );

    let page = f.page(CAMPAIGN_PAGE).await;
    assert!(
        page.contains("Spend fell because the Campaign stopped serving on 8 October."),
        "{page}"
    );
    assert!(page.contains("Confirmed cause"), "{page}");
    assert!(page.contains(STOPPED_EVIDENCE), "{page}");
    assert!(
        page.contains("26 September: Daily budget changed from EUR 10.00 to EUR 20.00."),
        "{page}"
    );
    assert!(
        page.contains("In the 14 days after the change, against the 14 days before it."),
        "{page}"
    );
}

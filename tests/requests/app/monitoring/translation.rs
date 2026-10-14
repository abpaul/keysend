use keysend::{
    language::Language,
    monitoring::worker::{TranslationJob, TranslationWorker},
    translation::{self, Translations, Wanted},
};
use loco_rs::bgworker::BackgroundWorker;

use super::*;

const PAGE: &str = "/ad-accounts/4829153307";
const SUMMARY: &str = "In the last 7 days, spend was EUR 140 against EUR 70.00.";

async fn use_language(f: &Fixture, code: &str) {
    f.ctx
        .db
        .execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!("UPDATE users SET language='{code}' WHERE email='a@example.com'"),
        ))
        .await
        .unwrap();
}

async fn moving(f: &Fixture) -> MonitoringJob {
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
    job
}

fn weekly_html(f: &Fixture) -> String {
    f.calls
        .lock()
        .unwrap()
        .emails
        .iter()
        .filter_map(|email| email["html"].as_str())
        .find(|html| html.contains("spend was"))
        .expect("the Weekly email was sent")
        .to_owned()
}

fn translation_requests(f: &Fixture) -> usize {
    f.calls.lock().unwrap().translations.len()
}

fn texts_sent(f: &Fixture) -> Vec<String> {
    f.calls
        .lock()
        .unwrap()
        .translations
        .iter()
        .flat_map(|body| {
            let input: Value = serde_json::from_str(body["input"].as_str().unwrap()).unwrap();
            input["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["text"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[tokio::test]
#[serial]
async fn a_romanian_user_reads_the_update_in_romanian_with_its_names_and_numbers_intact() {
    let f = Fixture::monitoring().await;
    let job = moving(&f).await;
    use_language(&f, "ro").await;
    worker::run(&f.ctx, &job).await.unwrap();

    assert_eq!(translation_requests(&f), 1);
    let sent = texts_sent(&f);
    assert!(sent.contains(&"In the last ⟦1⟧ days, spend was ⟦2⟧ against ⟦3⟧.".to_owned()));
    assert!(sent.contains(&"Overall, ⟦1⟧ spent ⟦2⟧ against ⟦3⟧.".to_owned()));
    let everything = f.calls.lock().unwrap().translations[0].to_string();
    for private in ["Search - Real", "140", "70.00", "EUR"] {
        assert!(!everything.contains(private), "{private} left the server");
    }
    assert!(everything.contains("\\\"Campaign\\\" = \\\"Campanie\\\""));

    let page = f.page(PAGE).await;
    let summary = "[ro] In the last 7 days, spend was 140\u{a0}EUR against 70,00\u{a0}EUR.";
    assert!(page.contains(summary), "{page}");
    assert!(page.contains("[ro] Overall, Search - Real spent 140\u{a0}EUR against 70,00\u{a0}EUR."));
    assert!(!page.contains("<span lang=\"en\""));
    assert!(!page.contains(SUMMARY));
    let html = weekly_html(&f);
    assert!(html.contains(summary), "{html}");
    assert!(!html.contains("<span lang=\"en\">"));
}

#[tokio::test]
#[serial]
async fn an_invalid_answer_is_not_stored_and_the_page_shows_the_marked_english_original() {
    let f = Fixture::monitoring().await;
    f.calls.lock().unwrap().translation_answer = Some(|_, text| format!("{text} 999 <b>bold</b>"));
    let job = moving(&f).await;
    use_language(&f, "ro").await;
    let (captured, _guard) = capture();
    worker::run(&f.ctx, &job).await.unwrap();

    let signed = signed(&f).await;
    let rows = f
        .ctx
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM translations",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.try_get::<i64>("", "n").unwrap(), 0);
    let page = f.page(PAGE).await;
    assert!(
        page.contains(&format!("<span lang=\"en\">{SUMMARY}</span>")),
        "{page}"
    );
    let log = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(log.contains("Translation rejected"), "{log}");
    for private in ["Search - Real", "EUR 140", "spend was", "bold"] {
        assert!(!log.contains(private), "{private} in the log");
    }
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(store.updates(AD, 0, i64::MAX).await.unwrap().len(), 1);
    assert!(weekly_html(&f).contains(&format!("<span lang=\"en\">{SUMMARY}</span>")));
}

#[tokio::test]
#[serial]
async fn an_english_user_triggers_no_translation_and_no_extra_statement() {
    let f = Fixture::monitoring().await;
    let job = moving(&f).await;
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(translation_requests(&f), 0);

    f.statements.lock().unwrap().clear();
    let english = f.page(PAGE).await;
    let english_statements = std::mem::take(&mut *f.statements.lock().unwrap());
    assert!(english_statements
        .iter()
        .all(|statement| !statement.contains("translations")));
    assert!(english.contains(SUMMARY));
    assert!(!english.contains("<span lang=\"en\""));

    use_language(&f, "ro").await;
    let _ = f.page(PAGE).await;
    f.statements.lock().unwrap().clear();
    let _ = f.page(PAGE).await;
    let romanian_statements = std::mem::take(&mut *f.statements.lock().unwrap());
    let reads = romanian_statements
        .iter()
        .filter(|statement| statement.contains("FROM translations"))
        .count();
    assert_eq!(reads, 1);
    assert_eq!(romanian_statements.len(), english_statements.len() + 1);
}

#[tokio::test]
#[serial]
async fn changing_the_language_queues_one_backfill_that_translates_what_the_pages_show() {
    let f = Fixture::monitoring().await;
    let job = moving(&f).await;
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(translation_requests(&f), 0);

    let choose = |code: &'static str| {
        let f = &f;
        async move {
            let response = f
                .post(&f.a, "/settings/language", json!({"language": code}))
                .await;
            assert_eq!(response.status_code(), 303);
        }
    };
    choose("ro").await;
    assert_eq!(translation_requests(&f), 1);
    let page = f.page(PAGE).await;
    assert!(page.contains("[ro] In the last 7 days"));

    choose("ro").await;
    assert_eq!(translation_requests(&f), 1);
    choose("en").await;
    assert_eq!(translation_requests(&f), 1);
    choose("de").await;
    assert_eq!(translation_requests(&f), 2);
    assert!(f.page(PAGE).await.contains("[de] In the last 7 days"));

    let signed = signed(&f).await;
    TranslationWorker::build(&f.ctx)
        .perform(TranslationJob {
            workspace_id: signed.workspace.id().into(),
        })
        .await
        .unwrap();
    assert_eq!(translation_requests(&f), 2);
}

#[tokio::test]
#[serial]
async fn another_workspace_cannot_read_a_translation() {
    let f = Fixture::monitoring().await;
    let job = moving(&f).await;
    use_language(&f, "ro").await;
    worker::run(&f.ctx, &job).await.unwrap();
    let a = signed(&f).await;
    let b = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    let mut wanted = Wanted::default();
    wanted.text(SUMMARY);
    let own = Translations::load(
        &WorkspaceStore::new(&f.ctx.db, &a.workspace),
        Language::Ro,
        wanted,
    )
    .await
    .unwrap();
    assert!(!own.shown(SUMMARY).english);
    let mut wanted = Wanted::default();
    wanted.text(SUMMARY);
    let other = Translations::load(
        &WorkspaceStore::new(&f.ctx.db, &b.workspace),
        Language::Ro,
        wanted,
    )
    .await
    .unwrap();
    assert_eq!(other.shown(SUMMARY).text, SUMMARY);
    assert!(other.shown(SUMMARY).english);
    assert_eq!(translation::hash(SUMMARY).len(), 64);
}

#[tokio::test]
#[serial]
async fn account_changes_and_their_chart_labels_are_translated_with_their_amounts_in_the_format_of_the_language(
) {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    f.ctx
        .db
        .execute_unprepared(&format!(
            "INSERT INTO account_changes(workspace_id,ad_account_customer_id,changed_at,resource_name,resource_type,operation,campaign_id,changed_fields,old_value,new_value) SELECT workspace_id,customer_id,'2026-10-08 10:00:00','customers/{AD}/changeEvents/1','CAMPAIGN_BUDGET','UPDATE',701,'','{{\"campaignBudget\":{{\"amountMicros\":\"50000000\"}}}}','{{\"campaignBudget\":{{\"amountMicros\":\"80000000\"}}}}' FROM ad_accounts"
        ))
        .await
        .unwrap();
    let english = f.page(&format!("{PAGE}/campaigns/701")).await;
    assert!(english.contains("Daily budget changed from EUR 50.00 to EUR 80.00"));
    use_language(&f, "ro").await;
    let signed = signed(&f).await;
    let outcome = translation::translate_account(
        &Settings::load(&f.ctx).unwrap(),
        &WorkspaceStore::new(&f.ctx.db, &signed.workspace),
        AD,
        Language::Ro,
    )
    .await;
    assert_eq!(outcome.rejected, 0);
    let page = f.page(&format!("{PAGE}/campaigns/701")).await;
    assert!(
        page.contains("[ro] Daily budget changed from 50,00\u{a0}EUR to 80,00\u{a0}EUR"),
        "{page}"
    );
    assert!(
        page.contains("[ro] Daily budget 50\u{a0}EUR → 80\u{a0}EUR"),
        "{page}"
    );
    assert!(!page.contains("<span lang=\"en\""));
}

#[tokio::test]
#[serial]
async fn the_alert_email_needs_no_translation_and_is_written_in_the_language_of_the_user() {
    let f = Fixture::monitoring().await;
    let job = moving(&f).await;
    use_language(&f, "ro").await;
    worker::run(&f.ctx, &job).await.unwrap();
    let calls = f.calls.lock().unwrap();
    let alert = calls
        .emails
        .iter()
        .find(|email| {
            email["subject"]
                .as_str()
                .unwrap()
                .contains("Northwind Dental")
        })
        .unwrap();
    assert!(!alert["html"].as_str().unwrap().contains("Spend doubled"));
}

#[tokio::test]
#[serial]
async fn a_romanian_user_reads_the_evidence_of_a_cause_in_romanian() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    budget_raised_before_the_spend_rose(&f);
    f.calls.lock().unwrap().override_monitoring = Some(json!({
        "summary":"Spend rose to EUR 110.",
        "cause":null,
        "lines":[{"campaign_id":701,"text":"Spend rose to EUR 110 after the daily budget went up on 7 October.","cause":BUDGET_CAUSE}]
    }));
    use_language(&f, "ro").await;
    worker::run(&f.ctx, &job).await.unwrap();

    assert!(texts_sent(&f)
        .iter()
        .any(|text| text.starts_with("Daily budget changed from ⟦1⟧ to ⟦2⟧ on ⟦3⟧.")));
    let page = f.page(PAGE).await;
    assert!(page.contains("Cauză confirmată"), "{page}");
    assert!(
        page.contains("[ro] Daily budget changed from 10,00\u{a0}EUR to 30,00\u{a0}EUR"),
        "{page}"
    );
    assert!(
        !page.contains("<span lang=\"en\">Daily budget changed"),
        "{page}"
    );
}

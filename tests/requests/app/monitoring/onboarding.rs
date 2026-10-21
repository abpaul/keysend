use super::*;

struct NewUser {
    f: Fixture,
    selection: String,
}

const HARBOUR: &str = "4829153307";

async fn signed_as(
    f: &Fixture,
    cookie: &axum_extra::extract::cookie::Cookie<'static>,
) -> identity::SignedIn {
    identity::authenticate(&f.ctx.db, cookie.value())
        .await
        .unwrap()
        .unwrap()
}

impl NewUser {
    async fn new(clock: &str) -> Self {
        Self::of(Fixture::configured_clock(false, false, true, "google", 15_000, clock).await).await
    }

    async fn with_queue(clock: &str) -> Self {
        Self::of(Fixture::queued_at(clock).await).await
    }

    async fn of(f: Fixture) -> Self {
        {
            let mut calls = f.calls.lock().unwrap();
            calls.google.monitoring_days = Some(google_days(1.0));
            calls.google.monitoring_zone = Some("UTC".into());
        }
        let b = identity::authenticate(&f.ctx.db, f.b.value())
            .await
            .unwrap()
            .unwrap();
        let store = WorkspaceStore::new(&f.ctx.db, &b.workspace);
        let connection = store
            .save_google_connection(
                "b",
                "b@example.com",
                "never-send-this-refresh-secret",
                &MasterKey::from_base64(KEY).unwrap(),
            )
            .await
            .unwrap();
        let choices = [
            (HARBOUR, "Harbour Dental"),
            ("6170254418", "Northgate Physio"),
            ("3309581126", "Tidewater Vets"),
            ("9042276630", "Harbour Dental (old)"),
            ("5550000001", "Fifth Practice"),
            ("5550000002", "Sixth Practice"),
        ]
        .map(|(id, name)| AdAccountChoice {
            customer_id: id.into(),
            name: name.into(),
            login_customer_id: None,
        })
        .to_vec();
        let selection = store
            .create_selection(&connection.id, choices)
            .await
            .unwrap();
        Self { f, selection }
    }

    fn path(&self) -> String {
        format!("/ad-accounts/choose/{}", self.selection)
    }

    async fn get(&self, path: &str) -> axum_test::TestResponse {
        self.f.request.get(path).add_cookie(self.f.b.clone()).await
    }

    async fn choose(&self, ids: &[&str]) -> axum_test::TestResponse {
        let form = ids.iter().map(|id| ("ad_account", *id)).collect::<Vec<_>>();
        self.f
            .request
            .post(&self.path())
            .add_header("origin", ORIGIN)
            .add_cookie(self.f.b.clone())
            .form(&form)
            .await
    }
}

#[tokio::test]
#[serial]
async fn step_one_invites_the_user_to_sign_in_with_google() {
    let f = Fixture::new(false).await;
    let page = f.request.get("/sign-in").await.text();
    assert!(
        page.contains(r#"<h1><span class="caption">Step 1 of 4</span>Sign in with Google</h1>"#)
    );
    assert!(page.contains("Google's Ads permission includes reading and editing."));
    assert!(page.contains("does not send edit requests."));
    assert!(page.contains("You can remove access in your Google account."));
    assert!(page.contains(
        "Your Trial starts when you sign in. It lasts 14 days and needs no payment card."
    ));
    assert!(page.contains(
        r#"<a class="btn btn-primary btn-lg" href="/auth/google/start">Sign in with Google</a>"#
    ));
    assert!(page.contains("images/kite-welcome.webp"));
    assert!(page.contains(r#"<link rel="stylesheet" href="/static/"#));
    assert!(!page.contains("site-nav"));
    assert!(page.contains(r#"href="/privacy""#));
    assert!(page.contains(r#"href="/privacy#read""#));
    assert!(!page.contains("turbo"));
}

#[tokio::test]
#[serial]
async fn step_two_tells_the_user_what_the_google_question_means() {
    let f = Fixture::new(false).await;
    let page = f.page("/connect").await;
    assert!(page.contains(
        r#"<h1 class="heading-xl"><span class="caption">Step 2 of 4</span>Connect Google Ads</h1>"#
    ));
    assert!(page.contains("Google now asks if KeySend can see your Google Ads. Choose Allow."));
    assert!(page.contains("Google's Ads permission includes reading and editing."));
    assert!(page.contains("does not send edit requests."));
    assert!(page.contains("You can remove access in your Google account."));
    assert!(
        page.find(r#"href="/privacy#read""#).unwrap()
            < page.find(r#"action="/connections/google/start""#).unwrap()
    );
    assert!(
        page.contains(r#"<form class="actions" method="post" action="/connections/google/start">"#)
    );
    assert!(page.contains(r#"<button class="button" type="submit">Connect Google Ads</button>"#));
    assert!(!page.contains("site-nav"));
    assert_eq!(
        f.request.get("/connect").await.header("location"),
        "/sign-in"
    );
}

#[tokio::test]
#[serial]
async fn step_three_lists_the_ad_accounts_with_their_google_ads_ids() {
    let user = NewUser::new("2026-10-12T12:00:00Z").await;
    let response = user.get(&user.path()).await;
    assert_eq!(response.status_code(), 200);
    let page = response.text();
    assert!(page.contains(
        r#"<h1 class="heading-xl"><span class="caption">Step 3 of 4</span>Choose your Ad Accounts</h1>"#
    ));
    assert!(page
        .contains("Choose up to 5 Ad Accounts. We found 6 Ad Accounts in your Google Connection."));
    assert!(page.contains(r#"<label class="choice"><input type="checkbox" name="ad_account" value="4829153307" id="account-1"><span>Harbour Dental <small class="num">482-915-3307</small></span></label>"#));
    assert!(page.contains(r#"<small class="num">617-025-4418</small>"#));
    assert!(page.contains(r#"<button class="button" type="submit">Continue</button>"#));
    assert!(!page.contains("There is a problem"));
    assert!(!page.contains("site-nav"));
    assert_eq!(
        user.f
            .request
            .get(&user.path())
            .add_cookie(user.f.a.clone())
            .await
            .status_code(),
        404
    );
}

#[tokio::test]
#[serial]
async fn a_user_is_refused_six_ad_accounts_or_none() {
    let user = NewUser::new("2026-10-12T12:00:00Z").await;
    let response = user
        .choose(&[
            HARBOUR,
            "6170254418",
            "3309581126",
            "9042276630",
            "5550000001",
            "5550000002",
        ])
        .await;
    assert_eq!(response.status_code(), 422);
    let page = response.text();
    assert!(page.contains("<title>Error: Choose your Ad Accounts | KeySend</title>"));
    assert!(page.contains(
        r##"<li><a href="#account-1">You chose 6 Ad Accounts. Choose up to 5.</a></li>"##
    ));
    assert!(page.contains(r#"<p class="error-message" id="choose-error"><span class="visually-hidden">Error: You chose 6 Ad Accounts. Choose up to 5.</span><span aria-hidden="true">You chose 6 Ad Accounts. Choose up to 5.</span></p>"#));
    assert!(page.contains(r#"aria-describedby="choose-hint choose-error""#));
    let none = user.choose(&[]).await;
    assert_eq!(none.status_code(), 422);
    assert!(none.text().contains("Choose at least one Ad Account."));
    let b = signed_as(&user.f, &user.f.b).await;
    let store = WorkspaceStore::new(&user.f.ctx.db, &b.workspace);
    assert!(store.ad_accounts().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn a_user_can_choose_five_ad_accounts() {
    let user = NewUser::new("2026-10-12T02:00:00Z").await;
    let chosen = user
        .choose(&[
            HARBOUR,
            "6170254418",
            "3309581126",
            "9042276630",
            "5550000001",
        ])
        .await;
    assert_eq!(chosen.status_code(), 303, "{}", chosen.text());
    assert_eq!(chosen.header("location"), "/ad-accounts");
    let page = user.get("/ad-accounts").await.text();
    for name in [
        "Harbour Dental",
        "Northgate Physio",
        "Tidewater Vets",
        "Harbour Dental (old)",
        "Fifth Practice",
    ] {
        assert!(page.contains(name), "{page}");
    }
}

#[tokio::test]
#[serial]
async fn choosing_one_lands_on_its_page_with_the_numbers_and_the_first_update_before_four() {
    let user = NewUser::new("2026-10-12T02:00:00Z").await;
    let response = user.choose(&[HARBOUR]).await;
    assert_eq!(response.status_code(), 303, "{}", response.text());
    assert_eq!(response.header("location"), "/ad-accounts/4829153307");
    let b = signed_as(&user.f, &user.f.b).await;
    let store = WorkspaceStore::new(&user.f.ctx.db, &b.workspace);
    let updates = store.updates(HARBOUR, 0, i64::MAX).await.unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].day, day_number(today()));
    assert_eq!(
        store
            .window_metrics(HARBOUR, day_number(end()), Window::ThirtyDays, None)
            .await
            .unwrap()
            .current
            .spend,
        300.0
    );
    let page = user.get("/ad-accounts/4829153307?window=30").await.text();
    assert!(page.contains(r#"<h2 class="heading-m" id="window">Last 30 days</h2>"#));
    assert!(
        page.contains(r#"<dd class="metric__value">EUR 300</dd>"#),
        "{page}"
    );
    assert!(page.contains("Today's Update"));
    assert!(!page.contains("Step 4 of 4"));

    let before = user.f.calls.lock().unwrap().google.requests.len();
    let signed = signed_as(&user.f, &user.f.b).await;
    worker::run(
        &user.f.ctx,
        &MonitoringJob {
            workspace_id: signed.workspace.id().into(),
            ad_account: HARBOUR.into(),
            immediate: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(store.updates(HARBOUR, 0, i64::MAX).await.unwrap().len(), 1);
    assert_eq!(user.f.calls.lock().unwrap().google.requests.len(), before);
}

#[tokio::test]
#[serial]
async fn the_daily_run_still_waits_for_four_in_the_time_zone_of_the_ad_account() {
    let user = NewUser::new("2026-10-12T02:00:00Z").await;
    let signed = signed_as(&user.f, &user.f.b).await;
    let store = WorkspaceStore::new(&user.f.ctx.db, &signed.workspace);
    store
        .save_ad_account_metadata(HARBOUR, "EUR", "UTC")
        .await
        .unwrap();
    worker::run(
        &user.f.ctx,
        &MonitoringJob {
            workspace_id: signed.workspace.id().into(),
            ad_account: HARBOUR.into(),
            immediate: false,
        },
    )
    .await
    .unwrap();
    assert!(store
        .updates(HARBOUR, 0, i64::MAX)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[serial]
async fn choosing_several_lands_on_the_list_and_the_workspace_scope_holds() {
    let user = NewUser::new("2026-10-12T12:00:00Z").await;
    let response = user.choose(&[HARBOUR, "6170254418"]).await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(response.header("location"), "/ad-accounts");
    let b = signed_as(&user.f, &user.f.b).await;
    let store = WorkspaceStore::new(&user.f.ctx.db, &b.workspace);
    assert_eq!(store.ad_accounts().await.unwrap().len(), 2);
    assert_eq!(user.choose(&[HARBOUR]).await.status_code(), 400);
}

#[tokio::test]
#[serial]
async fn the_first_page_shows_a_waiting_card_that_a_turbo_frame_replaces_and_a_link_reloads() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let page = f.page("/ad-accounts/4829153307").await;
    assert!(page.contains(
        r#"<h1 class="heading-xl"><span class="caption">Step 4 of 4</span>Northwind Dental</h1>"#
    ));
    assert!(page
        .contains(r#"<turbo-frame id="first-update" src="/ad-accounts/4829153307/first-update">"#));
    assert!(page.contains(r#"<script type="module" src="/static/"#));
    assert!(page.contains("turbo.min.js"));
    assert!(page.contains(r#"<body data-turbo="false">"#));
    assert!(page.contains(
        r#"<a href="/ad-accounts/4829153307" data-turbo="false">Check for the Update</a>"#
    ));
    assert!(page.contains("We are reading your Ad Account"));

    let waiting = f.page("/ad-accounts/4829153307/first-update").await;
    assert!(waiting.starts_with(r#"<turbo-frame id="first-update">"#));
    assert!(waiting.contains("We are reading your Ad Account"));
    assert!(!waiting.contains("<html"));

    worker::run(&f.ctx, &job).await.unwrap();
    let ready = f.page("/ad-accounts/4829153307/first-update").await;
    assert!(ready.starts_with(r#"<turbo-frame id="first-update">"#));
    assert!(ready.contains(update::QUIET));
    assert!(ready.contains("Today's Update"));
    assert!(!ready.contains("We are reading your Ad Account"));
    let full = f.page("/ad-accounts/4829153307").await;
    assert!(!full.contains("turbo"));
    assert_eq!(full.matches("<script").count(), 2);
    assert!(full.contains("/window-switch.js") && full.contains("/popover.js"));

    let other = f
        .request
        .get("/ad-accounts/4829153307/first-update")
        .add_cookie(f.b.clone())
        .await;
    assert_eq!(other.status_code(), 404);
    assert_eq!(
        f.request
            .get("/ad-accounts/4829153307/first-update")
            .await
            .status_code(),
        303
    );
}

#[tokio::test]
#[serial]
async fn an_ad_account_that_cannot_be_read_says_so_in_the_frame() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_ad_account_metadata(AD, "EUR", "UTC")
        .await
        .unwrap();
    store.history_failed(AD, "Google refused").await.unwrap();
    let page = f.page("/ad-accounts/4829153307/first-update").await;
    assert!(page.contains("We could not read this Ad Account from Google Ads."));
    assert!(!page.contains("Google refused"));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn the_queue_worker_writes_the_first_update_after_the_user_chooses_an_ad_account() {
    let user = NewUser::with_queue("2026-10-12T12:00:00Z").await;
    let chosen = user.choose(&[HARBOUR]).await;
    assert_eq!(chosen.status_code(), 303, "{}", chosen.text());
    assert_eq!(chosen.header("location"), "/ad-accounts/4829153307");
    let mut frame = String::new();
    for _ in 0..10 {
        frame = user
            .get("/ad-accounts/4829153307/first-update")
            .await
            .text();
        if frame.contains(update::QUIET) {
            break;
        }
    }
    let finished: Vec<(String, String)> = user
        .f
        .ctx
        .db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT name,status FROM sqlt_loco_queue ORDER BY created_at",
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.try_get("", "name").unwrap(),
                row.try_get("", "status").unwrap(),
            )
        })
        .collect();
    assert!(frame.contains(update::QUIET), "{finished:?} {frame}");
    let signed = signed_as(&user.f, &user.f.b).await;
    let store = WorkspaceStore::new(&user.f.ctx.db, &signed.workspace);
    assert!(
        store
            .activity_count(keysend::workspace::measurement::Activity::Synced)
            .await
            .unwrap()
            > 0
    );
    for kind in keysend::workspace::measurement::Activity::ALL
        .into_iter()
        .filter(|kind| kind.by_user())
    {
        assert_eq!(store.activity_count(kind).await.unwrap(), 0);
    }

    assert!(
        finished.contains(&("MonitoringWorker".to_owned(), "completed".to_owned())),
        "{finished:?}"
    );
}

#[tokio::test]
#[serial]
async fn the_frame_waits_for_the_first_run_and_answers_with_the_update() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let started = std::time::Instant::now();
    let (frame, ()) = tokio::join!(
        f.request
            .get("/ad-accounts/4829153307/first-update")
            .add_cookie(f.a.clone()),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            worker::run(&f.ctx, &job).await.unwrap();
        }
    );
    let frame = frame.text();
    assert!(frame.contains(update::QUIET), "{frame}");
    assert!(!frame.contains("We are reading your Ad Account"));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[tokio::test]
#[serial]
async fn the_only_ad_account_that_google_offers_is_ticked() {
    let user = NewUser::new("2026-10-12T12:00:00Z").await;
    let b = signed_as(&user.f, &user.f.b).await;
    let store = WorkspaceStore::new(&user.f.ctx.db, &b.workspace);
    let connection = store.google_connections().await.unwrap().remove(0).id;
    let selection = store
        .create_selection(
            &connection,
            vec![AdAccountChoice {
                customer_id: HARBOUR.into(),
                name: "Harbour Dental".into(),
                login_customer_id: None,
            }],
        )
        .await
        .unwrap();
    let page = user
        .get(&format!("/ad-accounts/choose/{selection}"))
        .await
        .text();
    assert!(page.contains("We found 1 Ad Account in your Google Connection."));
    assert!(page.contains(r#"value="4829153307" id="account-1" checked>"#));
    let several = user.get(&user.path()).await.text();
    assert!(!several.contains("checked>"));
}

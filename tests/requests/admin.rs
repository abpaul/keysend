use super::app::{capture, Fixture};
use axum_extra::extract::cookie::{Cookie, SameSite};
use axum_test::TestResponse;
use sea_orm::ConnectionTrait;
use serde_json::json;
use serial_test::serial;

pub(super) async fn fixture() -> Fixture {
    fixture_for_host(false).await
}

pub(super) async fn fixture_with_prices() -> Fixture {
    let previous = std::env::var("ADMIN_EMAILS").ok();
    std::env::set_var("ADMIN_EMAILS", "admin@example.com");
    let f = Fixture::admin_priced().await;
    match previous {
        Some(v) => std::env::set_var("ADMIN_EMAILS", v),
        None => std::env::remove_var("ADMIN_EMAILS"),
    }
    f
}

async fn fixture_for_host(secure: bool) -> Fixture {
    let previous = std::env::var("ADMIN_EMAILS").ok();
    std::env::set_var("ADMIN_EMAILS", " Admin@Example.com , other@example.com ");
    let f = if secure {
        Fixture::admin_secure().await
    } else {
        Fixture::new(false).await
    };
    match previous {
        Some(v) => std::env::set_var("ADMIN_EMAILS", v),
        None => std::env::remove_var("ADMIN_EMAILS"),
    }
    f
}
async fn request(f: &Fixture, email: &str, ip: &str) -> TestResponse {
    f.request
        .post("/admin/code")
        .add_header(
            "origin",
            keysend::settings::Settings::load(&f.ctx)
                .unwrap()
                .origin
                .as_str(),
        )
        .add_header("cf-connecting-ip", ip)
        .form(&json!({"email":email}))
        .await
}
async fn code(f: &Fixture) -> String {
    code_after(f, 0).await
}
async fn code_after(f: &Fixture, sent: usize) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(mail) = f.calls.lock().unwrap().emails.get(sent) {
                return mail["text"]
                    .as_str()
                    .unwrap()
                    .split_whitespace()
                    .find(|v| {
                        v.len() == 7
                            && v.ends_with('.')
                            && v[..6].bytes().all(|b| b.is_ascii_digit())
                    })
                    .unwrap()[..6]
                    .to_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn begin(f: &Fixture) -> (Cookie<'static>, String) {
    let sent = f.calls.lock().unwrap().emails.len();
    let response = request(f, "ADMIN@example.com", "192.0.2.1").await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(response.header("location"), "/admin/verify");
    (
        response.cookie("ks_admin_challenge").into_owned(),
        code_after(f, sent).await,
    )
}
async fn verify(f: &Fixture, cookie: &Cookie<'static>, code: &str) -> TestResponse {
    f.request
        .post("/admin/verify")
        .add_header(
            "origin",
            keysend::settings::Settings::load(&f.ctx)
                .unwrap()
                .origin
                .as_str(),
        )
        .add_cookie(cookie.clone())
        .form(&json!({"code":code}))
        .await
}
pub(super) async fn signed(f: &Fixture) -> Cookie<'static> {
    let (challenge, code) = begin(f).await;
    let response = verify(f, &challenge, &code).await;
    assert_eq!(response.status_code(), 303, "{}", response.text());
    response.cookie("ks_admin").into_owned()
}
#[tokio::test]
#[serial]
async fn an_administrator_signs_in_with_the_emailed_code() {
    let f = fixture().await;
    let cookie = signed(&f).await;
    let page = f.request.get("/admin").add_cookie(cookie).await;
    assert_eq!(page.status_code(), 200);
    assert!(page.text().contains("Overview"));
    assert_eq!(page.header("cache-control"), "no-store");
    assert!(page.text().contains("noindex"));
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.emails[0]["to"], json!(["admin@example.com"]));
    assert!(calls.emails[0]["html"]
        .as_str()
        .unwrap()
        .contains("10 minutes"));
}
#[tokio::test]
#[serial]
async fn an_address_not_on_the_list_receives_no_email_and_the_same_page() {
    let f = fixture().await;
    let denied = request(&f, "intruder@example.com", "192.0.2.1").await;
    assert_eq!(denied.status_code(), 303);
    assert_eq!(denied.header("location"), "/admin/verify");
    assert!(f.calls.lock().unwrap().emails.is_empty());
    let page = f
        .request
        .get("/admin/verify")
        .add_cookie(denied.cookie("ks_admin_challenge"))
        .await;
    assert!(page
        .text()
        .contains("If this address may sign in, a code is on its way."));
    let allowed = request(&f, "admin@example.com", "192.0.2.2").await;
    code(&f).await;
    assert_eq!(allowed.status_code(), denied.status_code());
    assert_eq!(allowed.header("location"), denied.header("location"));
    let allowed_page = f
        .request
        .get("/admin/verify")
        .add_cookie(allowed.cookie("ks_admin_challenge"))
        .await;
    assert_eq!(allowed_page.text(), page.text());
}
#[tokio::test]
#[serial]
async fn a_code_expires_after_ten_minutes() {
    let f = fixture().await;
    let (cookie, code) = begin(&f).await;
    assert_eq!(cookie.max_age().unwrap().whole_seconds(), 600);
    f.ctx
        .db
        .execute_unprepared("UPDATE admin_challenges SET expires_at=0")
        .await
        .unwrap();
    assert!(verify(&f, &cookie, &code)
        .await
        .text()
        .contains("That code did not work."));
}
#[tokio::test]
#[serial]
async fn a_used_code_cannot_sign_in_again() {
    let f = fixture().await;
    let (cookie, code) = begin(&f).await;
    assert_eq!(verify(&f, &cookie, &code).await.status_code(), 303);
    assert!(verify(&f, &cookie, &code)
        .await
        .text()
        .contains("That code did not work."));
}
#[tokio::test]
#[serial]
async fn a_new_code_replaces_the_earlier_one() {
    let f = fixture().await;
    let (old, old_code) = begin(&f).await;
    f.calls.lock().unwrap().emails.clear();
    let (new, new_code) = begin(&f).await;
    assert!(verify(&f, &old, &old_code)
        .await
        .text()
        .contains("That code did not work."));
    assert_eq!(verify(&f, &new, &new_code).await.status_code(), 303);
}
#[tokio::test]
#[serial]
async fn five_wrong_codes_lock_the_challenge() {
    let f = fixture().await;
    let (cookie, code) = begin(&f).await;
    let wrong = if code == "000000" { "000001" } else { "000000" };
    for _ in 0..5 {
        assert!(verify(&f, &cookie, wrong)
            .await
            .text()
            .contains("That code did not work."));
    }
    let response = verify(&f, &cookie, &code).await;
    assert!(response.text().contains("That code did not work."));
    assert!(response.maybe_cookie("ks_admin").is_none());
}
#[tokio::test]
#[serial]
async fn code_requests_are_throttled_per_address_and_per_ip() {
    let f = fixture().await;
    for n in 0..6 {
        request(&f, "admin@example.com", &format!("192.0.2.{n}")).await;
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while f.calls.lock().unwrap().emails.len() < 5 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.calls.lock().unwrap().emails.len(), 5);
    for _ in 0..10 {
        request(&f, "nobody@example.com", "192.0.2.99").await;
    }
    request(&f, "other@example.com", "192.0.2.99").await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(f.calls.lock().unwrap().emails.len(), 5);
}
#[tokio::test]
#[serial]
async fn the_admin_cookie_is_httponly_strict_secure_and_scoped_to_admin() {
    let f = fixture_for_host(true).await;
    let (challenge, code) = begin(&f).await;
    let response = verify(&f, &challenge, &code).await;
    let session = response.cookie("ks_admin");
    for cookie in [&challenge, &session] {
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Strict));
        assert_eq!(cookie.path(), Some("/admin"));
        assert_eq!(cookie.secure(), Some(true));
    }
    assert_eq!(challenge.max_age().unwrap().whole_seconds(), 600);
    assert_eq!(session.max_age().unwrap().whole_seconds(), 14400);
}
#[tokio::test]
#[serial]
async fn signing_out_ends_the_admin_session() {
    let f = fixture().await;
    let cookie = signed(&f).await;
    let response = f
        .request
        .post("/admin/sign-out")
        .add_header(
            "origin",
            keysend::settings::Settings::load(&f.ctx)
                .unwrap()
                .origin
                .as_str(),
        )
        .add_cookie(cookie.clone())
        .await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(
        response
            .cookie("ks_admin")
            .max_age()
            .unwrap()
            .whole_seconds(),
        0
    );
    assert!(f
        .request
        .get("/admin")
        .add_cookie(cookie)
        .await
        .text()
        .contains("Send code"));
}
#[tokio::test]
#[serial]
async fn an_expired_admin_session_returns_to_sign_in() {
    let f = fixture().await;
    let cookie = signed(&f).await;
    f.ctx
        .db
        .execute_unprepared("UPDATE admin_sessions SET expires_at=0")
        .await
        .unwrap();
    assert!(f
        .request
        .get("/admin")
        .add_cookie(cookie)
        .await
        .text()
        .contains("Send code"));
}
#[tokio::test]
#[serial]
async fn a_customer_session_cannot_open_any_admin_page() {
    let f = fixture().await;
    for path in ["/admin", "/admin/verify"] {
        let page = f.request.get(path).add_cookie(f.a.clone()).await;
        assert!(!page.text().contains("Overview"));
    }
    assert!(verify(&f, &f.a, "123456")
        .await
        .text()
        .contains("That code did not work."));
    request(&f, "a@example.com", "192.0.2.1").await;
    assert!(f.calls.lock().unwrap().emails.is_empty());
    assert_eq!(
        f.request
            .post("/admin/sign-out")
            .add_header(
                "origin",
                keysend::settings::Settings::load(&f.ctx)
                    .unwrap()
                    .origin
                    .as_str()
            )
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        303
    );
}
#[tokio::test]
#[serial]
async fn removing_an_address_from_the_list_ends_its_session() {
    let f = fixture().await;
    let cookie = signed(&f).await;
    let mut settings = keysend::settings::Settings::load(&f.ctx).unwrap();
    settings.admin_emails.clear();
    let server = protected(&f, settings);
    let response = server.get("/admin/protected").add_cookie(cookie).await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(response.header("location"), "/admin");
}

fn protected(f: &Fixture, settings: keysend::settings::Settings) -> axum_test::TestServer {
    let router = axum::Router::new()
        .route(
            "/admin/protected",
            axum::routing::get(
                |admin: keysend::controllers::admin::Admin| async move { admin.email },
            ),
        )
        .with_state(f.ctx.clone())
        .layer(axum::Extension(std::sync::Arc::new(settings)))
        .layer(axum::middleware::from_fn(keysend::language::scope_request));
    axum_test::TestServer::new(router).unwrap()
}
#[tokio::test]
#[serial]
async fn a_cross_origin_admin_post_is_refused() {
    let f = fixture().await;
    for path in ["/admin/code", "/admin/verify", "/admin/sign-out"] {
        assert_eq!(
            f.request
                .post(path)
                .add_header("origin", "https://evil.example")
                .form(&json!({"email":"admin@example.com","code":"123456"}))
                .await
                .status_code(),
            403
        );
    }
}
#[tokio::test]
#[serial]
async fn the_code_never_appears_in_logs() {
    let f = fixture().await;
    let (logs, _capture) = capture();
    let (cookie, code) = begin(&f).await;
    verify(&f, &cookie, &code).await;
    let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("admin_signed_in"));
    assert!(!logs.contains(&code));
    assert!(!logs.contains(cookie.value()));
}
#[tokio::test]
#[serial]
async fn two_simultaneous_verifications_of_one_code_sign_in_once() {
    let f = fixture().await;
    let (cookie, code) = begin(&f).await;
    let (a, b) = tokio::join!(verify(&f, &cookie, &code), verify(&f, &cookie, &code));
    assert_eq!([a, b].iter().filter(|r| r.status_code() == 303).count(), 1);
}

#[tokio::test]
#[serial]
async fn an_administrator_needs_an_exact_allowlist_match_and_a_master_key() {
    let f = fixture().await;
    for email in ["admin@example.com.evil", "someone@example.com"] {
        request(&f, email, "192.0.2.1").await;
    }
    let previous = std::env::var("KEYSEND_MASTER_KEY").ok();
    let previous_admins = std::env::var("ADMIN_EMAILS").ok();
    std::env::set_var("ADMIN_EMAILS", "admin@example.com");
    std::env::remove_var("KEYSEND_MASTER_KEY");
    let credentials =
        keysend::settings::Credentials::resolve(&f.ctx.environment, &f.ctx.config).unwrap();
    f.ctx.shared_store.insert(std::sync::Arc::new(credentials));
    if let Some(value) = previous {
        std::env::set_var("KEYSEND_MASTER_KEY", value);
    }
    match previous_admins {
        Some(value) => std::env::set_var("ADMIN_EMAILS", value),
        None => std::env::remove_var("ADMIN_EMAILS"),
    }
    request(&f, "admin@example.com", "192.0.2.1").await;
    assert!(f.calls.lock().unwrap().emails.is_empty());
}

#[tokio::test]
#[serial]
async fn an_administrator_cannot_sign_in_after_twenty_failed_verifications_from_one_ip() {
    let f = fixture().await;
    for _ in 0..20 {
        verify(&f, &Cookie::new("ks_admin_challenge", "missing"), "000000").await;
    }
    let (cookie, code) = begin(&f).await;
    assert!(verify(&f, &cookie, &code)
        .await
        .text()
        .contains("That code did not work."));
}

#[tokio::test]
#[serial]
async fn strangers_cannot_use_up_the_code_requests_of_an_administrator() {
    let f = fixture().await;
    for n in 0..60 {
        request(
            &f,
            &format!("stranger{n}@example.com"),
            &format!("192.0.2.{n}"),
        )
        .await;
    }
    request(&f, "admin@example.com", "198.51.100.1").await;
    code(&f).await;
    assert_eq!(f.calls.lock().unwrap().emails.len(), 1);
}

#[tokio::test]
#[serial]
async fn a_wrong_code_gets_the_same_answer_whether_or_not_the_address_may_sign_in() {
    let f = fixture().await;
    let (listed, code) = begin(&f).await;
    let wrong = if code == "000000" { "000001" } else { "000000" };
    let stranger = request(&f, "intruder@example.com", "192.0.2.9")
        .await
        .cookie("ks_admin_challenge")
        .into_owned();
    let listed = verify(&f, &listed, wrong).await;
    let stranger = verify(&f, &stranger, wrong).await;
    assert_eq!(listed.status_code(), stranger.status_code());
    assert_eq!(listed.text(), stranger.text());
}

#[tokio::test]
#[serial]
async fn ten_failed_codes_in_a_day_stop_an_address_from_signing_in() {
    let f = fixture().await;
    for _ in 0..2 {
        let (challenge, code) = begin(&f).await;
        let wrong = if code == "000000" { "000001" } else { "000000" };
        for _ in 0..5 {
            verify(&f, &challenge, wrong).await;
        }
    }
    let (challenge, code) = begin(&f).await;
    let response = verify(&f, &challenge, &code).await;
    assert_eq!(response.status_code(), 200);
    assert!(response.maybe_cookie("ks_admin").is_none());
}

#[tokio::test]
#[serial]
async fn an_administrator_receives_a_new_session_and_keeps_the_customer_session_separate() {
    let f = fixture().await;
    let (challenge, code) = begin(&f).await;
    let response = verify(&f, &challenge, &code).await;
    let session = response.cookie("ks_admin");
    assert_ne!(challenge.value(), session.value());
    assert_eq!(
        response
            .cookie("ks_admin_challenge")
            .max_age()
            .unwrap()
            .whole_seconds(),
        0
    );
    assert_eq!(
        f.request
            .get("/settings")
            .add_cookie(session)
            .await
            .status_code(),
        303
    );
    assert_eq!(
        f.request
            .get("/settings")
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        200
    );
}

#[tokio::test]
#[serial]
#[ignore = "serves admin pages for a manual browser check"]
async fn an_administrator_uses_the_sign_in_forms_in_a_browser() {
    let directory = std::path::PathBuf::from(std::env::var("KEYSEND_BROWSER_CHECK_DIR").unwrap());
    let f = fixture().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        directory.join("address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    std::fs::write(directory.join("cookie"), f.a.value()).unwrap();
    let router = f.router.clone();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    while !directory.join("stop").exists() {
        if !f.calls.lock().unwrap().emails.is_empty() {
            std::fs::write(directory.join("code"), code(&f).await).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    server.abort();
}

#[tokio::test]
#[serial]
async fn the_operator_retains_only_unexpired_admin_credentials_and_recent_throttles() {
    let f = fixture().await;
    let now = keysend::settings::wall_clock().timestamp();
    for sql in [
        format!("INSERT INTO admin_challenges VALUES ('old','admin@example.com','mac',0,0),('current','admin@example.com','mac',{},0)",now+600),
        format!("INSERT INTO admin_sessions VALUES ('old','admin@example.com',0,0),('current','admin@example.com',{now},{})",now+14400),
        format!("INSERT INTO admin_throttle VALUES ('old',{},1),('current',{now},1)",now-86401),
    ] { f.ctx.db.execute_unprepared(&sql).await.unwrap(); }
    keysend::monitoring::retention::run(&f.ctx).await.unwrap();
    for table in ["admin_challenges", "admin_sessions", "admin_throttle"] {
        let row = f
            .ctx
            .db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                format!("SELECT COUNT(*) AS n FROM {table}"),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 1, "{table}");
    }
}

#[tokio::test]
#[serial]
async fn an_administrator_session_is_required_to_read_a_protected_admin_page() {
    let f = fixture().await;
    let cookie = signed(&f).await;
    let server = protected(&f, keysend::settings::Settings::load(&f.ctx).unwrap());
    for cookie in [
        None,
        Some(f.a.clone()),
        Some(Cookie::new("ks_admin", "invalid")),
    ] {
        let mut request = server.get("/admin/protected");
        if let Some(cookie) = cookie {
            request = request.add_cookie(cookie);
        }
        let response = request.await;
        assert_eq!(response.status_code(), 303);
        assert_eq!(response.header("location"), "/admin");
    }
    let response = server.get("/admin/protected").add_cookie(cookie).await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.text(), "admin@example.com");
}

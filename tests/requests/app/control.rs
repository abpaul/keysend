use super::*;
use keysend::language::Language;

#[tokio::test]
#[serial]
async fn account_controls_refuse_other_workspaces_and_remove_owned_data() {
    let f = Fixture::monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let connection = store.google_connections().await.unwrap().remove(0).id;
    for target in [
        format!("/remove/workspaces/{}", signed.workspace.id()),
        format!("/remove/connections/{connection}"),
        "/remove/accounts/4829153307".into(),
        "/ad-accounts/4829153307/remove".into(),
    ] {
        assert_eq!(
            f.request
                .get(&target)
                .add_cookie(f.b.clone())
                .await
                .status_code(),
            404,
            "{target}"
        );
        assert_eq!(
            f.request
                .post(&target)
                .add_header("origin", ORIGIN)
                .add_cookie(f.b.clone())
                .form(&json!({"confirm":"remove"}))
                .await
                .status_code(),
            404,
            "{target}"
        );
    }
    assert!(f.calls.lock().unwrap().google.revocations.is_empty());
    let settings = f.page("/settings").await;
    assert!(settings.contains(r#"<h1 class="heading-xl">Your Workspace</h1>"#));
    assert!(settings.contains("a@example.com"));
    assert!(settings.contains(&format!(
        r#"<a href="/remove/connections/{connection}">Remove"#
    )));
    assert!(settings.contains(&format!(
        r#"<a href="/remove/workspaces/{}">Delete my Workspace</a>"#,
        signed.workspace.id()
    )));
    assert!(settings.contains(r#"<a href="/ad-accounts" aria-current="page">Ad Accounts</a>"#));
    assert!(!settings.contains("Report"));
    let removal = f.page(&format!("/remove/connections/{connection}")).await;
    assert!(removal.contains("<h1 class=\"heading-xl\">Remove a@example.com?</h1>"));
    assert!(removal.contains(r#"<input type="checkbox" name="confirm" value="remove" required>"#));
    assert!(removal.contains("<a href=\"/ad-accounts\" aria-current=\"page\">Ad Accounts</a>"));
    let wrong = f
        .request
        .post(&format!("/remove/connections/{connection}"))
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .form(&json!({"confirm":"wrong"}))
        .await;
    assert_eq!(wrong.status_code(), 422);
    assert_eq!(
        f.request
            .get("/remove/accounts/4829153307")
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        404
    );
    assert_eq!(
        f.request
            .post("/remove/accounts/4829153307")
            .add_header("origin", ORIGIN)
            .add_cookie(f.a.clone())
            .form(&json!({"confirm":"remove"}))
            .await
            .status_code(),
        404
    );
    assert_eq!(store.ad_accounts().await.unwrap().len(), 1);
    assert_eq!(
        f.post(&f.a, "/ad-accounts/4829153307/remove", json!({}))
            .await
            .status_code(),
        303
    );
    assert!(store.ad_accounts().await.unwrap().is_empty());
    assert!(store.branding("4829153307").await.unwrap().is_none());
    assert!(store.logo("4829153307").await.unwrap().is_none());
    assert_eq!(store.google_connections().await.unwrap().len(), 1);
}

#[tokio::test]
#[serial]
async fn google_connection_removal_revokes_permission_and_preserves_data_on_failure() {
    let f = Fixture::new(false).await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let id = store.google_connections().await.unwrap().remove(0).id;
    let target = format!("/remove/connections/{id}");
    f.calls.lock().unwrap().google.revoke_failure = true;
    assert_eq!(
        f.request
            .post(&target)
            .add_header("origin", ORIGIN)
            .add_cookie(f.a.clone())
            .form(&json!({"confirm":"remove"}))
            .await
            .status_code(),
        503
    );
    assert_eq!(store.google_connections().await.unwrap().len(), 1);
    assert_eq!(store.ad_accounts().await.unwrap().len(), 1);
    f.calls.lock().unwrap().google.revoke_failure = false;
    assert_eq!(
        f.request
            .post(&target)
            .add_header("origin", ORIGIN)
            .add_cookie(f.a.clone())
            .form(&json!({"confirm":"remove"}))
            .await
            .status_code(),
        303
    );
    assert_eq!(
        f.calls.lock().unwrap().google.revocations[1]["token"],
        "never-send-this-refresh-secret"
    );
    assert!(store.google_connections().await.unwrap().is_empty());
    assert!(store.ad_accounts().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn delete_user_revokes_google_deletes_stripe_customer_and_invalidates_sessions() {
    let f = Fixture::new(false).await;
    f.subscribe().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let target = format!("/remove/workspaces/{}", signed.workspace.id());
    assert_eq!(
        f.request
            .get(&target)
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        200
    );
    assert_eq!(
        f.request
            .post(&target)
            .add_header("origin", ORIGIN)
            .add_cookie(f.a.clone())
            .form(&json!({"confirm":"remove"}))
            .await
            .status_code(),
        303
    );
    assert!(identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.request
            .get("/settings")
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        303
    );
    assert_eq!(
        f.request
            .get("/settings")
            .add_cookie(f.b.clone())
            .await
            .status_code(),
        200
    );
    assert_eq!(f.calls.lock().unwrap().google.revocations.len(), 1);
    assert!(f
        .calls
        .lock()
        .unwrap()
        .stripe
        .requests
        .iter()
        .any(|(route, _)| route.starts_with("DELETE customers/cus_")));
}

#[tokio::test]
#[serial]
async fn error_ingest_contains_only_fixed_descriptions_and_valid_envelope() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let f = Fixture::configured(false, false, false).await;
    assert_eq!(
        f.request
            .post("/subscription")
            .add_header("origin", ORIGIN)
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        503
    );
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    for secret in [
        "stub-postdeploy-secret",
        "stub-openai-secret",
        "never-send-this-refresh-secret",
        f.a.value(),
    ] {
        assert!(!logs.contains(secret));
    }
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.errors.len(), 1);
    let body = &calls.errors[0];
    assert_eq!(body["header"]["sdk"]["platform"], "rust");
    assert_eq!(body["items"][0]["message"], "HTTP request failed");
    assert_eq!(body["items"][0]["id"].as_str().unwrap().len(), 32);
    assert_eq!(body["items"][0]["type"], "message");
    assert_eq!(body["items"][0]["level"], "error");
    let text = body.to_string();
    for secret in [
        "a@example.com",
        "never-send-this-refresh-secret",
        "stub-openai-secret",
        "google-client-secret",
        "4829153307",
    ] {
        assert!(!text.contains(secret));
    }
}

#[tokio::test]
#[serial]
async fn the_report_pages_and_routes_are_gone() {
    let f = Fixture::new(false).await;
    for path in [
        "/reports",
        "/reports/new",
        "/reports/anything",
        "/reports/anything/exports/pdf",
        "/accounts/4829153307/branding",
    ] {
        let response = f.request.get(path).add_cookie(f.a.clone()).await;
        assert_eq!(response.status_code(), 404, "{path}");
        assert!(response.text().contains("Page not found"), "{path}");
    }
    for path in ["/reports", "/reports/anything/unlock"] {
        assert_eq!(
            f.post(&f.a, path, json!({})).await.status_code(),
            404,
            "{path}"
        );
    }
}

#[tokio::test]
#[serial]
async fn the_settings_page_reads_its_texts_in_the_language_of_the_user() {
    let f = Fixture::monitoring().await;
    let settings = f.page("/settings").await;
    assert!(settings.starts_with("<!doctype html>\n<html lang=\"en\">"));
    assert!(settings.contains("<title>Your Workspace | KeySend</title>"));
    assert!(settings
        .contains(r#"<a class="back-link" href="/ad-accounts">Back to the Ad Accounts</a>"#));
    assert!(
        settings.contains("<p class=\"small muted\">1 Google Connection in this Workspace.</p>")
    );
    assert!(settings.contains("You signed in as a@example.com. We delete your Workspace"));
    assert!(settings.contains("action=\"/settings/language\""));
    for name in [
        "English",
        "Română",
        "Français",
        "Deutsch",
        "Español",
        "Italiano",
        "Português (Brasil)",
    ] {
        assert!(settings.contains(&format!(">{name}</")), "{name}");
    }
}

#[tokio::test]
#[serial]
async fn a_user_chooses_only_an_offered_language() {
    let f = Fixture::monitoring().await;
    let saved = f
        .post(&f.a, "/settings/language", json!({"language":"en"}))
        .await;
    assert_eq!(saved.status_code(), 303);
    assert_eq!(saved.header("location"), "/settings");
    for refused in ["xx", "EN", "pt-BR", ""] {
        let response = f
            .post(&f.a, "/settings/language", json!({"language":refused}))
            .await;
        assert_eq!(response.status_code(), 422, "{refused}");
        assert_eq!(response.text(), "Choose one of the languages in the list.");
    }
    let foreign = f
        .request
        .post("/settings/language")
        .add_cookie(f.a.clone())
        .form(&json!({"language":"en"}))
        .await;
    assert_eq!(foreign.status_code(), 403);
    let stranger = f
        .request
        .post("/settings/language")
        .add_header("origin", ORIGIN)
        .form(&json!({"language":"en"}))
        .await;
    assert_eq!(stranger.status_code(), 303);
    assert_eq!(stranger.header("location"), "/sign-in");
    assert_eq!(signed(&f.ctx, &f.a).await.user.language, Language::En);
    for language in Language::ALL {
        let saved = f
            .post(
                &f.a,
                "/settings/language",
                json!({"language":language.code()}),
            )
            .await;
        assert_eq!(saved.status_code(), 303, "{}", language.code());
        assert_eq!(signed(&f.ctx, &f.a).await.user.language, language);
    }
}

#[tokio::test]
#[serial]
async fn the_store_keeps_the_language_of_one_user_only() {
    let f = Fixture::monitoring().await;
    let a = signed(&f.ctx, &f.a).await;
    let b = signed(&f.ctx, &f.b).await;
    assert_eq!(a.user.language, Language::En);
    for language in Language::ALL {
        WorkspaceStore::new(&f.ctx.db, &a.workspace)
            .set_user_language(language)
            .await
            .unwrap();
        assert_eq!(signed(&f.ctx, &f.a).await.user.language, language);
        assert_eq!(signed(&f.ctx, &f.b).await.user.language, Language::En);
    }
    assert_eq!(b.user.language, Language::En);
}

#[tokio::test]
#[serial]
async fn a_user_with_another_language_reads_the_settings_in_it_and_the_home_page_stays_english() {
    let f = Fixture::monitoring().await;
    let a = signed(&f.ctx, &f.a).await;
    WorkspaceStore::new(&f.ctx.db, &a.workspace)
        .set_user_language(Language::Ro)
        .await
        .unwrap();
    let response = f.request.get("/settings").add_cookie(f.a.clone()).await;
    assert_eq!(response.status_code(), 200);
    let page = response.text();
    assert!(page.starts_with("<!doctype html>\n<html lang=\"ro\">"));
    assert!(page.contains("<title>Spațiul dumneavoastră de lucru | KeySend</title>"));
    assert!(!page.contains("Your Workspace"));
    let home = f.request.get("/").await;
    assert_eq!(home.status_code(), 200);
    assert!(home.text().contains("<html lang=\"en\">"));
}

async fn signed(ctx: &AppContext, cookie: &Cookie<'static>) -> identity::SignedIn {
    identity::authenticate(&ctx.db, cookie.value())
        .await
        .unwrap()
        .unwrap()
}

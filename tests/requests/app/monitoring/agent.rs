use super::*;
use keysend::agent::{
    data::{self, Scope},
    operation::{Operation, Period},
    run::{run, Refused},
    Handle,
};

#[tokio::test]
#[serial]
async fn a_users_investigation_cannot_include_an_ad_account_of_another_workspace() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let user = signed(&f).await;
    let other = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    let a = WorkspaceStore::new(&f.ctx.db, &user.workspace);
    let b = WorkspaceStore::new(&f.ctx.db, &other.workspace);
    let connection = b
        .save_google_connection(
            "b",
            "b@example.com",
            "test-refresh-token",
            &MasterKey::from_base64(KEY).unwrap(),
        )
        .await
        .unwrap();
    let foreign = "0000000002".to_owned();
    let selection = b
        .create_selection(
            &connection.id,
            vec![AdAccountChoice {
                customer_id: foreign.clone(),
                name: "Other Workspace".into(),
                login_customer_id: None,
            }],
        )
        .await
        .unwrap();
    assert!(b
        .save_selected(&selection, std::slice::from_ref(&foreign))
        .await
        .unwrap());
    b.save_ad_account_metadata(&foreign, "USD", "UTC")
        .await
        .unwrap();
    let scope = Scope::load(&a, &[AD.into(), foreign.clone()])
        .await
        .unwrap();
    let mut snapshots = data::load_scope(&a, &scope, day_number(end()))
        .await
        .unwrap();
    let rows = run(
        &Operation::RankAccounts {
            period: Period::Last30Days,
        },
        1,
        &scope,
        &snapshots,
    )
    .unwrap();
    assert_eq!(rows[0].facts["Ad Account"], "Northwind Dental");
    assert_eq!(rows[0].facts["Spend"], "EUR 300");
    assert!(rows
        .iter()
        .all(|r| r.facts["Ad Account"] != "Other Workspace"));
    assert!(data::load(&a, &foreign, day_number(end())).await.is_err());
    let outside = Handle::new(1);
    snapshots.insert(
        outside,
        data::load(&b, &foreign, day_number(end())).await.unwrap(),
    );
    assert_eq!(
        run(
            &Operation::AccountOverview {
                account: outside,
                period: Period::Last30Days
            },
            2,
            &scope,
            &snapshots
        ),
        Err(Refused::UnknownAdAccount(outside))
    );
    let foreign_scope = Scope::load(&a, &[foreign]).await.unwrap();
    assert!(run(
        &Operation::RankAccounts {
            period: Period::Last30Days
        },
        3,
        &foreign_scope,
        &snapshots
    )
    .unwrap()
    .is_empty());
}

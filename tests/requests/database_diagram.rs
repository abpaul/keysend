use std::collections::{BTreeMap, BTreeSet};

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};

const DIAGRAM: &str = "docs/database.md";
const WRITE: &str = "KEYSEND_WRITE_DIAGRAMS";
const HUB: &str = "ad_accounts";

async fn rows(db: &DatabaseConnection, query: &str, columns: &[&str]) -> Vec<Vec<String>> {
    db.query_all_raw(Statement::from_string(DatabaseBackend::Sqlite, query))
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|column| row.try_get("", column).unwrap())
                .collect()
        })
        .collect()
}

async fn diagram(db: &DatabaseConnection) -> String {
    let tables: Vec<String> = rows(
        db,
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'seaql_%' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        &["name"],
    )
    .await
    .into_iter()
    .flatten()
    .collect();
    let mut keys: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows(
        db,
        "SELECT m.name AS child, CAST(p.id AS TEXT) AS id, p.\"table\" AS parent, p.\"from\" AS column_name, p.on_delete AS on_delete FROM sqlite_master m, pragma_foreign_key_list(m.name) p WHERE m.type='table' ORDER BY m.name, p.id, p.seq",
        &["child", "id", "parent", "column_name", "on_delete"],
    )
    .await
    {
        let [child, id, parent, column, on_delete] = row.try_into().unwrap();
        assert_eq!(on_delete, "CASCADE", "{child} keeps rows of a deleted {parent}");
        parents.entry(child.clone()).or_default().insert(parent.clone());
        keys.entry((parent, child, id)).or_default().push(column);
    }
    let referred: BTreeSet<&String> = keys.keys().map(|(parent, _, _)| parent).collect();
    let of_one_account = |table: &String| {
        !referred.contains(table)
            && parents
                .get(table)
                .is_some_and(|parents| parents.len() == 1 && parents.contains(HUB))
    };

    let mut page = format!(
        "# KeySend database\n\nA test writes this page from the migrated schema (`tests/requests/database_diagram.rs`). Do not edit it. After a migration, write it again:\n\n```sh\n{WRITE}=1 cargo test --locked --test mod database_diagram\n```\n\nEach line is a foreign key, with its columns. Every foreign key deletes its rows with the parent row. The diagram does not show which relations are one-to-one.\n\n```mermaid\nerDiagram\n"
    );
    for ((parent, child, _), columns) in &keys {
        if !of_one_account(child) {
            page.push_str(&format!(
                "  {parent} ||--o{{ {child} : \"{}\"\n",
                columns.join(", ")
            ));
        }
    }
    for table in &tables {
        if !parents.contains_key(table) && !referred.contains(table) {
            page.push_str(&format!("  {table}\n"));
        }
    }
    page.push_str(&format!(
        "```\n\n## Tables of one Ad Account\n\nEach of these tables has one foreign key, to `{HUB}`, and no table refers to it.\n\n| Table | Primary key |\n|---|---|\n"
    ));
    for table in tables.iter().filter(|table| of_one_account(table)) {
        let primary: Vec<String> = rows(
            db,
            &format!("SELECT name FROM pragma_table_info('{table}') WHERE pk>0 ORDER BY pk"),
            &["name"],
        )
        .await
        .into_iter()
        .flatten()
        .collect();
        page.push_str(&format!("| `{table}` | `{}` |\n", primary.join("`, `")));
    }
    page
}

#[tokio::test]
async fn the_database_page_shows_the_tables_and_foreign_keys_of_the_migrated_schema() {
    let path = std::env::temp_dir().join(format!(
        "keysend-diagram-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    let page = diagram(&db).await;
    db.close().await.unwrap();
    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{}{suffix}", path.display())).ok();
    }
    if std::env::var_os(WRITE).is_some() {
        std::fs::write(DIAGRAM, &page).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(DIAGRAM).unwrap_or_default(),
        page,
        "{DIAGRAM} does not match the schema. Run: {WRITE}=1 cargo test --locked --test mod database_diagram"
    );
}

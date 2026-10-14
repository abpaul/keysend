use std::path::Path;

use keysend::app::App;
use loco_rs::{app::Hooks, testing::prelude::*};
use serial_test::serial;

const FEATURE_MAP: &str = "docs/features.md";
const SOURCE_ROOTS: [&str; 7] = [
    "src/",
    "tests/",
    "scripts/",
    "assets/",
    "migration/",
    "config/",
    "content/",
];

fn named(map: &str) -> Vec<&str> {
    map.split('`').skip(1).step_by(2).collect()
}

fn covers(name: &str, route: &str) -> bool {
    match name.strip_suffix("/*").filter(|prefix| !prefix.is_empty()) {
        Some(prefix) => route
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/')),
        None => name == route,
    }
}

#[tokio::test]
#[serial]
async fn the_feature_map_names_every_route_and_worker_and_only_paths_that_exist() {
    request::<App, _, _>(|_request, ctx| async move {
        let map = std::fs::read_to_string(FEATURE_MAP).unwrap();
        let named = named(&map);

        let missing: Vec<String> = App::routes(&ctx)
            .collect()
            .into_iter()
            .map(|route| route.uri)
            .filter(|route| !named.iter().any(|name| covers(name, route)))
            .collect();
        assert_eq!(missing, Vec::<String>::new(), "routes not in {FEATURE_MAP}");

        let app = std::fs::read_to_string("src/app.rs").unwrap();
        let workers: Vec<&str> = app
            .split("::build(ctx)")
            .filter_map(|before| before.rsplit("::").next())
            .filter(|name| name.ends_with("Worker"))
            .collect();
        assert!(!workers.is_empty(), "no worker was read from src/app.rs");
        for worker in workers {
            assert!(named.contains(&worker), "{worker} is not in {FEATURE_MAP}");
        }

        let absent: Vec<&str> = named
            .iter()
            .filter(|name| SOURCE_ROOTS.iter().any(|root| name.starts_with(root)))
            .filter(|name| !Path::new(name).exists())
            .copied()
            .collect();
        assert_eq!(absent, Vec::<&str>::new(), "paths that do not exist");
    })
    .await;
}

use std::path::{Path, PathBuf};

const DATA_LAYER: [&str; 3] = ["src/workspace", "src/identity", "src/database.rs"];
const DATABASE_TOKENS: [&str; 13] = [
    "sea_orm",
    "sqlx",
    "query_one",
    "query_all",
    "execute_raw",
    "execute_unprepared",
    ".execute(",
    "Statement::",
    "ActiveModel",
    "Entity::",
    ".begin(",
    "DatabaseConnection",
    "DatabaseTransaction",
];

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if DATA_LAYER.iter().any(|layer| path == Path::new(layer)) {
            continue;
        }
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn only_the_data_layer_talks_to_the_database() {
    let mut files = Vec::new();
    rust_files(Path::new("src"), &mut files);
    assert!(files.len() > 50, "src was not read");
    let mut found = Vec::new();
    for file in files {
        for (number, line) in std::fs::read_to_string(&file).unwrap().lines().enumerate() {
            if let Some(token) = DATABASE_TOKENS.iter().find(|token| line.contains(**token)) {
                found.push(format!("{}:{}: {token}", file.display(), number + 1));
            }
        }
    }
    assert_eq!(
        found,
        Vec::<String>::new(),
        "database access outside {DATA_LAYER:?}"
    );
}

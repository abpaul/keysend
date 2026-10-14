use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use loco_rs::{environment::Environment, Error, Result};
use sha2::{Digest, Sha256};

pub fn cache_dir(environment: &Environment) -> PathBuf {
    if *environment == Environment::Production {
        PathBuf::from("/data/cache/assets")
    } else {
        PathBuf::from("assets/.cache")
    }
}

pub fn prepare(source: &Path, destination: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = Vec::new();
    collect(source, source, &mut files)?;
    let mut urls = BTreeMap::new();
    for path in files
        .iter()
        .filter(|path| path.extension().is_none_or(|ext| ext != "css"))
    {
        let bytes = fs::read(source.join(path))?;
        urls.insert(path_string(path)?, store(destination, path, &bytes)?);
    }
    for path in files
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "css"))
    {
        let mut css = fs::read_to_string(source.join(path))?;
        let parent = path.parent().unwrap_or(Path::new(""));
        for (name, url) in &urls {
            if let Ok(relative) = Path::new(name).strip_prefix(parent) {
                let relative = path_string(relative)?;
                for expression in [
                    format!("url(\"{relative}\")"),
                    format!("url('{relative}')"),
                    format!("url({relative})"),
                ] {
                    css = css.replace(&expression, &format!("url(\"{url}\")"));
                }
            }
        }
        urls.insert(
            path_string(path)?,
            store(destination, path, css.as_bytes())?,
        );
    }
    Ok(urls)
}

fn collect(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect(root, &entry.path(), files)?;
        } else if entry.file_type()?.is_file() {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| Error::string("Invalid static asset path"))?
                    .to_owned(),
            );
        }
    }
    files.sort();
    Ok(())
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::string("Static asset path must be UTF-8"))
}

fn store(destination: &Path, name: &Path, bytes: &[u8]) -> Result<String> {
    let digest = format!("{:x}", Sha256::digest(bytes));
    let relative = Path::new(&digest).join(name);
    let path = destination.join(&relative);
    if path.exists() {
        if fs::read(&path)? != bytes {
            return Err(Error::string("Immutable static asset content mismatch"));
        }
    } else {
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| Error::string("Invalid static asset directory"))?,
        )?;
        let temporary = path.with_extension(format!("{}.tmp", crate::identity::random_token()));
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, &path)?;
    }
    Ok(format!("/static/{}", path_string(&relative)?))
}

pub fn is_fingerprinted(path: &str) -> bool {
    let Some(relative) = path.strip_prefix("/static/") else {
        return false;
    };
    let Some((digest, name)) = relative.split_once('/') else {
        return false;
    };
    digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !name.is_empty()
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_fonts_change_css_urls_and_older_assets_survive_three_releases() {
        let root = std::env::temp_dir().join(format!(
            "keysend-assets-{}",
            crate::identity::random_token()
        ));
        let source = root.join("source");
        let destination = root.join("archive");
        fs::create_dir_all(source.join("fonts")).unwrap();
        fs::write(
            source.join("styles.css"),
            "@font-face { src: url(\"fonts/test.woff2\"); }",
        )
        .unwrap();
        let mut releases = Vec::new();
        for value in ["first font", "second font", "third font"] {
            fs::write(source.join("fonts/test.woff2"), value).unwrap();
            releases.push(prepare(&source, &destination).unwrap());
        }
        for (index, release) in releases.iter().enumerate() {
            let font = &release["fonts/test.woff2"];
            let css = &release["styles.css"];
            assert_eq!(
                fs::read_to_string(destination.join(font.strip_prefix("/static/").unwrap()))
                    .unwrap(),
                ["first font", "second font", "third font"][index]
            );
            assert_eq!(
                fs::read_to_string(destination.join(css.strip_prefix("/static/").unwrap()))
                    .unwrap(),
                format!("@font-face {{ src: url(\"{font}\"); }}")
            );
            assert!(is_fingerprinted(css));
        }
        assert_ne!(releases[0]["styles.css"], releases[1]["styles.css"]);
        assert_ne!(releases[1]["styles.css"], releases[2]["styles.css"]);
        fs::remove_dir_all(root).unwrap();
    }
}

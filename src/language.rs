//! The language of the interface and of the Export.
//!
//! A language is a `Text`: one value in `language/<code>.rs` with every text of the interface.
//! A text that is missing does not compile, and a language without a `Text` is not in
//! `Language::available()`. The language of a request is the language of the signed-in user; a
//! page that nobody signed in to is English.
//!
//! How to write a text:
//! - A plain text in a template: add a line to `Text` and to `en.rs`, then write
//!   `{{ t(key="save") }}`.
//! - A text with values: put `{name}` in the text, then write
//!   `{{ t(key="delete_workspace_summary", email=email) }}`.
//! - A plural: add it to `plurals` in `Text` with `Plural::two` or `Plural::three`, use `{count}`
//!   in the forms, then write `{{ t(key="google_connections_count", count=n) }}`. The `Rule` of
//!   the language picks the form.
//! - A text that Rust writes when it renders: add a line or a plural to `written!` in
//!   `written.rs` and its value to `en_written.rs`. The handler takes
//!   `signed.user.language.text()?` and gives the `&Text` to the builder; a builder never reads the
//!   language of the request, so a background worker and a blocking task write the same texts.
//!   Write `text.written.not_available` for a plain text, `say!(text, change_up, percent = p)?` for
//!   values and `say_plural!(text, period_days, n)?` for a plural. A number, an amount or a date
//!   goes through `text.number`, `text.money`, `text.count` or `text.written.format`.
//! - A sentence with a link or a bold word has a placeholder for it. The template writes
//!   `{% set link %}<a href="...">{{ t(key="settings") }}</a>{% endset %}` and gives `link` as a
//!   value, so that a language puts the link where its sentence needs it.
//! - A page that nobody signed in to reads its texts in the same way, in the language that the
//!   request scope starts with. A handler of such a page that builds a text in Rust takes it from
//!   `request_text()`. The words of the Privacy, Terms and Legal pages are the one exception: a
//!   lawyer owns them, so they stay in their templates and the guard test names them.
//!
//! The `marker-language` feature adds a test language, `Language::Pt` in the tests, whose texts
//! are their keys and whose numbers and dates differ from English. A test that renders a page, an
//! Export or an email in it finds each English word that the language table does not write.
//!
//! An email renders outside a request, so the worker runs it inside `with_language` with the
//! language of the recipient; `t` called outside a scope is an error.
//!
//! An unknown key, a placeholder without a value and a value without a placeholder are errors.
//!
//! A text is HTML that the program owns, so `t` gives it to the template as safe. `t` escapes each
//! value on its own before it fills a placeholder, so a name that has `<` in it stays text. A text
//! never holds `<`, `>` or `"`; a test checks this.

pub mod de;
mod de_written;
pub mod en;
mod en_written;
pub mod es;
mod es_written;
pub mod format;
pub mod fr;
mod fr_written;
pub mod it;
mod it_written;
#[cfg(feature = "marker-language")]
pub mod marker;
pub mod pt;
mod pt_written;
pub mod ro;
mod ro_written;
mod text;
mod written;

use std::cell::Cell;

use axum::{extract::Request, middleware::Next, response::Response};
use loco_rs::{Error, Result};
use serde::Serialize;
use tera::{Kwargs, State, Tera, TeraResult, Value};

pub use text::{fill, Entry, Form, Plural, Rule, Text};
pub use written::Written;
pub(crate) use written::{say, say_amount, say_plural};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Ro,
    Fr,
    De,
    Es,
    It,
    Pt,
}

impl Language {
    pub const ALL: [Language; 7] = [
        Language::En,
        Language::Ro,
        Language::Fr,
        Language::De,
        Language::Es,
        Language::It,
        Language::Pt,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Ro => "ro",
            Language::Fr => "fr",
            Language::De => "de",
            Language::Es => "es",
            Language::It => "it",
            Language::Pt => "pt",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::Ro => "Română",
            Language::Fr => "Français",
            Language::De => "Deutsch",
            Language::Es => "Español",
            Language::It => "Italiano",
            Language::Pt => "Português (Brasil)",
        }
    }

    pub fn from_code(code: &str) -> Option<Language> {
        Language::ALL
            .into_iter()
            .find(|language| language.code() == code)
    }

    fn translation(self) -> Option<&'static Text> {
        match self {
            Language::En => Some(&en::TEXT),
            Language::Ro => Some(&ro::TEXT),
            Language::Fr => Some(&fr::TEXT),
            Language::De => Some(&de::TEXT),
            Language::Es => Some(&es::TEXT),
            Language::It => Some(&it::TEXT),
            Language::Pt => Some(&pt::TEXT),
        }
    }

    pub fn available() -> Vec<Language> {
        Language::ALL
            .into_iter()
            .filter(|language| language.translation().is_some())
            .collect()
    }

    pub fn parse_available(code: &str) -> Option<Language> {
        Language::from_code(code).filter(|language| language.translation().is_some())
    }

    pub fn text(self) -> Result<&'static Text> {
        #[cfg(feature = "marker-language")]
        if self == marker::LANGUAGE {
            return Ok(marker::text());
        }
        self.translation().ok_or_else(|| {
            Error::Message(format!("The language `{}` has no text yet", self.code()))
        })
    }
}

tokio::task_local! {
    static CURRENT: Cell<Language>;
}

pub async fn scope_request(request: Request, next: Next) -> Response {
    CURRENT
        .scope(Cell::new(Language::En), next.run(request))
        .await
}

pub fn with_language<R>(language: Language, render: impl FnOnce() -> R) -> R {
    CURRENT.sync_scope(Cell::new(language), render)
}

pub fn set_current(language: Language) -> Result<()> {
    CURRENT
        .try_with(|current| current.set(language))
        .map_err(|_| Error::Message("The language of the request is not scoped".into()))
}

pub fn request_text() -> Result<&'static Text> {
    current()
        .map_err(|error| Error::Message(error.to_string()))?
        .text()
}

pub fn english() -> &'static Text {
    &en::TEXT
}

fn current() -> TeraResult<Language> {
    CURRENT
        .try_with(Cell::get)
        .map_err(|_| tera::Error::message("`t` was called outside of a language scope"))
}

fn t(kwargs: Kwargs, state: &State) -> TeraResult<Value> {
    let text = current()?
        .text()
        .map_err(|error| tera::Error::message(error.to_string()))?;
    let mut key = None;
    let mut values = Vec::new();
    for (name, value) in kwargs.iter() {
        let name = name
            .as_str()
            .ok_or_else(|| tera::Error::message("`t` takes named values only"))?;
        if name == "key" {
            key = value.as_str();
        } else {
            let value = match value.as_str() {
                Some(string) if state.autoescaping_enabled() && !value.is_safe() => {
                    state.escape(string)?
                }
                Some(string) => string.to_owned(),
                None if value.is_number() => value.to_string(),
                None => {
                    return Err(tera::Error::message(format!(
                        "`{name}` must be a string or a number"
                    )))
                }
            };
            values.push((name, value));
        }
    }
    let key = key.ok_or_else(|| tera::Error::message("`t` needs a `key` string"))?;
    text.get(key, &values)
        .map(|text| Value::safe_string(&text))
        .map_err(|error| tera::Error::message(error.to_string()))
}

fn lang(_kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    Ok(Value::from(current()?.code()))
}

fn languages(_kwargs: Kwargs, _state: &State) -> TeraResult<Value> {
    let current = current()?;
    Ok(Value::from(
        Language::available()
            .into_iter()
            .map(|language| {
                let mut entry = tera::Map::new();
                entry.insert("code".into(), Value::from(language.code()));
                entry.insert("name".into(), Value::from(language.name()));
                entry.insert("selected".into(), Value::from(language == current));
                Value::from(entry)
            })
            .collect::<Vec<_>>(),
    ))
}

pub fn register(tera: &mut Tera) {
    tera.register_function("t", t);
    tera.register_function("lang", lang);
    tera.register_function("languages", languages);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn views(directory: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                views(&path, found);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "html")
            {
                found.push(path);
            }
        }
    }

    fn head(source: &str) -> String {
        source.chars().take(20).collect()
    }

    fn used_keys(source: &str) -> Vec<Result<String, String>> {
        let mut keys = Vec::new();
        for (at, _) in source.match_indices("t(") {
            let before = source[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '"')) {
                continue;
            }
            let call = &source[at + 2..];
            let Some(rest) = call.trim_start().strip_prefix("key") else {
                continue;
            };
            let rest = rest.trim_start().strip_prefix('=').map(str::trim_start);
            keys.push(match rest.and_then(|rest| rest.strip_prefix('"')) {
                Some(rest) => rest
                    .split_once('"')
                    .map(|(key, _)| key.to_owned())
                    .ok_or_else(|| format!("unterminated key near `{}`", head(call))),
                None => Err(format!("key is not a string near `{}`", head(call))),
            });
        }
        keys
    }

    #[test]
    fn every_text_key_that_a_template_uses_exists() {
        let mut files = Vec::new();
        views(Path::new("assets/views"), &mut files);
        assert!(!files.is_empty());
        let mut used = 0;
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            for key in used_keys(&source) {
                let key = key.unwrap_or_else(|error| panic!("{}: {error}", file.display()));
                assert!(
                    Text::KEYS.contains(&key.as_str()),
                    "{}: unknown text key `{key}`",
                    file.display()
                );
                used += 1;
            }
        }
        assert!(used > 0);
    }

    #[test]
    fn the_scanner_finds_literal_keys_and_refuses_other_keys() {
        let found = used_keys(
            r#"{{ t(key="a") }} {{ x | set(key="b") }} {{ t(key = "c", n=1) }} {{ t(key=name) }}"#,
        );
        assert_eq!(found.len(), 3);
        assert_eq!(found[0], Ok("a".to_owned()));
        assert_eq!(found[1], Ok("c".to_owned()));
        assert!(found[2].is_err());
    }

    #[test]
    fn every_language_text_has_the_placeholders_of_the_english_text() {
        let english = Language::En.text().unwrap();
        for language in Language::available() {
            let text = language.translation().unwrap();
            for key in Text::KEYS {
                let names = |text: &'static Text| -> Vec<Vec<&'static str>> {
                    let templates = match text.entry(key).unwrap() {
                        Entry::Line(line) => vec![line],
                        Entry::Plural(plural) => vec![plural.one, plural.few, plural.other],
                    };
                    templates
                        .into_iter()
                        .map(|template| {
                            let mut names =
                                super::text::placeholders(template).unwrap_or_else(|| {
                                    panic!("{} `{key}` has a stray brace", language.code())
                                });
                            names.sort_unstable();
                            names.dedup();
                            names.retain(|name| *name != "count");
                            names
                        })
                        .collect()
                };
                let expected = names(english)[0].clone();
                for found in names(text) {
                    assert_eq!(found, expected, "{} `{key}`", language.code());
                }
            }
        }
    }

    #[test]
    fn the_codes_and_names_of_the_languages() {
        let codes = Language::ALL.map(Language::code);
        assert_eq!(codes, ["en", "ro", "fr", "de", "es", "it", "pt"]);
        let names = Language::ALL.map(Language::name);
        assert_eq!(
            names,
            [
                "English",
                "Română",
                "Français",
                "Deutsch",
                "Español",
                "Italiano",
                "Português (Brasil)"
            ]
        );
        for language in Language::ALL {
            assert_eq!(Language::from_code(language.code()), Some(language));
        }
        assert_eq!(Language::from_code("EN"), None);
        assert_eq!(Language::from_code("pt-BR"), None);
        assert_eq!(Language::default(), Language::En);
    }

    #[test]
    fn only_a_language_with_a_text_is_available() {
        assert_eq!(Language::available(), Language::ALL.to_vec());
        assert_eq!(Language::parse_available("en"), Some(Language::En));
        assert_eq!(Language::parse_available("ro"), Some(Language::Ro));
        assert_eq!(Language::parse_available("xx"), None);
        assert_eq!(Language::parse_available(""), None);
        assert_eq!(Language::Ro.text().unwrap().save, "Salvați");
    }

    fn render(template: &str) -> tera::TeraResult<String> {
        let mut tera = Tera::default();
        register(&mut tera);
        tera.add_raw_template("page.html", template)?;
        let mut context = tera::Context::new();
        context.insert("email", "a<b>@example.com");
        context.insert("n", &2);
        with_language(Language::En, || tera.render("page.html", &context))
    }

    #[test]
    fn templates_read_plain_texts_values_and_plurals() {
        assert_eq!(render(r#"{{ t(key="save") }}"#).unwrap(), "Save");
        assert_eq!(
            render(r#"{{ t(key="delete_workspace_summary", email=email) }}"#).unwrap(),
            "You signed in as a&lt;b&gt;@example.com. We delete your Workspace with its Google Connections, Ad Accounts, Account history, Updates, Recommendations and Branding. We remove Google's permission and cancel your Subscription."
        );
        assert_eq!(
            render(r#"{{ t(key="google_connections_count", count=1) }}"#).unwrap(),
            "1 Google Connection in this Workspace."
        );
        assert_eq!(
            render(r#"{{ t(key="google_connections_count", count=n) }}"#).unwrap(),
            "2 Google Connections in this Workspace."
        );
        assert_eq!(render("{{ lang() }}").unwrap(), "en");
    }

    #[test]
    fn templates_escape_each_value_and_keep_the_text() {
        assert_eq!(
            render(r#"{{ t(key="back_to", name="<i>A</i> & 'B'") }}"#).unwrap(),
            "Back to &lt;i&gt;A&lt;/i&gt; &amp; &#39;B&#39;"
        );
        assert_eq!(
            render(r#"{{ t(key="back_to", name="<b>A</b>" | safe) }}"#).unwrap(),
            "Back to <b>A</b>"
        );
        assert_eq!(
            render(r#"{{ t(key="update_today") }}"#).unwrap(),
            "Today's Update"
        );
    }

    #[test]
    fn no_text_holds_markup_or_a_double_quote() {
        for language in Language::available() {
            let text = language.translation().unwrap();
            for key in Text::KEYS {
                let templates = match text.entry(key).unwrap() {
                    Entry::Line(line) => vec![line],
                    Entry::Plural(plural) => vec![plural.one, plural.few, plural.other],
                };
                for template in templates {
                    assert!(
                        !template.contains(['<', '>', '"']),
                        "{} `{key}` holds markup or a double quote",
                        language.code()
                    );
                }
            }
        }
    }

    fn without_spans(source: &str, open: &str, close: &str) -> String {
        let mut kept = String::new();
        let mut rest = source;
        while let Some(start) = rest.find(open) {
            let Some(length) = rest[start + open.len()..].find(close) else {
                break;
            };
            kept.push_str(&rest[..start]);
            rest = &rest[start + open.len() + length + close.len()..];
        }
        kept.push_str(rest);
        kept
    }

    const NAMES: [&str; 5] = ["KeySend", "Rust", "Loco", "postdeploy", "OVHcloud"];

    const LEGAL_BODIES: [&str; 3] = [
        "assets/views/home/privacy.html",
        "assets/views/home/terms.html",
        "assets/views/home/legal.html",
    ];

    fn has_words(text: &str) -> bool {
        NAMES
            .iter()
            .fold(without_spans(text, "&", ";"), |text, name| {
                text.replace(name, "")
            })
            .chars()
            .any(char::is_alphabetic)
    }

    fn fixed_word(source: &str) -> Option<String> {
        let mut source = without_spans(source, "{#", "#}");
        for block in [
            "robots",
            "head",
            "body_attrs",
            "page_class",
            "nav_accounts",
            "nav_subscription",
        ] {
            source = without_spans(
                &source,
                &format!("{{% block {block} %}}"),
                &format!("{{% endblock {block} %}}"),
            );
        }
        let source = without_spans(&source, "{{", "}}");
        let source = without_spans(&source, "{%", "%}");
        let source = without_spans(&source, "<script", "</script>");
        let source = without_spans(&source, "<style", "</style>");
        let mut visible = String::new();
        let mut rest = source.as_str();
        while let Some(start) = rest.find('<') {
            visible.push_str(&rest[..start]);
            visible.push('\n');
            let Some(length) = rest[start..].find('>') else {
                return Some(rest[start..].to_owned());
            };
            let tag = &rest[start..start + length];
            let shown = [
                " property=\"og:title\"",
                " property=\"og:description\"",
                " name=\"description\"",
            ]
            .iter()
            .any(|marker| tag.contains(marker));
            let names = ["alt", "title", "aria-label", "placeholder"]
                .into_iter()
                .chain(shown.then_some("content"));
            for name in names {
                let attribute = format!(" {name}=\"");
                if let Some(at) = tag.find(&attribute) {
                    let value = &tag[at + attribute.len()..];
                    let value = value.split('"').next().unwrap_or_default();
                    if has_words(value) {
                        return Some(format!("{name}=\"{value}\""));
                    }
                }
            }
            rest = &rest[start + length + 1..];
        }
        visible.push_str(rest);
        visible
            .lines()
            .find(|line| has_words(line))
            .map(|line| line.trim().to_owned())
    }

    #[test]
    fn the_scan_finds_a_word_outside_t() {
        assert_eq!(
            fixed_word(r#"<p class="a">{{ t(key="save") }}</p> &nbsp;"#),
            None
        );
        assert_eq!(
            fixed_word("{% block title %}Hello{% endblock title %}"),
            Some("Hello".to_owned())
        );
        assert_eq!(
            fixed_word(r#"<p>{{ x }} <b>Done</b></p>"#),
            Some("Done".to_owned())
        );
        assert_eq!(
            fixed_word(r#"<img src="{{ a }}" alt="Logo"> {# note #}"#),
            Some(r#"alt="Logo""#.to_owned())
        );
        assert_eq!(fixed_word(r#"<a title="KeySend">KeySend</a>"#), None);
        assert_eq!(fixed_word(r#"<img alt="OVHcloud"><span>Rust</span>"#), None);
        assert_eq!(
            fixed_word(r#"<meta property="og:title" content="Hello">"#),
            Some(r#"content="Hello""#.to_owned())
        );
        assert_eq!(
            fixed_word(r#"<meta property="og:type" content="website">"#),
            None
        );
        assert_eq!(
            fixed_word(r#"<meta property="og:title" content="{{ t(key="save") }}">"#),
            None
        );
        assert_eq!(fixed_word("<script>var a = 1;</script>"), None);
    }

    #[test]
    fn no_template_holds_a_word_outside_t_but_the_legal_bodies() {
        let mut files = Vec::new();
        views(Path::new("assets/views"), &mut files);
        let mut checked = 0;
        for file in files {
            if LEGAL_BODIES.iter().any(|body| file == Path::new(body)) {
                continue;
            }
            let source = std::fs::read_to_string(&file).unwrap();
            if let Some(word) = fixed_word(&source) {
                panic!("{}: `{word}` is not read through `t`", file.display());
            }
            checked += 1;
        }
        assert!(checked > 30);
        for body in LEGAL_BODIES {
            assert!(Path::new(body).exists(), "{body}");
        }
    }

    #[test]
    fn the_texts_of_the_file_name_of_an_export_are_ascii_without_spaces() {
        for language in Language::available() {
            let written = &language.translation().unwrap().written;
            for key in ["export_file", "export_file_fallback"] {
                for template in written.templates(key).unwrap() {
                    assert!(
                        template.bytes().all(|byte| byte.is_ascii_graphic()),
                        "{} `{key}`",
                        language.code()
                    );
                }
            }
        }
    }

    #[test]
    fn every_content_index_has_a_title_text() {
        for index in crate::content::INDEXES {
            assert!(Text::KEYS.contains(&index.title_key), "{}", index.title_key);
        }
    }

    #[test]
    fn templates_fail_on_an_unknown_key_a_wrong_value_and_a_missing_scope() {
        assert!(render(r#"{{ t(key="nope") }}"#).is_err());
        assert!(render(r#"{{ t(key="save", extra=1) }}"#).is_err());
        assert!(render(r#"{{ t(key="delete_workspace_summary") }}"#).is_err());
        assert!(render(r#"{{ t(key="google_connections_count") }}"#).is_err());
        assert!(render(r#"{{ t() }}"#).is_err());

        let mut tera = Tera::default();
        register(&mut tera);
        tera.add_raw_template("page.html", r#"{{ t(key="save") }}"#)
            .unwrap();
        assert!(tera.render("page.html", &tera::Context::new()).is_err());
    }

    #[test]
    fn every_template_loads_with_the_text_functions() {
        let views =
            loco_rs::controller::views::engines::TeraView::build_with_post_process(|tera| {
                register(tera);
                Ok(())
            });
        assert!(views.is_ok());
    }

    #[test]
    fn the_settings_page_shows_the_language_form_only_for_a_choice() {
        use loco_rs::controller::views::{engines::TeraView, ViewRenderer};
        let views = TeraView::build_with_post_process(|tera| {
            register(tera);
            tera.global_context().insert(
                "assets",
                &serde_json::json!({"images/logo.svg":"/l","images/logo-on-plum.svg":"/p","app.css":"/a","parts.css":"/b","popover.js":"/j"}),
            );
            Ok(())
        })
        .unwrap();
        let page = |languages: serde_json::Value| {
            with_language(Language::En, || {
                views
                    .render(
                        "accounts/settings.html",
                        serde_json::json!({"email":"a@example.com","google_connections":[],"workspace_id":"w","languages":languages}),
                    )
                    .unwrap()
            })
        };
        let one = page(serde_json::json!([{"code":"en","name":"English","selected":true}]));
        assert!(!one.contains(r#"<form class="form" method="post" action="/settings/language">"#));
        let two = page(serde_json::json!([
            {"code":"en","name":"English","selected":true},
            {"code":"ro","name":"Română","selected":false}
        ]));
        assert!(two.contains(r#"<form class="form" method="post" action="/settings/language">"#));
        assert!(two.contains(r#"<option value="en" selected>English</option>"#));
        assert!(two.contains(r#"<option value="ro">Română</option>"#));
        assert!(two.contains(r#"<button class="button" type="submit">Save</button>"#));
        assert!(two.contains("<p class=\"muted\">No Google Connection yet.</p>"));
    }
}

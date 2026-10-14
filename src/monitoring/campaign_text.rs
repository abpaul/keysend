use serde::Serialize;

use super::{update::Update, view};
use crate::translation::Translations;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Plain,
    Campaign,
    Mark,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Part {
    pub kind: Kind,
    pub text: String,
    pub href: Option<String>,
    pub english: bool,
}

impl Part {
    fn plain(text: &str) -> Self {
        Self {
            kind: Kind::Plain,
            text: text.into(),
            href: None,
            english: false,
        }
    }
}

pub struct Campaigns<'a> {
    names: Vec<(i64, &'a str)>,
    page: Option<i64>,
    href: Box<dyn Fn(i64) -> String + Send + Sync + 'a>,
    translations: Translations,
}

impl<'a> Campaigns<'a> {
    pub fn new(
        campaigns: impl IntoIterator<Item = (i64, &'a str)>,
        page: Option<i64>,
        href: impl Fn(i64) -> String + Send + Sync + 'a,
    ) -> Self {
        let mut names = campaigns
            .into_iter()
            .filter(|(_, name)| !name.trim().is_empty())
            .collect::<Vec<_>>();
        names.sort_by(|(a_id, a), (b_id, b)| b.len().cmp(&a.len()).then(a_id.cmp(b_id)));
        Self {
            names,
            page,
            href: Box::new(href),
            translations: Translations::english(),
        }
    }

    pub fn unlinked(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self::new(
            names
                .into_iter()
                .enumerate()
                .map(|(id, name)| (id as i64, name)),
            None,
            |_| String::new(),
        )
    }

    pub fn translated(mut self, translations: Translations) -> Self {
        self.translations = translations;
        self
    }

    pub fn translations(&self) -> &Translations {
        &self.translations
    }

    pub fn href(&self, campaign: i64) -> Option<String> {
        (self.page != Some(campaign) && self.names.iter().any(|(id, _)| *id == campaign))
            .then(|| (self.href)(campaign))
    }

    pub fn parts(&self, text: &str, about: Option<i64>) -> Vec<Part> {
        let shown = self.translations.shown(text);
        let mut parts = self.split(shown.text, about);
        if shown.english {
            parts.iter_mut().for_each(|part| part.english = true);
        }
        parts
    }

    pub fn split(&self, text: &str, about: Option<i64>) -> Vec<Part> {
        let mut parts = Vec::new();
        let mut plain = 0;
        let mut position = 0;
        while position < text.len() {
            let Some((id, name)) = self.name_at(text, position, about) else {
                position += text[position..].chars().next().map_or(1, char::len_utf8);
                continue;
            };
            if plain < position {
                parts.push(Part::plain(&text[plain..position]));
            }
            parts.push(Part {
                kind: Kind::Campaign,
                text: name.into(),
                href: id.and_then(|id| self.href(id)),
                english: false,
            });
            position += name.len();
            plain = position;
        }
        if plain < text.len() {
            parts.push(Part::plain(&text[plain..]));
        }
        parts
    }

    fn name_at(
        &self,
        text: &str,
        position: usize,
        about: Option<i64>,
    ) -> Option<(Option<i64>, &'a str)> {
        let outside = |neighbour: Option<char>| !neighbour.is_some_and(char::is_alphanumeric);
        if !outside(text[..position].chars().next_back()) {
            return None;
        }
        let rest = &text[position..];
        let (id, name) = self.names.iter().copied().find(|(id, name)| {
            about.is_none_or(|about| about == *id)
                && rest.starts_with(name)
                && outside(rest[name.len()..].chars().next())
        })?;
        let shared = about.is_none()
            && self
                .names
                .iter()
                .any(|(other, other_name)| *other != id && *other_name == name);
        Some(((!shared).then_some(id), name))
    }

    pub fn summary(&self, update: &Update) -> Vec<Part> {
        let shown = self.translations.shown(&update.summary);
        let highlight = self
            .translations
            .highlight_of(&update.summary, update.highlight.as_deref());
        let marked = view::marked(shown.text, highlight);
        let mut parts = self.split(&marked.before, None);
        if !marked.mark.is_empty() {
            parts.push(Part {
                kind: Kind::Mark,
                text: marked.mark,
                href: None,
                english: false,
            });
        }
        parts.extend(self.split(&marked.after, None));
        if shown.english {
            parts.iter_mut().for_each(|part| part.english = true);
        }
        parts
    }

    pub fn line<'t>(&'t self, text: &'t str) -> crate::translation::Shown<'t> {
        self.translations.shown(text)
    }
}

pub fn without_campaign_id(text: &str, campaign_id: i64) -> String {
    text.replace(&format!(" ({campaign_id})"), "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn campaigns(page: Option<i64>) -> Campaigns<'static> {
        Campaigns::new(
            [
                (1, "Search"),
                (2, "Search - Emergency"),
                (3, "Tom & <b>Jerry</b>"),
                (4, "Shared"),
                (5, "Shared"),
            ],
            page,
            |id| format!("/ad-accounts/7/campaigns/{id}?window=30"),
        )
    }

    fn campaign(text: &str, id: Option<i64>) -> Part {
        Part {
            kind: Kind::Campaign,
            text: text.into(),
            href: id.map(|id| format!("/ad-accounts/7/campaigns/{id}?window=30")),
            english: false,
        }
    }

    #[test]
    fn a_campaign_name_is_a_part_with_the_link_to_its_campaign() {
        assert_eq!(
            campaigns(None).parts("Review Search - Emergency and Search today.", None),
            [
                Part::plain("Review "),
                campaign("Search - Emergency", Some(2)),
                Part::plain(" and "),
                campaign("Search", Some(1)),
                Part::plain(" today."),
            ]
        );
    }

    #[test]
    fn a_name_inside_a_word_or_in_another_case_is_plain_text() {
        let text = "Searches and search terms of Research.";
        assert_eq!(campaigns(None).parts(text, None), [Part::plain(text)]);
    }

    #[test]
    fn a_text_with_a_stored_campaign_names_only_that_campaign() {
        assert_eq!(
            campaigns(None).parts("Shared lost budget. Search did not.", Some(5)),
            [
                campaign("Shared", Some(5)),
                Part::plain(" lost budget. Search did not."),
            ]
        );
        assert_eq!(
            campaigns(None).parts("Search lost budget.", Some(99)),
            [Part::plain("Search lost budget.")]
        );
    }

    #[test]
    fn a_name_of_two_campaigns_has_no_link_without_a_stored_campaign() {
        assert_eq!(
            campaigns(None).parts("Shared lost budget.", None),
            [campaign("Shared", None), Part::plain(" lost budget.")]
        );
    }

    #[test]
    fn the_campaign_of_the_page_has_no_link() {
        assert_eq!(
            campaigns(Some(2)).parts("Search - Emergency lost budget.", Some(2)),
            [
                campaign("Search - Emergency", None),
                Part::plain(" lost budget."),
            ]
        );
        assert_eq!(campaigns(Some(2)).href(2), None);
        assert_eq!(
            campaigns(Some(2)).href(1).as_deref(),
            Some("/ad-accounts/7/campaigns/1?window=30")
        );
        assert_eq!(campaigns(None).href(99), None);
    }

    #[test]
    fn a_name_keeps_its_characters_for_the_template_to_escape() {
        assert_eq!(
            campaigns(None).parts("Pause Tom & <b>Jerry</b>.", None),
            [
                Part::plain("Pause "),
                campaign("Tom & <b>Jerry</b>", Some(3)),
                Part::plain("."),
            ]
        );
    }

    #[test]
    fn a_stored_text_loses_the_id_of_its_campaign() {
        assert_eq!(
            without_campaign_id(
                "Sales-Search-1- Hotline (23056941016) lost 20% of its search impressions (23).",
                23_056_941_016
            ),
            "Sales-Search-1- Hotline lost 20% of its search impressions (23)."
        );
    }
}

use axum::extract::OriginalUri;
use loco_rs::prelude::*;
use serde_json::{json, Value};

use super::monitoring::example;
use crate::{
    app::not_found,
    content,
    language::{request_text, set_current, Entry, Language, Text},
    monitoring::access,
};

type Question = (&'static str, &'static str);

pub struct Audience {
    pub path: &'static str,
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    source: &'static str,
    template: &'static str,
    questions: &'static [Question],
}

pub const AUDIENCES: [Audience; 3] = [
    Audience {
        path: "/for/business-owners",
        name: "for_owners_link",
        title: "for_owners_title",
        description: "for_owners_description",
        source: "for-business-owners",
        template: "home/for_business_owners.html",
        questions: &[
            ("for_owners_faq_know_q", "for_owners_faq_know_a"),
            ("home_faq_start_q", "for_owners_faq_start_a"),
            ("home_faq_tracking_q", "home_faq_tracking_a"),
            ("home_faq_change_q", "home_faq_change_a"),
            ("home_faq_consent_q", "home_faq_consent_a"),
            ("home_faq_ends_q", "home_faq_ends_a"),
        ],
    },
    Audience {
        path: "/for/agency-clients",
        name: "for_agency_link",
        title: "for_agency_title",
        description: "for_agency_description",
        source: "for-agency-clients",
        template: "home/for_agency_clients.html",
        questions: &[
            ("for_agency_faq_work_q", "for_agency_faq_work_a"),
            ("for_agency_faq_access_q", "for_agency_faq_access_a"),
            ("for_agency_faq_judge_q", "for_agency_faq_judge_a"),
            ("home_faq_consent_q", "home_faq_consent_a"),
            ("home_faq_ends_q", "home_faq_ends_a"),
        ],
    },
    Audience {
        path: "/for/marketing-managers",
        name: "for_managers_link",
        title: "for_managers_title",
        description: "for_managers_description",
        source: "for-marketing-managers",
        template: "home/for_marketing_managers.html",
        questions: &[
            ("home_faq_more_q", "home_faq_more_a"),
            ("home_faq_ai_q", "home_faq_ai_a"),
            (
                "for_managers_faq_dashboard_q",
                "for_managers_faq_dashboard_a",
            ),
            ("home_faq_change_q", "home_faq_change_a"),
            ("home_faq_ends_q", "home_faq_ends_a"),
        ],
    },
];

const HOME_QUESTIONS: [Question; 10] = [
    ("home_faq_change_q", "home_faq_change_a"),
    ("home_faq_count_q", "home_faq_count_a"),
    ("home_faq_know_q", "home_faq_know_a"),
    ("home_faq_start_q", "home_faq_start_a"),
    ("home_faq_tracking_q", "home_faq_tracking_a"),
    ("home_faq_consent_q", "home_faq_consent_a"),
    ("home_faq_advisor_q", "home_faq_advisor_a"),
    ("home_faq_more_q", "home_faq_more_a"),
    ("home_faq_agency_q", "home_faq_agency_a"),
    ("home_faq_ends_q", "home_faq_ends_a"),
];

fn trial_days() -> i64 {
    access::TRIAL_SECONDS / 86_400
}

fn landing(text: &'static Text, questions: &[Question]) -> Result<Value> {
    let price = crate::billing::SUBSCRIPTION_USD_CENTS / 100;
    let limit = [("count", access::AD_ACCOUNT_LIMIT.to_string())];
    let mut data = example::view(text)?;
    data["trial_days"] = trial_days().into();
    data["price"] = price.into();
    data["price_text"] = text.whole_money(price as f64, "USD").into();
    data["limit"] = access::AD_ACCOUNT_LIMIT.into();
    data["organization_id"] = content::organization_id().into();
    data["contact"] = content::CONTACT.into();
    data["questions"] = questions
        .iter()
        .map(|(question, answer)| {
            let values: &[(&str, String)] = match text.entry(answer) {
                Some(Entry::Plural(_)) => &limit,
                _ => &[],
            };
            Ok(json!({
                "question": text.get(question, &[])?,
                "answer": text.get(answer, values)?,
            }))
        })
        .collect::<Result<Vec<_>>>()?
        .into();
    Ok(data)
}

fn structured_data(data: &Value, nodes: Vec<Value>) -> String {
    let questions = data["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            json!({
                "@type": "Question",
                "name": item["question"],
                "acceptedAnswer": {"@type": "Answer", "text": item["answer"]},
            })
        })
        .collect::<Vec<_>>();
    let mut graph = vec![content::organization()];
    graph.extend(nodes);
    graph.push(json!({"@type": "FAQPage", "mainEntity": questions}));
    content::script_json(&json!({"@context": "https://schema.org", "@graph": graph}))
}

async fn home(
    ViewEngine(v): ViewEngine<TeraView>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response> {
    let language = if uri.path() == "/ro" {
        Language::Ro
    } else {
        Language::En
    };
    set_current(language)?;
    let mut data = landing(request_text()?, &HOME_QUESTIONS)?;
    data["home_path"] = uri.path().into();
    data["canonical"] = format!("{}{}", content::ORIGIN, uri.path()).into();
    data["home_languages"] = json!([
        {"code": Language::En.code(), "name": Language::En.name(), "path": "/"},
        {"code": Language::Ro.code(), "name": Language::Ro.name(), "path": "/ro"},
    ]);
    let suffix = if language == Language::Ro { "-ro" } else { "" };
    data["source_suffix"] = suffix.into();
    data["nav_from"] = format!("home-nav{suffix}").into();
    data["schema"] = structured_data(&data, Vec::new()).into();
    format::render().view(&v, "home/index.html", data)
}

async fn audience(
    ViewEngine(v): ViewEngine<TeraView>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response> {
    let Some(page) = AUDIENCES.iter().find(|page| page.path == uri.path()) else {
        return Ok(not_found());
    };
    let text = request_text()?;
    let origin = content::ORIGIN;
    let url = format!("{origin}{}", page.path);
    let title = text.get(page.title, &[])?;
    let description = page.described(text)?;
    let mut data = landing(text, page.questions)?;
    data["schema"] = structured_data(
        &data,
        vec![
            json!({
                "@type": "WebPage",
                "@id": format!("{url}#page"),
                "url": url,
                "name": title,
                "description": description,
                "inLanguage": "en",
                "publisher": {"@id": content::organization_id()},
            }),
            json!({
                "@type": "BreadcrumbList",
                "itemListElement": [
                    {"@type": "ListItem", "position": 1, "name": "KeySend", "item": format!("{origin}/")},
                    {"@type": "ListItem", "position": 2, "name": text.get(page.name, &[])?, "item": url},
                ],
            }),
        ],
    )
    .into();
    data["path"] = page.path.into();
    data["from"] = page.source.into();
    data["nav_from"] = page.source.into();
    data["title"] = title.into();
    data["description"] = description.into();
    format::render().view(&v, page.template, data)
}

impl Audience {
    pub fn described(&self, text: &'static Text) -> Result<String> {
        text.get(
            self.description,
            &[("trial_days", trial_days().to_string())],
        )
    }
}

async fn privacy(ViewEngine(v): ViewEngine<TeraView>) -> Result<Response> {
    format::render().view(&v, "home/privacy.html", data!({}))
}

async fn terms(ViewEngine(v): ViewEngine<TeraView>) -> Result<Response> {
    format::render().view(
        &v,
        "home/terms.html",
        data!({"limit": access::AD_ACCOUNT_LIMIT}),
    )
}

pub const SECURITY_PATH: &str = "/security";

async fn security(ViewEngine(v): ViewEngine<TeraView>) -> Result<Response> {
    let text = request_text()?;
    let origin = content::ORIGIN;
    let url = format!("{origin}{SECURITY_PATH}");
    let title = text.get("security_title", &[])?;
    let description = text.get("security_description", &[])?;
    let schema = content::script_json(&json!({
        "@context": "https://schema.org",
        "@graph": [
            content::organization(),
            {
                "@type": "WebPage",
                "@id": format!("{url}#page"),
                "url": url,
                "name": title,
                "description": description,
                "inLanguage": "en",
                "publisher": {"@id": content::organization_id()},
            },
            {
                "@type": "BreadcrumbList",
                "itemListElement": [
                    {"@type": "ListItem", "position": 1, "name": "KeySend", "item": format!("{origin}/")},
                    {"@type": "ListItem", "position": 2, "name": text.get("footer_security", &[])?, "item": url},
                ],
            },
        ],
    }));
    format::render().view(
        &v,
        "home/security.html",
        data!({
            "path": SECURITY_PATH,
            "title": title,
            "description": description,
            "schema": schema,
            "contact": content::CONTACT,
            "contact_href": format!("mailto:{}?subject=Privacy", content::CONTACT),
        }),
    )
}

pub fn routes() -> Routes {
    let mut routes = Routes::new()
        .add("/", get(home))
        .add("/ro", get(home))
        .add("/privacy", get(privacy))
        .add("/terms", get(terms))
        .add(SECURITY_PATH, get(security));
    for page in &AUDIENCES {
        routes = routes.add(page.path, get(audience));
    }
    routes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::SignUpSource;

    #[test]
    fn the_sign_in_page_accepts_the_source_of_each_audience_page() {
        for page in &AUDIENCES {
            assert_eq!(
                SignUpSource::parse(page.source)
                    .as_ref()
                    .map(SignUpSource::as_str),
                Some(page.source)
            );
        }
    }
}

use std::{collections::BTreeSet, sync::OnceLock};

use loco_rs::{
    controller::views::{engines::TeraView, ViewRenderer},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    campaign_text::{self, Campaigns},
    causes,
    charts::{self, Part, Sentence},
    format::{self, Cell},
    metrics::{self, CampaignDay, WindowMetrics},
    update::{Update, RESULTS_UNJUDGED},
};
use crate::{
    language::{english, say, say_plural, with_language, Language, Text},
    settings::Settings,
    translation::{Translations, Wanted},
    workspace::{
        history::{date_of, Campaign},
        monitoring::Recommendation,
    },
};

const BAR_SHARE: f64 = 0.55;

pub struct Message {
    pub subject: String,
    pub html: String,
    pub text: String,
}

static VIEWS: OnceLock<TeraView> = OnceLock::new();

pub(super) fn render(
    language: Language,
    template: &str,
    context: &impl Serialize,
) -> Result<String> {
    let views = match VIEWS.get() {
        Some(views) => views,
        None => {
            let views = TeraView::build_with_post_process(|tera| {
                crate::language::register(tera);
                Ok(())
            })?;
            VIEWS.get_or_init(|| views)
        }
    };
    with_language(language, || views.render(template, context))
}

const NAME_LENGTH: usize = 60;

/// A name for a subject or a headline. A mail client turns a domain, an address or a URL in a
/// Campaign name into a link, and the name comes from Google Ads. A dot before a letter, an `@`
/// and `://` are written so that no client links them, and a long name is cut.
pub(super) fn display_name(name: &str) -> String {
    let characters = name.chars().collect::<Vec<_>>();
    let mut shown = String::new();
    for (index, character) in characters.iter().enumerate().take(NAME_LENGTH) {
        let after_text = index > 0 && characters[index - 1].is_alphanumeric();
        let before_letter = characters
            .get(index + 1)
            .is_some_and(|next| next.is_alphabetic());
        match character {
            '.' if after_text && before_letter => shown.push_str("[.]"),
            '@' => shown.push_str("[at]"),
            ':' if characters[index..].starts_with(&[':', '/', '/']) => shown.push_str("[:]"),
            other => shown.push(*other),
        }
    }
    if characters.len() > NAME_LENGTH {
        shown.push('…');
    }
    shown
}

pub(super) fn link(origin: &str, ad_account: &str) -> String {
    format!("{}/ad-accounts/{ad_account}", origin.trim_end_matches('/'))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CampaignPart {
    pub name: String,
    pub spend: f64,
    pub results: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct WeeklyBlock {
    pub ad_account: String,
    pub name: String,
    pub currency: String,
    pub metrics: WindowMetrics,
    pub summary: String,
    pub updates: Vec<Update>,
    pub recommendation: Option<Recommendation>,
    #[serde(default)]
    pub campaigns: Vec<CampaignPart>,
}

pub fn campaign_parts(
    days: &[CampaignDay],
    campaigns: &[Campaign],
    metrics: &WindowMetrics,
) -> Vec<CampaignPart> {
    let judged = metrics.current.conversions.is_some();
    campaigns
        .iter()
        .map(|campaign| {
            let total = super::metrics::sum(
                days,
                metrics.end_day - 6,
                metrics.end_day,
                Some(campaign.id),
            );
            CampaignPart {
                name: campaign.name.clone(),
                spend: total.spend,
                results: if judged {
                    total.conversions.unwrap_or_default()
                } else {
                    total.clicks
                },
            }
        })
        .filter(|part| part.spend > 0.0 || part.results > 0.0)
        .collect()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct WeeklyEmail {
    pub day: i64,
    pub blocks: Vec<WeeklyBlock>,
    #[serde(default)]
    pub left_out: Vec<String>,
    pub body: String,
}

pub fn block(
    ad_account: String,
    name: String,
    currency: String,
    metrics: WindowMetrics,
    updates: Vec<Update>,
    recommendation: Option<Recommendation>,
    campaigns: Vec<CampaignPart>,
) -> WeeklyBlock {
    let current = &metrics.current;
    let previous = &metrics.previous;
    let summary = format!(
        "Spend was {} against {} in the previous 7 days. Clicks were {:.0} against {:.0}. {}",
        metrics::english_money(current.spend, &currency),
        metrics::english_money(previous.spend, &currency),
        current.clicks,
        previous.clicks,
        match current.conversions.zip(previous.conversions) {
            Some((current, previous)) =>
                format!("Conversions were {current:.2} against {previous:.2}."),
            None => RESULTS_UNJUDGED.into(),
        }
    );
    WeeklyBlock {
        ad_account,
        name,
        currency,
        metrics,
        summary,
        updates,
        recommendation,
        campaigns,
    }
}

pub fn content(day: i64, blocks: Vec<WeeklyBlock>, left_out: Vec<String>) -> Result<WeeklyEmail> {
    let mut body = blocks
        .iter()
        .map(|block| {
            let mut body = format!(
                "{}\n{}\n\nMetrics\nSpend: {}\nClicks: {:.0}\n",
                block.name,
                block.summary,
                metrics::english_money(block.metrics.current.spend, &block.currency),
                block.metrics.current.clicks
            );
            if let Some(conversions) = block.metrics.current.conversions {
                body.push_str(&format!("Conversions: {conversions:.2}\n"));
            } else {
                body.push_str(&format!(
                    "Impressions: {:.0}\n",
                    block.metrics.current.impressions
                ));
            }
            body.push_str("\nUpdates\n");
            for update in block.updates.iter().rev() {
                let date = crate::workspace::history::date_of(update.day)
                    .map(|date| date.to_string())
                    .unwrap_or_default();
                body.push_str(&format!("{date}: {}\n", update.summary));
                if let Some(cause) = &update.cause {
                    body.push_str(&format!(
                        "{}: {}\n",
                        causes::label(english(), cause.level),
                        cause.evidence
                    ));
                }
                for line in &update.lines {
                    body.push_str(&format!("{}\n", line.text));
                }
            }
            if let Some(recommendation) = &block.recommendation {
                body.push_str(&format!(
                    "\nRecommendation\n{}\n{}\nEvidence: {}\n",
                    recommendation.action, recommendation.reason, recommendation.evidence
                ));
            }
            body
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if let Some(note) = left_out_note(english(), &left_out)? {
        body.push_str(&format!("\n\n{note}"));
    }
    Ok(WeeklyEmail {
        day,
        blocks,
        left_out,
        body,
    })
}

fn left_out_note(text: &Text, left_out: &[String]) -> Result<Option<String>> {
    if left_out.is_empty() {
        return Ok(None);
    }
    let names = join_names(
        text,
        &left_out
            .iter()
            .map(|name| display_name(name))
            .collect::<Vec<_>>(),
    )?;
    say_plural!(text, email_left_out, left_out.len() as u64, names = names).map(Some)
}

fn stored_names(block: &WeeklyBlock) -> BTreeSet<&str> {
    block
        .campaigns
        .iter()
        .map(|part| part.name.as_str())
        .chain(block.updates.iter().flat_map(|update| {
            update
                .detection
                .campaigns
                .iter()
                .map(|campaign| campaign.name.as_str())
        }))
        .collect()
}

#[derive(Serialize)]
struct Stored {
    text: String,
    parts: Vec<campaign_text::Part>,
    english: bool,
}

fn stored(translations: &Translations, campaigns: &Campaigns, english: &str) -> Stored {
    let shown = translations.shown(english);
    Stored {
        text: shown.text.to_owned(),
        parts: campaigns.split(shown.text, None),
        english: shown.english,
    }
}

#[derive(Serialize)]
struct CauseView {
    label: &'static str,
    evidence: Stored,
}

#[derive(Serialize)]
struct UpdateView {
    date: String,
    summary: Stored,
    cause: Option<CauseView>,
    lines: Vec<Stored>,
}

#[derive(Serialize)]
struct RecommendationView {
    action: Stored,
    reason: Stored,
    evidence: Stored,
}

pub fn wanted(email: &WeeklyEmail) -> Wanted {
    let mut wanted = Wanted::default();
    for block in &email.blocks {
        for update in &block.updates {
            wanted.update(update);
        }
        if let Some(recommendation) = &block.recommendation {
            wanted.recommendation(recommendation);
        }
    }
    wanted
}

pub fn untranslated(email: &WeeklyEmail, translations: &Translations) -> usize {
    let mut count = 0;
    let mut check = |english: &str| count += usize::from(translations.shown(english).english);
    for block in &email.blocks {
        for update in &block.updates {
            check(&update.summary);
            if let Some(cause) = &update.cause {
                check(&cause.evidence);
            }
            update.lines.iter().for_each(|line| check(&line.text));
        }
        if let Some(recommendation) = &block.recommendation {
            check(&recommendation.action);
            check(&recommendation.reason);
            check(&recommendation.evidence);
        }
    }
    count
}

#[derive(Serialize)]
struct BarView {
    name: String,
    spend: String,
    spend_width: u32,
    results: String,
    results_width: u32,
}

#[derive(Serialize)]
struct SentenceView {
    strong: String,
    text: Vec<campaign_text::Part>,
    mark: String,
    after: Vec<campaign_text::Part>,
    words: String,
}

#[derive(Serialize)]
struct ChartView {
    title: String,
    sentence: SentenceView,
    rows: Vec<BarView>,
}

fn chart_view(text: &Text, block: &WeeklyBlock) -> Result<Option<ChartView>> {
    let judged = block.metrics.current.conversions.is_some();
    let chart = charts::shares(
        text,
        charts::Subject::Campaign,
        &charts::shares_title(text, charts::Subject::Campaign, judged, None)?,
        block
            .campaigns
            .iter()
            .map(|part| Part {
                name: display_name(&part.name),
                href: None,
                spend: part.spend,
                results: part.results,
            })
            .collect(),
        judged,
        &block.currency,
    )?;
    if chart.rows.is_empty() {
        return Ok(None);
    }
    let shown = block
        .campaigns
        .iter()
        .map(|part| display_name(&part.name))
        .collect::<Vec<_>>();
    let finder = Campaigns::unlinked(shown.iter().map(String::as_str));
    let Sentence {
        strong,
        text: sentence_text,
        mark,
        after,
    } = &chart.sentence;
    let sentence = SentenceView {
        strong: strong.clone(),
        text: finder.split(sentence_text, None),
        mark: mark.clone(),
        after: finder.split(after, None),
        words: chart.sentence.words(),
    };
    let width = |percent: f64| {
        if percent > 0.0 {
            ((percent * BAR_SHARE).round() as u32).max(1)
        } else {
            0
        }
    };
    Ok(Some(ChartView {
        rows: chart
            .rows
            .iter()
            .filter_map(|row| {
                let bars = row.bars.as_ref()?;
                Some(BarView {
                    name: row.name.clone(),
                    spend: bars.spend.clone(),
                    spend_width: width(bars.spend_width),
                    results: bars.results.clone(),
                    results_width: width(bars.results_width),
                })
            })
            .collect(),
        title: chart.title,
        sentence,
    }))
}

#[derive(Serialize)]
struct BlockView {
    name: String,
    range: String,
    link: String,
    quiet: bool,
    judged: bool,
    cells: Vec<Cell>,
    unjudged: Option<&'static str>,
    chart: Option<ChartView>,
    lead: UpdateView,
    earlier: Vec<UpdateView>,
    recommendation: Option<RecommendationView>,
}

fn update_view(
    text: &Text,
    update: &Update,
    translations: &Translations,
    campaigns: &Campaigns,
) -> UpdateView {
    UpdateView {
        date: date_of(update.day)
            .map(|date| format::long_date(text, date))
            .unwrap_or_default(),
        summary: stored(translations, campaigns, &update.summary),
        cause: update.cause.as_ref().map(|cause| CauseView {
            label: causes::label(text, cause.level),
            evidence: stored(translations, campaigns, &cause.evidence),
        }),
        lines: update
            .lines
            .iter()
            .map(|line| stored(translations, campaigns, &line.text))
            .collect(),
    }
}

fn block_view(
    text: &Text,
    block: &WeeklyBlock,
    origin: &str,
    translations: &Translations,
) -> Result<BlockView> {
    let (lead, earlier) = block
        .updates
        .split_first()
        .ok_or_else(|| Error::Message("A Weekly email block has no Update".into()))?;
    let end = date_of(block.metrics.end_day)
        .ok_or_else(|| Error::Message("Invalid Weekly email day".into()))?;
    let start = end - chrono::Days::new(6);
    let names = stored_names(block);
    let finder = Campaigns::unlinked(names.iter().copied());
    let lead_view = update_view(text, lead, translations, &finder);
    let unjudged = block
        .metrics
        .current
        .conversions
        .is_none()
        .then_some(text.written.results_unjudged);
    Ok(BlockView {
        name: display_name(&block.name),
        range: format::date_range(text, start, end, false),
        link: link(origin, &block.ad_account),
        quiet: lead.detection.quiet() && block.recommendation.is_none(),
        judged: block.metrics.current.conversions.is_some(),
        cells: format::cells(text, &block.metrics, &block.currency)?
            .into_iter()
            .take(3)
            .collect(),
        unjudged,
        chart: chart_view(text, block)?,
        lead: lead_view,
        earlier: earlier
            .iter()
            .map(|update| update_view(text, update, translations, &finder))
            .collect(),
        recommendation: block
            .recommendation
            .as_ref()
            .map(|recommendation| RecommendationView {
                action: stored(translations, &finder, &recommendation.action),
                reason: stored(translations, &finder, &recommendation.reason),
                evidence: stored(translations, &finder, &recommendation.evidence),
            }),
    })
}

fn join_names(text: &Text, names: &[String]) -> Result<String> {
    match names {
        [] => Ok(String::new()),
        [only] => Ok(only.clone()),
        [rest @ .., last] => say!(text, email_list_and, rest = rest.join(", "), last = last),
    }
}

fn text_update(update: &UpdateView) -> String {
    let mut text = format!("{}\n{}\n", update.date, update.summary.text);
    if let Some(cause) = &update.cause {
        text.push_str(&format!("{}: {}\n", cause.label, cause.evidence.text));
    }
    for line in &update.lines {
        text.push_str(&format!("- {}\n", line.text));
    }
    text
}

fn text_block(text: &Text, block: &BlockView) -> Result<String> {
    let written = &text.written;
    let mut out = format!(
        "{}\n{}\n\n{}\n",
        block.name,
        say!(text, email_block_range, range = &block.range)?,
        text_update(&block.lead)
    );
    out.push_str(&format!("\n{}\n", written.email_metrics_heading));
    for cell in &block.cells {
        out.push_str(&say!(
            text,
            email_metric_line,
            label = cell.label,
            value = &cell.value,
            change = &cell.change
        )?);
        out.push('\n');
    }
    if let Some(unjudged) = block.unjudged {
        out.push_str(&format!("{unjudged}\n"));
    }
    if let Some(chart) = &block.chart {
        out.push_str(&format!("\n{}\n{}\n", chart.title, chart.sentence.words));
        for row in &chart.rows {
            let line = if block.judged {
                say!(
                    text,
                    email_share_row_conversions,
                    name = &row.name,
                    spend = &row.spend,
                    results = &row.results
                )?
            } else {
                say!(
                    text,
                    email_share_row_clicks,
                    name = &row.name,
                    spend = &row.spend,
                    results = &row.results
                )?
            };
            out.push_str(&format!("{line}\n"));
        }
    }
    if !block.earlier.is_empty() {
        out.push_str(&format!("\n{}\n", written.email_earlier_updates));
        for update in &block.earlier {
            out.push_str(&format!("\n{}", text_update(update)));
        }
    }
    if let Some(recommendation) = &block.recommendation {
        out.push_str(&format!(
            "\n{}\n{}\n{}\n{}\n",
            written.email_recommendation_heading,
            recommendation.action.text,
            recommendation.reason.text,
            say!(
                text,
                email_finding,
                evidence = &recommendation.evidence.text
            )?
        ));
    }
    out.push_str(&format!(
        "\n{}\n",
        say!(text, email_open, name = &block.name, link = &block.link)?
    ));
    Ok(out)
}

#[derive(Debug)]
pub enum DeliveryError {
    /// The service did not accept the email, so it can be sent again.
    Refused(Error),
    /// The request may have reached the service. The email may have been sent, so it must not be
    /// sent again.
    Unknown { kind: &'static str },
}

impl From<DeliveryError> for Error {
    fn from(error: DeliveryError) -> Self {
        match error {
            DeliveryError::Refused(error) => error,
            DeliveryError::Unknown { .. } => {
                Self::Message("The outcome of the email is unknown".into())
            }
        }
    }
}

fn refused(message: &str) -> DeliveryError {
    DeliveryError::Refused(Error::Message(message.into()))
}

pub async fn deliver(
    settings: &Settings,
    recipient: &str,
    subject: &str,
    text: &str,
    html: Option<&str>,
) -> std::result::Result<(), DeliveryError> {
    if settings.email_recipient.as_deref() != Some(recipient) {
        return Err(refused("Email recipient is not enabled"));
    }
    let (url, key) = settings
        .services
        .email_url
        .as_deref()
        .zip(settings.email_key.as_deref())
        .ok_or_else(|| refused("Email is not configured"))?;
    let response = settings
        .http
        .post(url)
        .bearer_auth(key)
        .json(&json!({
            "from":"hello@keysend.app", "to":[recipient], "subject":subject,
            "text":text, "html":html
        }))
        .send()
        .await
        .map_err(|error| {
            let kind = crate::settings::transport_kind(&error);
            if error.is_connect() || error.is_builder() {
                tracing::warn!(kind, "Email could not be sent");
                refused("Email could not be sent")
            } else {
                tracing::error!(
                    kind,
                    "The outcome of the email is unknown; it is not sent again"
                );
                DeliveryError::Unknown { kind }
            }
        })?;
    if !response.status().is_success() {
        tracing::warn!(
            http_status = response.status().as_u16(),
            "Email could not be sent"
        );
        return Err(refused("Email could not be sent"));
    }
    match response.json::<serde_json::Value>().await {
        Ok(body) if body.get("success").and_then(serde_json::Value::as_bool) == Some(false) => {
            tracing::warn!("Email service refused the email");
            Err(refused("Email was refused"))
        }
        Ok(_) => Ok(()),
        Err(error) => {
            tracing::warn!(
                kind = crate::settings::transport_kind(&error),
                "Email response could not be read; the status says the email was sent"
            );
            Ok(())
        }
    }
}

pub fn message(
    language: Language,
    email: &WeeklyEmail,
    origin: &str,
    translations: &Translations,
) -> Result<Message> {
    let text = language.text()?;
    let blocks = email
        .blocks
        .iter()
        .map(|block| block_view(text, block, origin, translations))
        .collect::<Result<Vec<_>>>()?;
    let sent_on = date_of(email.day)
        .map(|date| format::long_date(text, date))
        .unwrap_or_default();
    let subject = say!(
        text,
        email_subject_weekly,
        names = join_names(
            text,
            &blocks
                .iter()
                .map(|block| block.name.clone())
                .collect::<Vec<_>>()
        )?
    )?;
    let left_out = left_out_note(text, &email.left_out)?;
    let preheader = blocks
        .first()
        .map(|block| {
            block
                .lead
                .summary
                .text
                .chars()
                .take(140)
                .collect::<String>()
        })
        .unwrap_or_default();
    let footer = text.written.email_footer;
    let note = text.written.email_weekly_note;
    let html = render(
        language,
        "email/weekly.html",
        &serde_json::json!({"title": subject, "preheader": preheader, "sent_on": sent_on, "blocks": blocks, "left_out": left_out, "footer": footer, "note": note}),
    )?;
    let left_out_text = left_out
        .as_deref()
        .map(|note| format!("{note}\n\n"))
        .unwrap_or_default();
    let text_blocks = blocks
        .iter()
        .map(|block| text_block(text, block))
        .collect::<Result<Vec<_>>>()?
        .join("\n---\n\n");
    let body = format!(
        "{}\n{}\n\n{text_blocks}\n---\n{left_out_text}{footer}\n{note}",
        text.written.email_weekly_title,
        say!(text, email_weekly_intro, sent_on = &sent_on)?,
    );
    Ok(Message {
        subject,
        html,
        text: body,
    })
}

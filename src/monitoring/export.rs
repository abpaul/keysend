use std::fmt::Write as _;

use loco_rs::{Error, Result};
use typst_as_lib::TypstEngine;
use typst_layout::PagedDocument;

use crate::language::{fill, say, say_plural, Text};

use super::{
    campaign_text::{Campaigns, Kind},
    deep_view::{Drawn, Fact, Section},
    format::Cell,
    view::{self, Notice},
};

const INK: &str = "292039";
const MUTED: &str = "5C5470";
const RULE: &str = "E4E2EC";
const CANVAS: &str = "F5F5F7";
const FONT: &str = "Liberation Sans";
const METRIC_COLUMNS: usize = 4;
const CHIP_TEXT: &str = "3F1D85";
const CHIP_FILL: &str = "EEE9F6";
const CAMPAIGN_SLOT: char = '\u{E000}';
const TEXT_SLOT: char = '\u{E001}';

pub struct UpdateLine {
    pub campaign: String,
    pub text: String,
    pub cause: Option<String>,
}

pub struct UpdateText {
    pub date: String,
    pub summary: String,
    pub cause: Option<String>,
    pub lines: Vec<UpdateLine>,
    pub more_campaigns: usize,
}

pub struct EarlierUpdate {
    pub date: String,
    pub summary: String,
}

pub struct CampaignRow {
    pub name: String,
    pub sparkline: Option<String>,
    pub cells: Vec<view::Cell>,
}

pub struct RecommendationText {
    pub subject: String,
    pub confidence: super::recommendation::Confidence,
    pub finding: Option<String>,
    pub action: String,
    pub reason: String,
    pub measure: String,
    pub evidence: String,
}

pub struct ChartImage {
    pub title: String,
    pub sentence: String,
    pub svg: Option<String>,
    pub note: Option<String>,
    pub changes: ChartChanges,
}

#[derive(Debug, PartialEq)]
pub struct DatedChange {
    pub date: String,
    pub text: String,
    pub follow_up: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum ChartChanges {
    NotShown,
    NoneInWindow,
    Listed(Vec<DatedChange>),
}

impl ChartChanges {
    pub fn of(dated: Vec<DatedChange>) -> Self {
        if dated.is_empty() {
            Self::NoneInWindow
        } else {
            Self::Listed(dated)
        }
    }
}

pub struct DeepCharts {
    pub date: Option<String>,
    pub charts: Vec<ChartImage>,
}

pub struct CampaignDeepCheck {
    pub date: String,
    pub sections: Vec<Section>,
}

pub enum Subject {
    AdAccount {
        assessment: Option<Box<view::AssessmentText>>,
        update: Option<UpdateText>,
        earlier: Vec<EarlierUpdate>,
        campaign_headers: Vec<&'static str>,
        campaigns: Vec<CampaignRow>,
        deep_charts: DeepCharts,
    },
    Campaign {
        name: String,
        lines: Vec<(String, String)>,
        findings: Vec<String>,
        deep_check: Option<CampaignDeepCheck>,
    },
}

pub struct Export {
    pub sample: bool,
    pub name: String,
    pub accent: String,
    pub logo: Option<Vec<u8>>,
    pub window_days: i64,
    pub campaign_names: Vec<String>,
    pub range: String,
    pub compared: String,
    pub prepared: String,
    pub notice: Option<Notice>,
    pub cells: Vec<Cell>,
    pub sparklines: Vec<(&'static str, String)>,
    pub charts: Vec<ChartImage>,
    pub recommendations: Vec<RecommendationText>,
    pub subject: Subject,
}

impl Export {
    fn drawn(&self) -> Vec<&ChartImage> {
        let deep = match &self.subject {
            Subject::AdAccount { deep_charts, .. } => deep_charts.charts.as_slice(),
            Subject::Campaign { .. } => &[],
        };
        self.charts.iter().chain(deep).collect()
    }

    fn sparkline_files(&self) -> Vec<(String, &str)> {
        let metrics = self
            .sparklines
            .iter()
            .enumerate()
            .map(|(index, (_, svg))| (metric_spark_file(index), svg.as_str()));
        let campaigns = match &self.subject {
            Subject::AdAccount { campaigns, .. } => campaigns.as_slice(),
            Subject::Campaign { .. } => &[],
        };
        metrics
            .chain(campaigns.iter().enumerate().filter_map(|(index, row)| {
                Some((campaign_spark_file(index), row.sparkline.as_deref()?))
            }))
            .collect()
    }
}

fn export_error() -> Error {
    Error::Message("Export rendering failed".into())
}

fn ts(value: &str) -> String {
    let clean = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    serde_json::to_string(&clean).unwrap_or_default()
}

fn markup(campaigns: &Campaigns, text: &str) -> String {
    campaigns
        .split(text, None)
        .iter()
        .map(|part| match part.kind {
            Kind::Campaign => format!("#campaign({})", ts(&part.text)),
            Kind::Plain | Kind::Mark => format!("#({})", ts(&part.text)),
        })
        .collect()
}

fn valid_colour(colour: &str) -> bool {
    colour.len() == 7
        && colour.starts_with('#')
        && colour[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn chart_file(index: usize) -> String {
    format!("chart-{}.svg", index + 1)
}

fn metric_spark_file(index: usize) -> String {
    format!("metric-{}.svg", index + 1)
}

fn campaign_spark_file(index: usize) -> String {
    format!("campaign-{}.svg", index + 1)
}

fn join(items: impl IntoIterator<Item = String>) -> String {
    items.into_iter().collect::<Vec<_>>().join(", ")
}

fn muted(size: f64, body: &str) -> String {
    format!("#text(size: {size}pt, fill: muted)[{body}]")
}

fn campaign_line(text: &Text, campaigns: &Campaigns, line: &UpdateLine) -> Result<String> {
    let pattern = fill(
        "update_campaign_line",
        text.update_campaign_line,
        &[
            ("campaign", CAMPAIGN_SLOT.to_string()),
            ("text", TEXT_SLOT.to_string()),
        ],
    )?;
    let mut out = String::new();
    let mut rest = pattern.as_str();
    while let Some(at) = rest.find([CAMPAIGN_SLOT, TEXT_SLOT]) {
        if at > 0 {
            let _ = write!(out, "#({})", ts(&rest[..at]));
        }
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix(CAMPAIGN_SLOT) {
            let _ = write!(out, "#campaign({})", ts(&line.campaign));
            rest = after;
        } else if let Some(after) = rest.strip_prefix(TEXT_SLOT) {
            out.push_str(&markup(campaigns, &line.text));
            rest = after;
        }
    }
    if !rest.is_empty() {
        let _ = write!(out, "#({})", ts(rest));
    }
    if let Some(cause) = &line.cause {
        let _ = write!(out, " \\\n{}", muted(9.0, &markup(campaigns, cause)));
    }
    Ok(format!("[{out}]"))
}

fn change_item(text: &Text, campaigns: &Campaigns, change: &DatedChange) -> Result<String> {
    let line = markup(
        campaigns,
        &say!(
            text,
            export_change,
            date = &change.date,
            text = &change.text
        )?,
    );
    Ok(match &change.follow_up {
        Some(follow_up) => format!("[{line} \\\n{}]", muted(8.5, &markup(campaigns, follow_up))),
        None => format!("[{line}]"),
    })
}

fn chart_block(
    out: &mut String,
    text: &Text,
    campaigns: &Campaigns,
    file: Option<String>,
    chart: &ChartImage,
) -> Result<()> {
    let image = file.map_or_else(String::new, |file| {
        format!("\n  #v(6pt)\n  #image(\"{file}\", width: 100%)")
    });
    let note = chart.note.as_ref().map_or_else(String::new, |note| {
        format!("\n  #v(4pt)\n  {}", muted(8.5, &markup(campaigns, note)))
    });
    let _ = writeln!(
        out,
        "#chart({}, [{}])[{image}{note}]",
        ts(&chart.title),
        markup(campaigns, &chart.sentence)
    );
    let heading = format!(
        "#block(above: 8pt, below: 4pt, sticky: true)[#text(size: 9.5pt, weight: \"bold\")[#({})]]",
        ts(text.written.export_account_changes)
    );
    match &chart.changes {
        ChartChanges::NotShown => {}
        ChartChanges::NoneInWindow => {
            let _ = writeln!(
                out,
                "{heading}\n{}",
                muted(
                    9.0,
                    &format!("#({})", ts(text.written.export_no_account_changes))
                )
            );
        }
        ChartChanges::Listed(changes) => {
            let items = changes
                .iter()
                .map(|change| change_item(text, campaigns, change))
                .collect::<Result<Vec<_>>>()?;
            let _ = writeln!(out, "{heading}\n#text(size: 9pt)[#list({})]", join(items));
        }
    }
    out.push_str("#v(10pt)\n");
    Ok(())
}

fn deep_heading(
    text: &Text,
    date: Option<&str>,
    note: &str,
    none: &str,
) -> Result<(String, String)> {
    Ok(match date {
        Some(date) => (say!(text, export_deep_check, date = date)?, note.to_owned()),
        None => (
            text.written.export_no_deep_check.to_owned(),
            none.to_owned(),
        ),
    })
}

fn fact_cell(fact: &Fact) -> String {
    let value = (!fact.value.is_empty())
        .then(|| format!("#text(weight: \"bold\")[#({})]", ts(&fact.value)));
    let mut lines = Vec::new();
    match fact.bar.as_ref().map(|bar| bar.drawn) {
        Some(Drawn::Score(score)) => {
            lines.push(format!("#score({score})"));
            lines.extend(value);
        }
        Some(Drawn::Share { share, mark }) => lines.push(format!(
            "#bar({share:.4}, {}) {}",
            mark.map_or_else(|| "none".into(), |mark| format!("{mark:.4}")),
            value.unwrap_or_default()
        )),
        None => lines.extend(value),
    }
    if !fact.detail.is_empty() {
        lines.push(format!(
            "#text(size: 8pt, fill: muted)[#({})]",
            ts(&fact.detail)
        ));
    }
    if let Some(badge) = &fact.badge {
        lines.push(format!("#text(size: 8pt)[#({})]", ts(&badge.text)));
    }
    let body = format!("[{}]", lines.join(" \\ "));
    if fact.wide {
        format!("table.cell(colspan: {{columns}}){body}")
    } else {
        body
    }
}

fn breakable(address: &str) -> String {
    let mut out = String::with_capacity(address.len());
    for c in address.chars() {
        out.push(c);
        if matches!(c, '/' | '?' | '&' | '=' | '-' | '_') {
            out.push('\u{200B}');
        }
    }
    out
}

fn section_block(out: &mut String, text: &Text, campaigns: &Campaigns, section: &Section) {
    let mut body = String::new();
    if !section.note.is_empty() {
        let _ = write!(
            body,
            "\n  #v(2pt)\n  {}",
            muted(8.5, &markup(campaigns, section.note))
        );
    }
    let _ = writeln!(
        out,
        "#chart({}, [{}])[{body}]",
        ts(section.title),
        markup(campaigns, &section.sentence)
    );
    if !section.rows.is_empty() {
        let count = section.columns.len();
        let _ = writeln!(
            out,
            "#table(columns: (1.3fr, {}), align: left, table.header({}),",
            vec!["1fr"; count].join(", "),
            join(
                std::iter::once(section.name)
                    .chain(section.columns.iter().copied())
                    .map(|header| format!("head({})", ts(header)))
            ),
        );
        for row in &section.rows {
            let mut name = vec![if section.url {
                format!("#text(size: 8pt)[#({})]", ts(&breakable(&row.name)))
            } else {
                format!("#({})", ts(&row.name))
            }];
            name.extend(
                row.tags
                    .iter()
                    .map(|tag| (*tag).to_owned())
                    .chain(row.badge.iter().map(|badge| badge.text.clone()))
                    .map(|text| format!("#text(size: 8pt, fill: muted)[#({})]", ts(&text))),
            );
            let _ = writeln!(
                out,
                "  [{}], {},",
                name.join(" \\ "),
                join(
                    row.facts
                        .iter()
                        .map(|fact| { fact_cell(fact).replace("{columns}", &count.to_string()) })
                )
            );
        }
        out.push_str(")\n");
    }
    if !section.key.is_empty() {
        let _ = writeln!(
            out,
            "#v(3pt)\n#text(size: 8pt, fill: muted)[#box(baseline: 1.5pt, rect(width: 1pt, height: 9pt, fill: ink)) #h(3pt) #({})]",
            ts(&section.key)
        );
    }
    if !section.actions.is_empty() {
        let _ = writeln!(
            out,
            "#block(above: 8pt, below: 3pt, sticky: true)[#text(size: 9pt, weight: \"bold\")[#({})]]\n#text(size: 9pt)[#list({})]",
            ts(text.written.export_what_to_do),
            join(
                section
                    .actions
                    .iter()
                    .map(|action| format!("[{}]", markup(campaigns, action)))
            )
        );
    }
    if !section.limitation.is_empty() {
        let _ = writeln!(
            out,
            "#v(3pt)\n#text(size: 8pt, fill: muted)[*#({})* #({})]",
            ts(text.written.export_data_limitation),
            ts(section.limitation)
        );
    }
    out.push_str("#v(14pt)\n");
}

fn preamble(out: &mut String, export: &Export, text: &Text) {
    let _ = write!(
        out,
        r##"#let accent = rgb({accent})
#let ink = rgb("#{INK}")
#let muted = rgb("#{MUTED}")
#let rule = rgb("#{RULE}")
#let canvas = rgb("#{CANVAS}")
#set document(title: {title})
#set page(paper: "a4", margin: (x: 17mm, top: 15mm, bottom: 18mm), footer: context {{
  set text(size: 7.5pt, fill: muted)
  [#({footer}) #h(1fr) #counter(page).display("1 / 1", both: true)]
}})
#set text(font: "{FONT}", size: 9.5pt, fill: ink, lang: "{lang}", number-width: "tabular")
#set par(leading: 0.55em, spacing: 0.75em)
#set list(indent: 2pt, body-indent: 6pt, spacing: 0.6em)
#set table(stroke: (x: none, y: 0.5pt + rule), inset: (x: 5pt, y: 5pt))
#show table.cell.where(y: 0): set text(size: 8pt, fill: muted)
#let section(title) = block(above: 24pt, below: 10pt, width: 100%, breakable: false, sticky: true)[
  #text(size: 15pt, weight: "bold")[#title]
  #v(-2pt)
  #line(length: 100%, stroke: 1.5pt + accent)
]
#let subsection(title, note) = block(above: 14pt, below: 10pt, width: 100%, breakable: false, sticky: true)[
  #text(size: 12pt, weight: "bold")[#title] \
  #text(size: 9pt, fill: muted)[#note]
]
#let chart(title, sentence, body) = block(width: 100%, breakable: false, above: 6pt, below: 6pt)[
  #text(size: 10.5pt, weight: "bold")[#title]
  #v(1pt)
  #text(size: 9pt, fill: muted)[#sentence]
  #body
]
#let metric(label, value, change, spark) = block(width: 100%, fill: canvas, radius: 4pt, inset: (x: 8pt, y: 7pt), breakable: false)[
  #text(size: 8pt, fill: muted)[#label] \
  #text(size: 17pt, weight: "bold")[#value] \
  #text(size: 7.5pt, fill: muted)[#change]
  #if spark != none {{ v(3pt); image(spark, width: 100%, height: 7mm, fit: "stretch") }}
]
#let head(t) = t
#let was(value, before) = [#value \ #text(size: 7.5pt, fill: muted)[#before]]
#let score(value) = box(for part in range(10) {{
  box(width: 4pt, height: 6pt, radius: 1pt, fill: if part < value {{ accent }} else {{ rule }})
  h(1pt)
}})
#let bar(share, mark) = box(width: 54pt, height: 5pt, radius: 2.5pt, fill: rule, {{
  place(box(width: share * 54pt, height: 5pt, radius: 2.5pt, fill: accent))
  if mark != none {{ place(dx: mark * 54pt - 0.5pt, dy: -2pt, rect(width: 1pt, height: 9pt, fill: ink)) }}
}})
#let campaign(name) = highlight(fill: rgb("#{CHIP_FILL}"), extent: 1.5pt, radius: 2pt)[#text(fill: rgb("#{CHIP_TEXT}"), weight: "bold")[#name]]
#let labelled(label, body) = grid(columns: (24mm, 1fr), column-gutter: 8pt, text(size: 8pt, fill: muted)[#label], body)
#let recommendation(finding, impact, action, evidence, measure: none, metadata: []) = block(width: 100%, breakable: false, below: 10pt, stroke: (left: 2pt + accent), inset: (left: 10pt, y: 3pt))[
  #text(size: 10.5pt, weight: "bold")[#action]
  #v(3pt)
  #if finding != none {{ labelled({finding_label}, finding); v(2pt) }}
  #labelled({impact_label}, impact)
  #if measure != none {{ v(2pt); labelled({measure_label}, measure) }}
  #v(2pt)
  #labelled({evidence_label}, text(fill: muted)[#evidence])
  #v(2pt)
  #metadata
]
"##,
        accent = ts(&export.accent),
        title = ts(&export.name),
        footer = ts(if export.sample {
            text.written.export_sample_footer
        } else {
            text.written.export_footer
        }),
        lang = text.written.format.locale,
        finding_label = ts(text.recommendation_finding),
        impact_label = ts(text.recommendation_impact),
        measure_label = ts(text.written.recommendation_measure),
        evidence_label = ts(text.recommendation_evidence),
    );
}

fn header(out: &mut String, export: &Export, text: &Text) -> Result<()> {
    out.push_str("#rect(width: 100%, height: 4pt, fill: accent, stroke: none)\n#v(8pt)\n");
    let (title, account) = match &export.subject {
        Subject::AdAccount { .. } => (export.name.as_str(), None),
        Subject::Campaign { name, .. } => (name.as_str(), Some(export.name.as_str())),
    };
    let account = account.map_or_else(String::new, |account| {
        format!(" \\\n#text(size: 11pt, fill: muted)[#({})]", ts(account))
    });
    let title = format!(
        "#text(size: 24pt, weight: \"bold\")[#({title})]{account} \\\n#text(size: 9pt, fill: muted)[#({sub})]",
        title = ts(title),
        sub = ts(&say_plural!(
            text,
            export_period,
            u64::try_from(export.window_days).unwrap_or_default(),
            range = &export.range,
            compared = &export.compared,
            prepared = &export.prepared,
        )?)
    );
    if export.logo.is_some() {
        let _ = writeln!(
            out,
            "#grid(columns: (1fr, auto), column-gutter: 12pt, align: horizon, [{title}], image(\"logo.png\", height: 16mm))"
        );
    } else {
        let _ = writeln!(out, "{title}");
    }
    if export.sample {
        let _ = writeln!(
            out,
            "#v(6pt)\n#text(weight: \"bold\", fill: accent)[#({})]",
            ts(text.written.export_sample_label)
        );
    }
    if let Some(notice) = &export.notice {
        let _ = writeln!(
            out,
            "#block(width: 100%, above: 14pt, fill: canvas, radius: 4pt, inset: 10pt, stroke: (left: 3pt + accent))[#text(weight: \"bold\")[#({})] \\\n#text(size: 9pt)[#({})]]",
            ts(notice.title),
            ts(notice.text)
        );
    }
    Ok(())
}

fn assessment_section(
    out: &mut String,
    text: &Text,
    finder: &Campaigns,
    assessment: &view::AssessmentText,
) {
    let _ = writeln!(out, "#section({})", ts(text.written.assessment_title));
    let _ = writeln!(
        out,
        "{}\n",
        muted(8.0, &format!("#({})", ts(&assessment.period)))
    );
    let _ = writeln!(
        out,
        "#strong[#({})]\n\n{}\n",
        ts(&format!(
            "{}: {}",
            text.written.assessment_result, assessment.result
        )),
        markup(finder, &assessment.headline)
    );
    for (label, items) in &assessment.sections {
        let _ = writeln!(out, "#strong[#({})]", ts(label));
        for item in items {
            let _ = writeln!(out, "- {}", markup(finder, &item.observation));
            if let Some(explanation) = &item.explanation {
                let _ = writeln!(
                    out,
                    "  #linebreak() {}",
                    muted(
                        9.0,
                        &format!(
                            "#({}): {}",
                            ts(text.written.assessment_explanation),
                            markup(finder, explanation)
                        )
                    )
                );
            }
        }
        out.push('\n');
    }
    if !assessment.sources.is_empty() {
        let _ = writeln!(
            out,
            "{}",
            muted(
                8.0,
                &format!(
                    "#({})",
                    ts(&format!(
                        "{}: {}",
                        text.market_sources,
                        assessment.sources.join(", ")
                    ))
                )
            )
        );
    }
}

fn update_section(
    out: &mut String,
    text: &Text,
    finder: &Campaigns,
    update: Option<&UpdateText>,
) -> Result<()> {
    let _ = writeln!(out, "#section({})", ts(text.written.export_update));
    let Some(update) = update else {
        let _ = writeln!(
            out,
            "{}",
            muted(9.5, &format!("#({})", ts(text.written.export_no_update)))
        );
        return Ok(());
    };
    let cause = update.cause.as_deref().map_or_else(String::new, |cause| {
        format!("\n  #v(2pt)\n  {}", muted(9.0, &markup(finder, cause)))
    });
    let mut items = update
        .lines
        .iter()
        .map(|line| campaign_line(text, finder, line))
        .collect::<Result<Vec<_>>>()?;
    if update.more_campaigns > 0 {
        items.push(format!(
            "[{}]",
            muted(
                9.0,
                &format!(
                    "#({})",
                    ts(&text.plural(
                        &text.update_more_campaigns,
                        u64::try_from(update.more_campaigns).unwrap_or_default(),
                        &[]
                    )?)
                )
            )
        ));
    }
    let lines = if items.is_empty() {
        String::new()
    } else {
        format!("\n  #v(4pt)\n  #list({})", join(items))
    };
    let _ = writeln!(
        out,
        "#block(width: 100%, fill: canvas, radius: 6pt, inset: 12pt)[\n  #text(size: 8pt, weight: \"bold\", fill: muted)[#({})]\n  #v(2pt)\n  #text(size: 12pt)[{}]{cause}{lines}\n  #v(4pt)\n  {}\n]",
        ts(&update.date),
        markup(finder, &update.summary),
        muted(7.5, &format!("#({})", ts(text.update_comparison)))
    );
    Ok(())
}

fn campaign_updates_section(
    out: &mut String,
    text: &Text,
    finder: &Campaigns,
    lines: &[(String, String)],
) {
    let _ = writeln!(
        out,
        "#section({})",
        ts(text.written.export_campaign_updates)
    );
    if lines.is_empty() {
        let _ = writeln!(
            out,
            "{}",
            muted(
                9.5,
                &format!("#({})", ts(text.written.export_no_campaign_updates))
            )
        );
    } else {
        dated_rows(out, finder, lines);
    }
}

fn dated_rows(out: &mut String, finder: &Campaigns, rows: &[(String, String)]) {
    let _ = writeln!(
        out,
        "#text(size: 9pt)[#grid(columns: (40mm, 1fr), column-gutter: 10pt, row-gutter: 8pt, {})]",
        join(rows.iter().map(|(date, text)| format!(
            "text(size: 8.5pt, weight: \"bold\", fill: muted)[#({})], [{}]",
            ts(date),
            markup(finder, text)
        )))
    );
}

fn metrics_section(out: &mut String, export: &Export, text: &Text) {
    let _ = writeln!(out, "#section({})", ts(text.written.export_metrics));
    let columns = if export.cells.len() > METRIC_COLUMNS {
        METRIC_COLUMNS - 1
    } else {
        export.cells.len().max(1)
    };
    let spark = |label: &str| {
        export
            .sparklines
            .iter()
            .position(|(drawn, _)| *drawn == label)
            .map_or_else(|| "none".to_owned(), |index| ts(&metric_spark_file(index)))
    };
    let _ = writeln!(
        out,
        "#grid(columns: ({}), column-gutter: 8pt, row-gutter: 8pt, {})",
        vec!["1fr"; columns].join(", "),
        join(export.cells.iter().map(|cell| format!(
            "metric({}, {}, {}, {})",
            ts(cell.label),
            ts(&cell.value),
            ts(&cell.change),
            spark(cell.label)
        )))
    );
}

fn recommendations_section(out: &mut String, export: &Export, text: &Text, finder: &Campaigns) {
    let _ = writeln!(out, "#section({})", ts(text.written.export_recommendations));
    if export.recommendations.is_empty() {
        let none = match export.subject {
            Subject::AdAccount { .. } => text.written.export_no_recommendations,
            Subject::Campaign { .. } => text.written.export_no_campaign_recommendations,
        };
        let _ = writeln!(out, "{}", muted(9.5, &format!("#({})", ts(none))));
    }
    for recommendation in &export.recommendations {
        let _ = writeln!(
            out,
            "#recommendation({}, [{}], [{}], [{}]{}, metadata: [{}])",
            recommendation.finding.as_ref().map_or_else(
                || "none".to_owned(),
                |finding| format!("[{}]", markup(finder, finding))
            ),
            markup(finder, &recommendation.reason),
            markup(finder, &recommendation.action),
            markup(finder, &recommendation.evidence),
            if recommendation.measure.trim().is_empty() {
                String::new()
            } else {
                format!(", measure: [{}]", markup(finder, &recommendation.measure))
            },
            muted(
                8.0,
                &format!(
                    "#({}){}",
                    ts(view::confidence(text, recommendation.confidence)),
                    if recommendation.subject.trim().is_empty() {
                        String::new()
                    } else {
                        format!(
                            " #linebreak() #({})",
                            ts(&format!(
                                "{}: {}",
                                text.recommendation_subject, recommendation.subject
                            ))
                        )
                    }
                )
            )
        );
    }
}

fn campaigns_section(
    out: &mut String,
    text: &Text,
    headers: &[&'static str],
    campaigns: &[CampaignRow],
) {
    let _ = writeln!(out, "#section({})", ts(text.written.export_campaigns));
    if campaigns.is_empty() {
        let _ = writeln!(
            out,
            "{}",
            muted(9.5, &format!("#({})", ts(text.written.export_no_campaigns)))
        );
        return;
    }
    let mut columns = vec!["1fr", "20mm"];
    let mut aligns = vec!["left + horizon", "left + horizon"];
    columns.extend(headers.iter().map(|_| "auto"));
    aligns.extend(headers.iter().map(|_| "right + horizon"));
    let _ = writeln!(
        out,
        "#text(size: 8.5pt)[#table(columns: ({}), align: ({}), table.header({}),",
        columns.join(", "),
        aligns.join(", "),
        join(
            [text.ad_account_campaign_column, text.ad_account_spark_days]
                .iter()
                .chain(headers)
                .map(|header| format!("head({})", ts(header)))
        )
    );
    for (index, row) in campaigns.iter().enumerate() {
        let spark = row.sparkline.as_ref().map_or_else(
            || "[]".to_owned(),
            |_| {
                format!(
                    "image({}, width: 16mm, height: 5mm, fit: \"stretch\")",
                    ts(&campaign_spark_file(index))
                )
            },
        );
        let _ = writeln!(
            out,
            "  [#campaign({})], {spark}, {},",
            ts(&row.name),
            join(row.cells.iter().map(|cell| format!(
                "was({}, {})",
                ts(&cell.value),
                ts(&cell.was)
            )))
        );
    }
    out.push_str(")]\n");
}

fn earlier_section(out: &mut String, text: &Text, finder: &Campaigns, earlier: &[EarlierUpdate]) {
    if earlier.is_empty() {
        return;
    }
    let _ = writeln!(out, "#section({})", ts(text.earlier_updates));
    let rows = earlier
        .iter()
        .map(|update| (update.date.clone(), update.summary.clone()))
        .collect::<Vec<_>>();
    dated_rows(out, finder, &rows);
}

pub fn source(export: &Export, text: &Text) -> Result<String> {
    if !valid_colour(&export.accent) {
        return Err(Error::Message("Invalid accent colour".into()));
    }
    let mut out = String::new();
    preamble(&mut out, export, text);
    let finder = Campaigns::unlinked(export.campaign_names.iter().map(String::as_str));
    header(&mut out, export, text)?;

    match &export.subject {
        Subject::AdAccount {
            assessment, update, ..
        } => {
            if let Some(assessment) = assessment {
                assessment_section(&mut out, text, &finder, assessment);
            }
            update_section(&mut out, text, &finder, update.as_ref())?;
        }
        Subject::Campaign { lines, .. } => {
            campaign_updates_section(&mut out, text, &finder, lines);
        }
    }
    metrics_section(&mut out, export, text);
    recommendations_section(&mut out, export, text, &finder);
    if let Subject::Campaign { findings, .. } = &export.subject {
        if !findings.is_empty() {
            let _ = writeln!(
                out,
                "#block(above: 12pt, below: 6pt, sticky: true)[#text(size: 11pt, weight: \"bold\")[#({})]]\n#list({})",
                ts(text.findings_other_title),
                join(findings.iter().map(|finding| format!("[{}]", markup(&finder, finding))))
            );
        }
    }
    if let Subject::AdAccount {
        campaign_headers,
        campaigns,
        ..
    } = &export.subject
    {
        campaigns_section(&mut out, text, campaign_headers, campaigns);
    }

    let drawn = export.drawn();
    if !drawn.is_empty() {
        let _ = writeln!(out, "#section({})", ts(text.written.export_charts));
    }
    let file = |index: usize| drawn[index].svg.is_some().then(|| chart_file(index));
    for (index, chart) in export.charts.iter().enumerate() {
        chart_block(&mut out, text, &finder, file(index), chart)?;
    }
    match &export.subject {
        Subject::AdAccount {
            deep_charts,
            earlier,
            ..
        } => {
            let (title, note) = deep_heading(
                text,
                deep_charts.date.as_deref(),
                text.written.export_deep_check_note,
                text.written.export_no_deep_check_charts,
            )?;
            let _ = writeln!(out, "#subsection({}, {})", ts(&title), ts(&note));
            for (offset, chart) in deep_charts.charts.iter().enumerate() {
                chart_block(
                    &mut out,
                    text,
                    &finder,
                    file(export.charts.len() + offset),
                    chart,
                )?;
            }
            earlier_section(&mut out, text, &finder, earlier);
        }
        Subject::Campaign { deep_check, .. } => match deep_check {
            Some(deep_check) => {
                let note = if deep_check.sections.len() < 2 {
                    text.written.export_campaign_deep_check_empty
                } else {
                    text.written.export_campaign_deep_check_note
                };
                let _ = writeln!(
                    out,
                    "#section({})",
                    ts(&say!(text, export_deep_check, date = &deep_check.date)?)
                );
                let _ = writeln!(out, "{}\n#v(8pt)", muted(9.0, &format!("#({})", ts(note))));
                for section in &deep_check.sections {
                    section_block(&mut out, text, &finder, section);
                }
            }
            None => {
                let _ = writeln!(out, "#section({})", ts(text.written.export_no_deep_check));
                let _ = writeln!(
                    out,
                    "{}",
                    muted(
                        9.5,
                        &format!("#({})", ts(text.written.export_campaign_no_deep_check))
                    )
                );
            }
        },
    }
    Ok(out)
}

pub fn pdf(export: &Export, text: &Text) -> Result<Vec<u8>> {
    let engine = TypstEngine::builder()
        .main_file(source(export, text)?)
        .fonts([
            include_bytes!("../../assets/static/fonts/LiberationSans-Regular.ttf").as_slice(),
            include_bytes!("../../assets/static/fonts/LiberationSans-Bold.ttf").as_slice(),
        ]);
    let drawn = export.drawn();
    let names = (0..drawn.len()).map(chart_file).collect::<Vec<_>>();
    let sparklines = export.sparkline_files();
    let mut files = drawn
        .iter()
        .zip(&names)
        .filter_map(|(chart, name)| Some((name.as_str(), chart.svg.as_ref()?.as_bytes())))
        .collect::<Vec<_>>();
    files.extend(
        sparklines
            .iter()
            .map(|(name, svg)| (name.as_str(), svg.as_bytes())),
    );
    files.extend(export.logo.as_deref().map(|logo| ("logo.png", logo)));
    let engine = if files.is_empty() {
        engine
    } else {
        engine.with_static_file_resolver(files)
    };
    let document: PagedDocument = engine.build().compile().output.map_err(|error| {
        tracing::warn!(?error, "Export source did not compile");
        export_error()
    })?;
    typst_pdf::pdf(&document, &Default::default()).map_err(|_| export_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        language::english,
        monitoring::{
            audit::{Audit, AuditRow, Counts},
            charts::{self, Frame, Part},
            deep_view,
        },
        workspace::monitoring::{KeywordQuality, KeywordScore},
    };

    fn deep_sections() -> Vec<Section> {
        let score = |keyword: &str, score: Option<i64>| KeywordScore {
            quality: KeywordQuality {
                campaign_id: 701,
                ad_group_id: 55,
                criterion_id: 1,
                keyword: keyword.into(),
                match_type: "EXACT".into(),
                cost: 10.0,
                score,
                ad_relevance: score.map(|_| "AVERAGE".into()),
                landing_page: score.map(|_| "ABOVE_AVERAGE".into()),
                expected_ctr: score.map(|_| "BELOW_AVERAGE".into()),
            },
            previous: None,
            known_before: false,
        };
        let row = |name: &str, id, cost: f64, clicks: f64, conversions: f64| AuditRow {
            name: name.into(),
            id,
            counts: Counts {
                impressions: clicks * 10.0,
                clicks,
                conversions,
                cost,
                value: 0.0,
            },
            ..Default::default()
        };
        deep_view::sections(
            english(),
            deep_view::quality(
                english(),
                &[
                    score("emergency dentist", Some(5)),
                    score("tooth pain", None),
                ],
                false,
            )
            .unwrap(),
            Some(&Audit {
                campaigns: vec![row("Search - Emergency", Some(701), 4000.0, 2000.0, 100.0)],
                search_terms: vec![
                    row("emergency dentist", None, 400.0, 200.0, 12.0),
                    row("free dental care", None, 96.0, 61.0, 0.0),
                ],
                landing_pages: vec![row(
                    "https://harbourdental.example/emergency",
                    None,
                    600.0,
                    300.0,
                    30.0,
                )],
                ..Default::default()
            }),
            701,
            "USD",
            true,
        )
        .unwrap()
    }

    fn image(chart: &charts::Chart, frame: Frame) -> ChartImage {
        ChartImage {
            title: chart.title.clone(),
            sentence: chart.sentence.words(),
            svg: charts::standalone(chart, frame),
            note: None,
            changes: ChartChanges::NotShown,
        }
    }

    fn cells() -> Vec<Cell> {
        vec![Cell {
            label: "Spend",
            value: "$2,765".into(),
            change: "no change".into(),
        }]
        .into_iter()
        .cycle()
        .take(4)
        .collect()
    }

    fn row(name: &str, judged: bool) -> CampaignRow {
        let cell = |value: &str, was: &str| view::Cell {
            value: value.into(),
            was: was.into(),
        };
        let mut cells = vec![cell("$560", "was $540")];
        if judged {
            cells.push(cell("42", "was 40"));
        }
        cells.extend([cell("280", "was 270"), cell("2,800", "was 2,700")]);
        CampaignRow {
            name: name.into(),
            sparkline: None,
            cells,
        }
    }

    fn export(judged: bool) -> Export {
        let mut headers = vec!["Spend"];
        if judged {
            headers.push("Conversions");
        }
        headers.extend(["Clicks", "Impressions"]);
        Export {
            sample: false,
            name: "Harbour \"Dental\"".into(),
            accent: "#5126A6".into(),
            logo: None,
            window_days: 7,
            range: "5 to 11 October 2026".into(),
            compared: "28 September to 4 October 2026".into(),
            prepared: "12 October 2026".into(),
            campaign_names: ["Search - Brand", "Search - Emergency", "Tom & <b>Jerry</b>"]
                .map(String::from)
                .into(),
            notice: None,
            cells: cells(),
            sparklines: Vec::new(),
            charts: Vec::new(),
            recommendations: vec![RecommendationText {
                subject: String::new(),
                confidence: Default::default(),
                measure: String::new(),
                finding: None,
                action: "Add jobs as a negative keyword.".into(),
                reason: "Job seekers are not patients.".into(),
                evidence: "jobs: $186 spend, 74 clicks.".into(),
            }],
            subject: Subject::AdAccount {
                assessment: None,
                update: Some(UpdateText {
                    date: "Monday 12 October".into(),
                    summary: "Spend was steady at $2,765.".into(),
                    cause: None,
                    lines: vec![UpdateLine {
                        campaign: "Search - Emergency".into(),
                        text: "210 clicks.".into(),
                        cause: None,
                    }],
                    more_campaigns: 0,
                }),
                earlier: Vec::new(),
                campaign_headers: headers,
                campaigns: vec![row("Search - Brand", judged)],
                deep_charts: DeepCharts {
                    date: None,
                    charts: Vec::new(),
                },
            },
        }
    }

    #[test]
    fn a_user_reads_the_assessment_before_the_update_and_the_recommendation_measure() {
        let mut export = export(true);
        export.notice = Some(
            view::tracking_notice(
                english(),
                super::super::tracking::ConversionTracking::NotSetUp,
                None,
                false,
            )
            .unwrap(),
        );
        if let Subject::AdAccount { assessment, .. } = &mut export.subject {
            *assessment = Some(Box::new(view::AssessmentText {
                result: "Mixed",
                period: "The 30 days to 11 October 2026".into(),
                sources: Vec::new(),
                headline: "Search - Emergency delivered more enquiries.".into(),
                sections: vec![
                    ("What worked", vec!["42 conversions.".into()]),
                    ("Next 30 days", vec!["Review search terms.".into()]),
                ],
            }));
        }
        export.recommendations[0].measure = "Compare conversions after 30 days.".into();
        let source = source(&export, english()).unwrap();
        let notice = source.find("#section(\"Assessment\")").unwrap();
        assert!(source.find("Conversion tracking is not set up").unwrap() < notice);
        assert!(notice < source.find("#section(\"Update\")").unwrap());
        assert!(source.contains("The 30 days to 11 October 2026"));
        assert!(source.contains("#campaign(\"Search - Emergency\")"));
        assert!(source.contains("#strong[#(\"What worked\")]"));
        assert!(source.contains("measure: [#(\"Compare conversions after 30 days.\")]"));
        compiles(&export);
    }

    #[test]
    fn an_ad_account_export_labels_fictional_data_only_when_it_is_a_sample() {
        let text = crate::language::Language::En.text().unwrap();
        let mut document = export(true);
        let real = source(&document, text).unwrap();
        assert!(!real.contains("Sample report"));
        assert!(!real.contains("Fictional data"));
        assert!(real.contains(text.written.export_footer));
        document.sample = true;
        let sample = source(&document, text).unwrap();
        assert!(sample.contains("Sample report. Fictional data."));
        assert!(sample.contains("Sample report · Fictional data · keysend.app"));
        assert!(!sample.contains(text.written.export_footer));
    }

    fn campaign_export() -> Export {
        Export {
            subject: Subject::Campaign {
                name: "Search - Emergency".into(),
                lines: vec![("Monday 12 October".into(), "Spend rose to $1,810.".into())],
                findings: Vec::new(),
                deep_check: Some(CampaignDeepCheck {
                    date: "12 October".into(),
                    sections: deep_sections(),
                }),
            },
            ..export(true)
        }
    }

    fn compiles(export: &Export) {
        assert!(pdf(export, english()).unwrap().starts_with(b"%PDF-"));
    }

    #[test]
    fn a_source_without_conversions_states_once_that_results_cannot_be_judged() {
        let mut unjudged = export(false);
        unjudged.notice = Some(
            view::tracking_notice(
                english(),
                crate::monitoring::tracking::ConversionTracking::Working,
                Some(false),
                false,
            )
            .unwrap(),
        );
        let source = source(&unjudged, english()).unwrap();
        assert!(!source.contains("Conversions"));
        assert_eq!(source.matches("cannot be judged").count(), 1, "{source}");
        assert!(!source.contains("Results cannot be judged."), "{source}");
        assert!(source.contains("head(\"Impressions\")"));
        assert!(source.contains("#section(\"Recommendations\")"));
        assert!(source.contains("\"Harbour \\\"Dental\\\"\""));
    }

    #[test]
    fn a_campaign_name_is_a_chip_in_every_text_and_a_heading_stays_plain() {
        let mut with = export(true);
        with.recommendations = vec![RecommendationText {
            subject: String::new(),
            confidence: Default::default(),
            measure: String::new(),
            finding: Some("Search - Brand has a high cost per conversion".into()),
            action: "Pause Tom & <b>Jerry</b> today.".into(),
            reason: "Search - Brand has the same clicks.".into(),
            evidence: "Search - Emergency: $186 spend.".into(),
        }];
        with.charts = vec![ChartImage {
            title: "Spend and conversions by Campaign, last 7 days".into(),
            sentence: "Search - Emergency takes 30% of spend.".into(),
            svg: None,
            note: Some("Note on Search - Brand.".into()),
            changes: ChartChanges::of(vec![DatedChange {
                date: "5 October".into(),
                text: "Budget of Search - Brand changed".into(),
                follow_up: None,
            }]),
        }];
        if let Subject::AdAccount {
            update, campaigns, ..
        } = &mut with.subject
        {
            let update = update.as_mut().unwrap();
            update.summary = "Search - Emergency and Tom & <b>Jerry</b> moved.".into();
            campaigns[0].name = "Tom & <b>Jerry</b>".into();
        }
        let source = source(&with, english()).unwrap();
        for expected in [
            r##"#let campaign(name) = highlight(fill: rgb("#EEE9F6"), extent: 1.5pt, radius: 2pt)[#text(fill: rgb("#3F1D85"), weight: "bold")[#name]]"##,
            r##"#text(size: 12pt)[#campaign("Search - Emergency")#(" and ")#campaign("Tom & <b>Jerry</b>")#(" moved.")]"##,
            r##"#list([#campaign("Search - Emergency")#(": ")#("210 clicks.")])"##,
            r##"  [#campaign("Tom & <b>Jerry</b>")], [], was("$560", "was $540"), was("42", "was 40"), was("280", "was 270"), was("2,800", "was 2,700"),"##,
            r##"#recommendation([#campaign("Search - Brand")#(" has a high cost per conversion")], [#campaign("Search - Brand")#(" has the same clicks.")], [#("Pause ")#campaign("Tom & <b>Jerry</b>")#(" today.")], [#campaign("Search - Emergency")#(": $186 spend.")],"##,
            r##"#chart("Spend and conversions by Campaign, last 7 days", [#campaign("Search - Emergency")#(" takes 30% of spend.")])"##,
            r##"#text(size: 8.5pt, fill: muted)[#("Note on ")#campaign("Search - Brand")#(".")]"##,
            r##"#list([#("5 October: Budget of ")#campaign("Search - Brand")#(" changed.")])"##,
        ] {
            assert!(source.contains(expected), "{expected}\n{source}");
        }
        compiles(&with);

        let mut one = campaign_export();
        if let Subject::Campaign { lines, .. } = &mut one.subject {
            lines.push((
                "Sunday 11 October".into(),
                "Search - Emergency lost budget to Search - Brand.".into(),
            ));
        }
        let source = super::source(&one, english()).unwrap();
        assert!(
            source.contains(r##"#text(size: 24pt, weight: "bold")[#("Search - Emergency")] \"##)
        );
        assert!(source.contains(r##"[#("Spend rose to $1,810.")]"##));
        assert!(source.contains(
            r##"[#campaign("Search - Emergency")#(" lost budget to ")#campaign("Search - Brand")#(".")]"##
        ), "{source}");
    }

    #[test]
    fn an_update_line_names_its_campaign_and_states_its_cause_and_the_campaigns_left_out() {
        let mut with = export(true);
        if let Subject::AdAccount {
            update: Some(update),
            ..
        } = &mut with.subject
        {
            update.lines[0].cause =
                Some("Confirmed cause: Daily budget changed on 7 October.".into());
            update.more_campaigns = 2;
        }
        let source = source(&with, english()).unwrap();
        assert!(source.contains(
            "[#campaign(\"Search - Emergency\")#(\": \")#(\"210 clicks.\") \\\n#text(size: 9pt, fill: muted)[#(\"Confirmed cause: Daily budget changed on 7 October.\")]]"
        ), "{source}");
        assert!(source.contains(
            "[#text(size: 9pt, fill: muted)[#(\"We detected changes in 2 more Campaigns.\")]]"
        ));
        assert!(source
            .contains("#(\"An Update always compares the last 7 days with the 7 days before.\")"));
        compiles(&with);
    }

    #[test]
    fn earlier_updates_follow_the_charts_with_their_dates() {
        let mut with = export(true);
        if let Subject::AdAccount { earlier, .. } = &mut with.subject {
            *earlier = vec![
                EarlierUpdate {
                    date: "Sunday 11 October".into(),
                    summary: "Search - Brand spent $80.".into(),
                },
                EarlierUpdate {
                    date: "Saturday 10 October".into(),
                    summary: "Nothing notable changed.".into(),
                },
            ];
        }
        let source = source(&with, english()).unwrap();
        let heading = source.find("#section(\"Earlier Updates\")").unwrap();
        assert!(heading > source.find("#subsection(\"No Deep check yet\"").unwrap());
        assert!(source[heading..].contains(
            "text(size: 8.5pt, weight: \"bold\", fill: muted)[#(\"Sunday 11 October\")], [#campaign(\"Search - Brand\")#(\" spent $80.\")], text(size: 8.5pt, weight: \"bold\", fill: muted)[#(\"Saturday 10 October\")], [#(\"Nothing notable changed.\")]"
        ), "{source}");
        compiles(&with);
        assert!(!super::source(&export(true), english())
            .unwrap()
            .contains("Earlier Updates"));
    }

    #[test]
    fn a_tracking_notice_stands_under_the_title() {
        let mut with = export(false);
        with.notice = Some(Notice {
            title: "Conversion tracking is not set up",
            text: "Google Ads does not record conversions.",
        });
        let source = source(&with, english()).unwrap();
        let notice = source
            .find("[#text(weight: \"bold\")[#(\"Conversion tracking is not set up\")] \\\n#text(size: 9pt)[#(\"Google Ads does not record conversions.\")]]")
            .unwrap();
        assert!(notice < source.find("#section(\"Update\")").unwrap());
        compiles(&with);
    }

    #[test]
    fn an_account_change_states_its_follow_up_below_it() {
        let mut with = campaign_export();
        with.charts = vec![ChartImage {
            title: "Spend per day".into(),
            sentence: "Spend was steady.".into(),
            svg: None,
            note: None,
            changes: ChartChanges::of(vec![DatedChange {
                date: "7 October".into(),
                text: "Daily budget changed from $50.00 to $80.00".into(),
                follow_up: Some("KeySend checks the result on 23 October.".into()),
            }]),
        }];
        if let Subject::Campaign { findings, .. } = &mut with.subject {
            findings.push("Search - Emergency spends on free dental care.".into());
        }
        let source = source(&with, english()).unwrap();
        assert!(source.contains(
            "#list([#(\"7 October: Daily budget changed from $50.00 to $80.00.\") \\\n#text(size: 8.5pt, fill: muted)[#(\"KeySend checks the result on 23 October.\")]])"
        ), "{source}");
        assert!(source.contains(
            "#text(size: 11pt, weight: \"bold\")[#(\"Other Findings\")]]\n#list([#campaign(\"Search - Emergency\")#(\" spends on free dental care.\")])"
        ), "{source}");
        compiles(&with);
    }

    #[test]
    fn sparklines_are_drawn_beside_their_metric_and_their_campaign() {
        let spark = charts::standalone_spark(&charts::sparkline(&[1.0, 3.0, 2.0], "Spend"));
        let mut with = export(true);
        with.sparklines = vec![("Spend", spark.clone())];
        if let Subject::AdAccount { campaigns, .. } = &mut with.subject {
            campaigns[0].sparkline = Some(spark);
        }
        let source = source(&with, english()).unwrap();
        assert!(source.contains("metric(\"Spend\", \"$2,765\", \"no change\", \"metric-1.svg\")"));
        assert!(source.contains(
            "[#campaign(\"Search - Brand\")], image(\"campaign-1.svg\", width: 16mm, height: 5mm, fit: \"stretch\"),"
        ), "{source}");
        compiles(&with);
    }

    #[test]
    fn a_source_with_conversions_has_a_conversion_column() {
        let source = source(&export(true), english()).unwrap();
        assert!(source.contains("head(\"Conversions\")"));
        assert!(!source.contains("cannot be judged"));
    }

    #[test]
    fn a_logo_is_drawn_only_when_the_ad_account_has_one() {
        let without = source(&export(true), english()).unwrap();
        assert!(!without.contains("image(\"logo.png\""));
        assert!(!without.contains("#grid(columns: (1fr, auto)"));
        compiles(&export(true));
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut with = export(true);
        with.logo = Some(png.into_inner());
        assert!(source(&with, english())
            .unwrap()
            .contains("image(\"logo.png\""));
        compiles(&with);
    }

    #[test]
    fn a_chart_has_its_sentence_above_its_image_and_a_chart_with_nothing_to_show_has_no_image() {
        let shares = charts::english_calls::shares(
            "Spend and conversions by Campaign, last 7 days",
            "Campaign",
            vec![
                Part {
                    name: "Search - Emergency".into(),
                    href: None,
                    spend: 1810.0,
                    results: 61.0,
                },
                Part {
                    name: "Display - Remarketing".into(),
                    href: None,
                    spend: 780.0,
                    results: 6.0,
                },
            ],
            "conversions",
            "USD",
        );
        let mut with = export(true);
        with.charts = vec![
            image(&shares, Frame::Shares),
            ChartImage {
                title: "Cost per conversion, 12 weeks".into(),
                sentence: "No week of the last 12 had conversions, so there is no cost per conversion to show.".into(),
                svg: None,
                note: Some("About 23% of conversions arrive late.".into()),
                changes: ChartChanges::NotShown,
            },
        ];
        let source = source(&with, english()).unwrap();
        assert!(source.contains(
            "#section(\"Charts\")\n#chart(\"Spend and conversions by Campaign, last 7 days\", [#(\"Display - Remarketing takes 30% of spend and brings 9% of conversions.\")])[\n  #v(6pt)\n  #image(\"chart-1.svg\", width: 100%)]"
        ), "{source}");
        assert_eq!(source.matches("#image(").count(), 1);
        assert!(source.contains("there is no cost per conversion to show.\")])[\n  #v(4pt)\n  #text(size: 8.5pt, fill: muted)[#(\"About 23% of conversions arrive late.\")]]"), "{source}");
        compiles(&with);
        assert!(!super::source(&export(true), english())
            .unwrap()
            .contains("Charts"));
    }

    #[test]
    fn the_charts_of_the_deep_check_follow_the_others_under_a_heading_with_its_date() {
        let mut with = export(true);
        let none = |title: &str| ChartImage {
            title: title.into(),
            sentence: "We have not read this yet.".into(),
            svg: None,
            note: None,
            changes: ChartChanges::NotShown,
        };
        with.charts = vec![none("Spend per day, 8 weeks")];
        if let Subject::AdAccount { deep_charts, .. } = &mut with.subject {
            deep_charts.date = Some("12 October".into());
            deep_charts.charts = vec![none("Conversions by hour and weekday, 30 days")];
        }
        let source = source(&with, english()).unwrap();
        let first = source.find("Spend per day, 8 weeks").unwrap();
        let heading = source
            .find("#subsection(\"From the Deep check of 12 October\"")
            .unwrap();
        let second = source
            .find("Conversions by hour and weekday, 30 days")
            .unwrap();
        assert!(first < heading && heading < second, "{source}");
    }

    #[test]
    fn an_update_in_the_export_states_its_cause_under_the_summary() {
        let mut export = export(true);
        if let Subject::AdAccount {
            update: Some(update),
            ..
        } = &mut export.subject
        {
            update.cause = Some(
                "Confirmed cause: Search - Brand accounts for 80% of the rise in Spend.".into(),
            );
        }
        let written = source(&export, english()).unwrap();
        assert!(
            written.contains(
                "#text(size: 12pt)[#(\"Spend was steady at $2,765.\")]\n  #v(2pt)\n  #text(size: 9pt, fill: muted)[#(\"Confirmed cause: \")#campaign(\"Search - Brand\")#(\" accounts for 80% of the rise in Spend.\")]"
            ),
            "{written}"
        );
        compiles(&export);
    }

    #[test]
    fn a_campaign_source_names_the_campaign_and_the_ad_account_and_has_no_campaign_table() {
        let source = source(&campaign_export(), english()).unwrap();
        assert!(source.contains(
            "#text(size: 24pt, weight: \"bold\")[#(\"Search - Emergency\")] \\\n#text(size: 11pt, fill: muted)[#(\"Harbour \\\"Dental\\\"\")] \\\n#text(size: 9pt, fill: muted)[#(\"Last 7 days, 5 to 11 October 2026. Compared with 28 September to 4 October 2026. Prepared on 12 October 2026.\")]"
        ), "{source}");
        assert!(source.contains("#section(\"Updates about this Campaign\")"));
        assert!(source.contains("Spend rose to $1,810."));
        assert!(!source.contains("#section(\"Campaigns\")"));
        assert!(source.contains("#section(\"From the Deep check of 12 October\")"));
        compiles(&campaign_export());
    }

    #[test]
    fn a_campaign_source_prints_each_section_of_the_deep_check_with_its_bars_and_its_advice() {
        let source = source(&campaign_export(), english()).unwrap();
        assert!(source.contains(
            "#chart(\"Quality score of the keywords with the most spend\", [#(\"The keyword with the most spend has a Quality score of 5.\")])[\n  #v(2pt)\n  #text(size: 8.5pt, fill: muted)[#(\"This is the first Deep check with Quality scores. The change of each score appears after the next Deep check.\")]]\n#table(columns: (1.3fr, 1fr, 1fr, 1fr, 1fr), align: left, table.header(head(\"Keyword\"), head(\"Quality score\"), head(\"Expected clickthrough rate\"), head(\"Ad relevance\"), head(\"Landing page experience\")),\n  [#(\"emergency dentist\") \\ #text(size: 8pt, fill: muted)[#(\"Exact match\")]], [#score(5) \\ #text(weight: \"bold\")[#(\"5 of 10\")]], [#text(size: 8pt)[#(\"Below average\")]], [#text(size: 8pt)[#(\"Average\")]], [#text(size: 8pt)[#(\"Above average\")]],\n  [#(\"tooth pain\") \\ #text(size: 8pt, fill: muted)[#(\"Exact match\")]], table.cell(colspan: 4)[#text(size: 8pt, fill: muted)[#(\"Google Ads gives no Quality score yet. It needs more information about this keyword. You do not need to do anything.\")]],\n)"
        ), "{source}");
        assert!(source.contains(
            "[#(\"free dental care\")], [#bar(0.2400, none) #text(weight: \"bold\")[#(\"$96.00\")]], [#text(weight: \"bold\")[#(\"61\")]], [#text(weight: \"bold\")[#(\"0\")] \\ #text(size: 8pt)[#(\"Spend, no conversions\")]],"
        ), "{source}");
        assert!(source.contains(
            "[#bar(1.0000, 0.5000) #text(weight: \"bold\")[#(\"10.0%\")] \\ #text(size: 8pt, fill: muted)[#(\"30 conversions from 300 clicks\")] \\ #text(size: 8pt)[#(\"Above the Campaign rate\")]]"
        ), "{source}");
        assert!(source.contains("#(\"Campaign rate: 5.0%\")]\n#block(above: 8pt, below: 3pt, sticky: true)[#text(size: 9pt, weight: \"bold\")[#(\"What you can do\")]]"), "{source}");
        assert!(
            source.contains("[*#(\"Data limitation:\")* #(\"These are Google Ads numbers."),
            "{source}"
        );
        assert!(
            source.contains("#text(size: 8pt)[#(\"harbourdental.example/\u{200B}emergency\")]"),
            "{source}"
        );
        assert_eq!(source.matches("first Deep check").count(), 1);
        assert!(!source.contains("Not available"));
        assert!(source.contains(
            "The Deep check found no keyword with numbers for this Campaign in these 30 days."
        ));
    }

    #[test]
    fn a_source_reads_every_word_from_the_text_that_it_is_given() {
        use crate::language::{en, Plural, Rule, Written};
        let english = en::TEXT;
        let other = Text {
            rule: Rule::Romanian,
            written: Written {
                export_metrics: "Metrici",
                export_update: "Noutate",
                export_recommendations: "Recomandări",
                export_footer: "Subsol",
                export_period: Plural::three(
                    "Ultima zi, {range}. Comparat cu {compared}. Pregătit pe {prepared}.",
                    "Ultimele {count} zile, {range}. Comparat cu {compared}. Pregătit pe {prepared}.",
                    "Ultimele {count} de zile, {range}. Comparat cu {compared}. Pregătit pe {prepared}.",
                ),
                ..english.written
            },
            update_campaign_line: "{text} ({campaign})",
            ..english
        };
        let mut with = export(true);
        with.window_days = 30;
        let source = source(&with, &other).unwrap();
        assert!(source.contains("#section(\"Metrici\")"), "{source}");
        assert!(source.contains("#section(\"Noutate\")"));
        assert!(source.contains("#section(\"Recomandări\")"));
        assert!(source.contains("[#(\"Subsol\")"));
        assert!(source.contains("Ultimele 30 de zile, 5 to 11 October 2026."));
        assert!(
            source.contains(
                "#list([#(\"210 clicks.\")#(\" (\")#campaign(\"Search - Emergency\")#(\")\")])"
            ),
            "{source}"
        );
        assert!(!source.contains("#section(\"Metrics\")"));
    }

    #[test]
    fn an_accent_colour_that_is_not_a_hex_colour_is_refused() {
        let mut bad = export(true);
        bad.accent = "red\")".into();
        assert!(source(&bad, english()).is_err());
    }
}

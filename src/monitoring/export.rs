use std::fmt::Write as _;

use loco_rs::{Error, Result};
use typst_as_lib::TypstEngine;
use typst_layout::PagedDocument;

use crate::language::{say, say_plural, Text};

use super::{
    campaign_text::{Campaigns, Kind},
    deep_view::{Drawn, Fact, Section},
    format::Cell,
};

const INK: &str = "292039";
const MUTED: &str = "5C5470";
const RULE: &str = "E4E2EC";
const CANVAS: &str = "F5F5F7";
const FONT: &str = "Liberation Sans";
const METRIC_COLUMNS: usize = 4;
const CHIP_TEXT: &str = "3F1D85";
const CHIP_FILL: &str = "EEE9F6";

pub struct UpdateText {
    pub date: String,
    pub summary: String,
    pub cause: Option<String>,
    pub lines: Vec<String>,
}

pub struct CampaignRow {
    pub name: String,
    pub spend: String,
    pub clicks: String,
    pub impressions: String,
    pub conversions: Option<String>,
}

pub struct RecommendationText {
    pub action: String,
    pub reason: String,
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
pub enum ChartChanges {
    NotShown,
    NoneInWindow,
    Listed(Vec<(String, String)>),
}

impl ChartChanges {
    pub fn of(dated: Vec<(String, String)>) -> Self {
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
        update: Option<UpdateText>,
        campaigns: Vec<CampaignRow>,
        deep_charts: DeepCharts,
    },
    Campaign {
        name: String,
        lines: Vec<(String, String)>,
        deep_check: Option<CampaignDeepCheck>,
    },
}

pub struct Export {
    pub name: String,
    pub accent: String,
    pub logo: Option<Vec<u8>>,
    pub window_days: i64,
    pub campaign_names: Vec<String>,
    pub range: String,
    pub compared: String,
    pub prepared: String,
    pub cells: Vec<Cell>,
    pub judged: bool,
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

fn join(items: impl IntoIterator<Item = String>) -> String {
    items.into_iter().collect::<Vec<_>>().join(", ")
}

fn chart_block(
    out: &mut String,
    text: &Text,
    campaigns: &Campaigns,
    file: Option<String>,
    chart: &ChartImage,
) -> Result<()> {
    let image = file.map_or_else(String::new, |file| {
        format!("\n#v(4pt)\n#image(\"{file}\", width: 100%)")
    });
    let note = chart.note.as_ref().map_or_else(String::new, |note| {
        format!(
            "\n#v(4pt)\n#text(size: 9pt, fill: muted)[{}]",
            markup(campaigns, note)
        )
    });
    let heading = || {
        format!(
            "\n#v(6pt)\n#text(size: 9.5pt, weight: \"bold\")[#({})]\n#v(2pt)\n",
            ts(text.written.export_account_changes)
        )
    };
    let changes = match &chart.changes {
        ChartChanges::NotShown => String::new(),
        ChartChanges::NoneInWindow => format!(
            "{}#text(size: 9pt, fill: muted)[#({})]",
            heading(),
            ts(text.written.export_no_account_changes)
        ),
        ChartChanges::Listed(changes) => {
            let items = changes
                .iter()
                .map(|(date, change)| {
                    Ok(format!(
                        "[{}]",
                        markup(
                            campaigns,
                            &say!(text, export_change, date = date, text = change)?
                        )
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            format!("{}#text(size: 9pt)[#list({})]", heading(), join(items))
        }
    };
    let _ = writeln!(
        out,
        "#block(width: 100%, breakable: false, below: 16pt)[#text(weight: \"bold\")[#({})] \\\n#text(fill: muted)[{}]{image}{note}{changes}]",
        ts(&chart.title),
        markup(campaigns, &chart.sentence)
    );
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
            "#text(size: 8.5pt, fill: muted)[#({})]",
            ts(&fact.detail)
        ));
    }
    if let Some(badge) = &fact.badge {
        lines.push(format!("#text(size: 8.5pt)[#({})]", ts(&badge.text)));
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
        if matches!(c, '/' | '?' | '&' | '=') {
            out.push('\u{200B}');
        }
    }
    out
}

fn section_block(out: &mut String, text: &Text, campaigns: &Campaigns, section: &Section) {
    let _ = write!(
        out,
        "#block(width: 100%, breakable: false, below: 16pt)[#text(weight: \"bold\")[#({})] \\\n#text(fill: muted)[{}]",
        ts(section.title),
        markup(campaigns, &section.sentence)
    );
    if !section.note.is_empty() {
        let _ = write!(
            out,
            " \\\n#text(fill: muted)[{}]",
            markup(campaigns, section.note)
        );
    }
    if !section.rows.is_empty() {
        let count = section.columns.len();
        let _ = writeln!(
            out,
            "\n#v(4pt)\n#table(columns: (1.3fr, {}), align: left, stroke: (x: none, y: 0.6pt + rule), inset: (x: 6pt, y: 5pt), table.header({}),",
            vec!["1fr"; count].join(", "),
            join(
                std::iter::once(section.name)
                    .chain(section.columns.iter().copied())
                    .map(|header| format!("head({})", ts(header)))
            ),
        );
        for row in &section.rows {
            let mut name = vec![if section.url {
                format!("#text(size: 8.5pt)[#({})]", ts(&breakable(&row.name)))
            } else {
                format!("#({})", ts(&row.name))
            }];
            name.extend(
                row.tags
                    .iter()
                    .map(|tag| (*tag).to_owned())
                    .chain(row.badge.iter().map(|badge| badge.text.clone()))
                    .map(|text| format!("#text(size: 8.5pt, fill: muted)[#({})]", ts(&text))),
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
        out.push(')');
    }
    if !section.key.is_empty() {
        let _ = write!(
            out,
            "\n#v(2pt)\n#text(size: 8.5pt, fill: muted)[#box(baseline: 1.5pt, rect(width: 1pt, height: 9pt, fill: ink)) #h(3pt) #({})]",
            ts(&section.key)
        );
    }
    if !section.actions.is_empty() {
        let _ = write!(
            out,
            "\n#v(4pt)\n#text(size: 9pt, weight: \"bold\")[#({})]\n#v(2pt)\n#text(size: 9pt)[#list({})]",
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
        let _ = write!(
            out,
            "\n#v(2pt)\n#text(size: 8.5pt, fill: muted)[*#({})* #({})]",
            ts(text.written.export_data_limitation),
            ts(section.limitation)
        );
    }
    out.push_str("]\n");
}

pub fn source(export: &Export, text: &Text) -> Result<String> {
    if !valid_colour(&export.accent) {
        return Err(Error::Message("Invalid accent colour".into()));
    }
    let mut out = String::new();
    let _ = write!(
        out,
        r##"#let accent = rgb({accent})
#let ink = rgb("#{INK}")
#let muted = rgb("#{MUTED}")
#let rule = rgb("#{RULE}")
#let canvas = rgb("#{CANVAS}")
#set page(paper: "a4", margin: (x: 18mm, top: 14mm, bottom: 20mm), footer: context {{
  set text(size: 8pt, fill: muted)
  [#({footer}) #h(1fr) #counter(page).display()]
}})
#set text(font: "{FONT}", size: 10pt, fill: ink, lang: "{lang}")
#set par(leading: 0.6em, spacing: 0.9em)
#let section(title) = block(above: 20pt, below: 8pt, width: 100%, breakable: false, sticky: true)[
  #text(size: 13pt, weight: "bold")[#title]
  #v(3pt)
  #line(length: 100%, stroke: 1.2pt + accent)
]
#let subsection(title, body) = block(above: 8pt, below: 10pt, width: 100%, breakable: false, sticky: true)[
  #text(size: 11pt, weight: "bold")[#title] \
  #text(fill: muted)[#body]
]
#let metric(label, value, change) = block(width: 100%)[
  #text(size: 8.5pt, fill: muted)[#label] \
  #text(size: 20pt, weight: "bold")[#value] \
  #text(size: 8.5pt, fill: muted)[#change]
]
#let head(t) = text(size: 8.5pt, fill: muted)[#t]
#let score(value) = box(for part in range(10) {{
  box(width: 4pt, height: 6pt, radius: 1pt, fill: if part < value {{ accent }} else {{ rule }})
  h(1pt)
}})
#let bar(share, mark) = box(width: 54pt, height: 5pt, radius: 2.5pt, fill: rule, {{
  place(box(width: share * 54pt, height: 5pt, radius: 2.5pt, fill: accent))
  if mark != none {{ place(dx: mark * 54pt - 0.5pt, dy: -2pt, rect(width: 1pt, height: 9pt, fill: ink)) }}
}})
#let campaign(name) = highlight(fill: rgb("#{CHIP_FILL}"), extent: 2pt, radius: 2pt)[#text(fill: rgb("#{CHIP_TEXT}"), weight: "bold")[#name]]
#let recommendation(action, reason, evidence) = block(width: 100%, breakable: false, below: 12pt)[
  #text(weight: "bold")[#action] \
  #reason \
  #text(fill: muted)[*#({finding})* #evidence]
]
"##,
        accent = ts(&export.accent),
        footer = ts(text.written.export_footer),
        finding = ts(text.written.export_finding),
        lang = text.written.format.locale,
    );
    let finder = Campaigns::unlinked(export.campaign_names.iter().map(String::as_str));
    out.push_str("#rect(width: 100%, height: 5pt, fill: accent, stroke: none)\n#v(10pt)\n");
    let (title, account) = match &export.subject {
        Subject::AdAccount { .. } => (export.name.as_str(), None),
        Subject::Campaign { name, .. } => (name.as_str(), Some(export.name.as_str())),
    };
    let account = account.map_or_else(String::new, |account| {
        format!(" \\\n#text(fill: muted)[#({})]", ts(account))
    });
    let title = format!(
        "#text(size: 24pt, weight: \"bold\")[#({title})]{account} \\\n#text(fill: muted)[#({sub})]",
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

    match &export.subject {
        Subject::AdAccount { update, .. } => {
            let _ = writeln!(out, "#section({})", ts(text.written.export_update));
            match update {
                Some(update) => {
                    let lines = if update.lines.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\n#v(2pt)\n#list({})",
                            join(
                                update
                                    .lines
                                    .iter()
                                    .map(|line| format!("[{}]", markup(&finder, line)))
                            )
                        )
                    };
                    let cause = update.cause.as_deref().map_or_else(String::new, |cause| {
                        format!(
                            " \\\n#text(size: 9.5pt, fill: muted)[{}]",
                            markup(&finder, cause)
                        )
                    });
                    let _ = writeln!(
                        out,
                        "#block(width: 100%, fill: canvas, radius: 6pt, inset: 12pt)[#text(size: 8.5pt, weight: \"bold\", fill: muted)[#({})] \\\n#text(size: 12pt)[{}]{cause}{lines}]",
                        ts(&update.date),
                        markup(&finder, &update.summary)
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "#text(fill: muted)[#({})]",
                        ts(text.written.export_no_update)
                    );
                }
            }
        }
        Subject::Campaign { lines, .. } => {
            let _ = writeln!(
                out,
                "#section({})",
                ts(text.written.export_campaign_updates)
            );
            if lines.is_empty() {
                let _ = writeln!(
                    out,
                    "#text(fill: muted)[#({})]",
                    ts(text.written.export_no_campaign_updates)
                );
            } else {
                let _ = writeln!(
                    out,
                    "#grid(columns: (auto, 1fr), column-gutter: 12pt, row-gutter: 7pt, {})",
                    join(lines.iter().map(|(date, text)| format!(
                        "text(size: 8.5pt, weight: \"bold\", fill: muted)[#({})], [{}]",
                        ts(date),
                        markup(&finder, text)
                    )))
                );
            }
        }
    }

    let _ = writeln!(out, "#section({})", ts(text.written.export_metrics));
    let columns = if export.cells.len() > METRIC_COLUMNS {
        METRIC_COLUMNS - 1
    } else {
        export.cells.len()
    };
    let _ = writeln!(
        out,
        "#grid(columns: ({}), column-gutter: 10pt, row-gutter: 12pt, {})",
        vec!["1fr"; columns].join(", "),
        join(export.cells.iter().map(|cell| format!(
            "metric({}, {}, {})",
            ts(cell.label),
            ts(&cell.value),
            ts(&cell.change)
        )))
    );
    if !export.judged {
        let _ = writeln!(
            out,
            "#v(6pt)\n#text(size: 9pt, fill: muted)[#({})]",
            ts(text.written.results_unjudged)
        );
    }

    let drawn = export.drawn();
    if !drawn.is_empty() {
        let _ = writeln!(out, "#section({})", ts(text.written.export_charts));
    }
    let file = |index: usize| drawn[index].svg.is_some().then(|| chart_file(index));
    for (index, chart) in export.charts.iter().enumerate() {
        chart_block(&mut out, text, &finder, file(index), chart)?;
    }
    if let Subject::AdAccount {
        deep_charts,
        campaigns,
        ..
    } = &export.subject
    {
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

        let _ = writeln!(out, "#section({})", ts(text.written.export_campaigns));
        if campaigns.is_empty() {
            let _ = writeln!(
                out,
                "#text(fill: muted)[#({})]",
                ts(text.written.export_no_campaigns)
            );
        } else {
            let mut headers = vec![
                text.written.export_campaign,
                text.written.metric_spend,
                text.written.metric_clicks,
                text.written.metric_impressions,
            ];
            let mut columns = vec!["1fr", "auto", "auto", "auto"];
            let mut aligns = vec!["left", "right", "right", "right"];
            if export.judged {
                headers.push(text.written.metric_conversions);
                columns.push("auto");
                aligns.push("right");
            }
            let _ = writeln!(
                out,
                "#table(columns: ({}), align: ({}), stroke: (x: none, y: 0.6pt + rule), inset: (x: 6pt, y: 5pt), table.header({}),",
                columns.join(", "),
                aligns.join(", "),
                join(headers.iter().map(|header| format!("head({})", ts(header))))
            );
            for row in campaigns {
                let mut cells = vec![&row.spend, &row.clicks, &row.impressions];
                if let Some(conversions) = &row.conversions {
                    cells.push(conversions);
                }
                let _ = writeln!(
                    out,
                    "  [#campaign({})], {},",
                    ts(&row.name),
                    join(cells.iter().map(|cell| ts(cell)))
                );
            }
            out.push_str(")\n");
        }
    }

    let _ = writeln!(out, "#section({})", ts(text.written.export_recommendations));
    if export.recommendations.is_empty() {
        let none = match export.subject {
            Subject::AdAccount { .. } => text.written.export_no_recommendations,
            Subject::Campaign { .. } => text.written.export_no_campaign_recommendations,
        };
        let _ = writeln!(out, "#text(fill: muted)[#({})]", ts(none));
    }
    for recommendation in &export.recommendations {
        let _ = writeln!(
            out,
            "#recommendation([{}], [{}], [{}])",
            markup(&finder, &recommendation.action),
            markup(&finder, &recommendation.reason),
            markup(&finder, &recommendation.evidence)
        );
    }

    if let Subject::Campaign { deep_check, .. } = &export.subject {
        match deep_check {
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
                let _ = writeln!(out, "#text(fill: muted)[#({})]\n#v(6pt)", ts(note));
                for section in &deep_check.sections {
                    section_block(&mut out, text, &finder, section);
                }
            }
            None => {
                let _ = writeln!(out, "#section({})", ts(text.written.export_no_deep_check));
                let _ = writeln!(
                    out,
                    "#text(fill: muted)[#({})]",
                    ts(text.written.export_campaign_no_deep_check)
                );
            }
        }
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
    let mut files = drawn
        .iter()
        .zip(&names)
        .filter_map(|(chart, name)| Some((name.as_str(), chart.svg.as_ref()?.as_bytes())))
        .collect::<Vec<_>>();
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

    fn export(judged: bool) -> Export {
        Export {
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
            cells: cells(),
            judged,
            charts: Vec::new(),
            recommendations: vec![RecommendationText {
                action: "Add jobs as a negative keyword.".into(),
                reason: "Job seekers are not patients.".into(),
                evidence: "jobs: $186 spend, 74 clicks.".into(),
            }],
            subject: Subject::AdAccount {
                update: Some(UpdateText {
                    date: "Monday 12 October".into(),
                    summary: "Spend was steady at $2,765.".into(),
                    cause: None,
                    lines: vec!["Search - Emergency: 210 clicks.".into()],
                }),
                campaigns: vec![CampaignRow {
                    name: "Search - Brand".into(),
                    spend: "$560".into(),
                    clicks: "280".into(),
                    impressions: "2,800".into(),
                    conversions: judged.then(|| "42".into()),
                }],
                deep_charts: DeepCharts {
                    date: None,
                    charts: Vec::new(),
                },
            },
        }
    }

    fn campaign_export() -> Export {
        Export {
            subject: Subject::Campaign {
                name: "Search - Emergency".into(),
                lines: vec![("Monday 12 October".into(), "Spend rose to $1,810.".into())],
                deep_check: Some(CampaignDeepCheck {
                    date: "12 October".into(),
                    sections: deep_sections(),
                }),
            },
            ..export(true)
        }
    }

    #[test]
    fn a_source_without_conversions_states_once_that_results_cannot_be_judged() {
        let source = source(&export(false), english()).unwrap();
        assert!(!source.contains("Conversions"));
        assert_eq!(source.matches("Results cannot be judged.").count(), 1);
        assert!(source.contains("head(\"Impressions\")"));
        assert!(source.contains("#section(\"Recommendations\")"));
        assert!(source.contains("\"Harbour \\\"Dental\\\"\""));
    }

    #[test]
    fn a_campaign_name_is_a_chip_in_every_text_and_a_heading_stays_plain() {
        let mut with = export(true);
        with.recommendations = vec![RecommendationText {
            action: "Pause Tom & <b>Jerry</b> today.".into(),
            reason: "Search - Brand has the same clicks.".into(),
            evidence: "Search - Emergency: $186 spend.".into(),
        }];
        with.charts = vec![ChartImage {
            title: "Spend and conversions by Campaign, last 7 days".into(),
            sentence: "Search - Emergency takes 30% of spend.".into(),
            svg: None,
            note: Some("Note on Search - Brand.".into()),
            changes: ChartChanges::of(vec![(
                "5 October".into(),
                "Budget of Search - Brand changed".into(),
            )]),
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
            r##"#let campaign(name) = highlight(fill: rgb("#EEE9F6"), extent: 2pt, radius: 2pt)[#text(fill: rgb("#3F1D85"), weight: "bold")[#name]]"##,
            r##"#text(size: 12pt)[#campaign("Search - Emergency")#(" and ")#campaign("Tom & <b>Jerry</b>")#(" moved.")]"##,
            r##"#list([#campaign("Search - Emergency")#(": 210 clicks.")])"##,
            r##"  [#campaign("Tom & <b>Jerry</b>")], "$560", "280", "2,800", "42","##,
            r##"#recommendation([#("Pause ")#campaign("Tom & <b>Jerry</b>")#(" today.")], [#campaign("Search - Brand")#(" has the same clicks.")], [#campaign("Search - Emergency")#(": $186 spend.")])"##,
            r##"#text(fill: muted)[#campaign("Search - Emergency")#(" takes 30% of spend.")]"##,
            r##"#text(size: 9pt, fill: muted)[#("Note on ")#campaign("Search - Brand")#(".")]"##,
            r##"#list([#("5 October: Budget of ")#campaign("Search - Brand")#(" changed.")])"##,
        ] {
            assert!(source.contains(expected), "{expected}\n{source}");
        }
        assert!(pdf(&with, english()).unwrap().starts_with(b"%PDF-"));

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
    fn a_source_with_conversions_has_a_conversion_column() {
        let source = source(&export(true), english()).unwrap();
        assert!(source.contains("head(\"Conversions\")"));
        assert!(!source.contains("Results cannot be judged."));
    }

    #[test]
    fn a_logo_is_drawn_only_when_the_ad_account_has_one() {
        let without = source(&export(true), english()).unwrap();
        assert!(!without.contains("image("));
        assert!(!without.contains("#grid(columns: (1fr, auto)"));
        assert!(pdf(&export(true), english()).unwrap().starts_with(b"%PDF-"));
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut with = export(true);
        with.logo = Some(png.into_inner());
        assert!(source(&with, english())
            .unwrap()
            .contains("image(\"logo.png\""));
        assert!(pdf(&with, english()).unwrap().starts_with(b"%PDF-"));
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
            "#section(\"Charts\")\n#block(width: 100%, breakable: false, below: 16pt)[#text(weight: \"bold\")[#(\"Spend and conversions by Campaign, last 7 days\")] \\\n#text(fill: muted)[#(\"Display - Remarketing takes 30% of spend and brings 9% of conversions.\")]\n#v(4pt)\n#image(\"chart-1.svg\", width: 100%)]"
        ), "{source}");
        assert_eq!(source.matches("#image(").count(), 1);
        assert!(source.contains("there is no cost per conversion to show.\")]\n#v(4pt)\n#text(size: 9pt, fill: muted)[#(\"About 23% of conversions arrive late.\")]]"), "{source}");
        assert!(pdf(&with, english()).unwrap().starts_with(b"%PDF-"));
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
                "#text(size: 12pt)[#(\"Spend was steady at $2,765.\")] \\\n#text(size: 9.5pt, fill: muted)[#(\"Confirmed cause: \")#campaign(\"Search - Brand\")#(\" accounts for 80% of the rise in Spend.\")]"
            ),
            "{written}"
        );
        assert!(pdf(&export, english()).unwrap().starts_with(b"%PDF-"));
    }

    #[test]
    fn a_campaign_source_names_the_campaign_and_the_ad_account_and_has_no_campaign_table() {
        let source = source(&campaign_export(), english()).unwrap();
        assert!(source.contains(
            "#text(size: 24pt, weight: \"bold\")[#(\"Search - Emergency\")] \\\n#text(fill: muted)[#(\"Harbour \\\"Dental\\\"\")] \\\n#text(fill: muted)[#(\"Last 7 days, 5 to 11 October 2026. Compared with 28 September to 4 October 2026. Prepared on 12 October 2026.\")]"
        ), "{source}");
        assert!(source.contains("#section(\"Updates about this Campaign\")"));
        assert!(source.contains("Spend rose to $1,810."));
        assert!(!source.contains("#section(\"Campaigns\")"));
        assert!(source.contains("#section(\"From the Deep check of 12 October\")"));
        assert!(pdf(&campaign_export(), english())
            .unwrap()
            .starts_with(b"%PDF-"));
    }

    #[test]
    fn a_campaign_source_prints_each_section_of_the_deep_check_with_its_bars_and_its_advice() {
        let source = source(&campaign_export(), english()).unwrap();
        assert!(source.contains(
            "#block(width: 100%, breakable: false, below: 16pt)[#text(weight: \"bold\")[#(\"Quality score of the keywords with the most spend\")] \\\n#text(fill: muted)[#(\"The keyword with the most spend has a Quality score of 5.\")] \\\n#text(fill: muted)[#(\"This is the first Deep check with Quality scores. The change of each score appears after the next Deep check.\")]\n#v(4pt)\n#table(columns: (1.3fr, 1fr, 1fr, 1fr, 1fr), align: left, stroke: (x: none, y: 0.6pt + rule), inset: (x: 6pt, y: 5pt), table.header(head(\"Keyword\"), head(\"Quality score\"), head(\"Expected clickthrough rate\"), head(\"Ad relevance\"), head(\"Landing page experience\")),\n  [#(\"emergency dentist\") \\ #text(size: 8.5pt, fill: muted)[#(\"Exact match\")]], [#score(5) \\ #text(weight: \"bold\")[#(\"5 of 10\")]], [#text(size: 8.5pt)[#(\"Below average\")]], [#text(size: 8.5pt)[#(\"Average\")]], [#text(size: 8.5pt)[#(\"Above average\")]],\n  [#(\"tooth pain\") \\ #text(size: 8.5pt, fill: muted)[#(\"Exact match\")]], table.cell(colspan: 4)[#text(size: 8.5pt, fill: muted)[#(\"Google Ads gives no Quality score yet. It needs more information about this keyword. You do not need to do anything.\")]],\n)"
        ), "{source}");
        assert!(source.contains(
            "[#(\"free dental care\")], [#bar(0.2400, none) #text(weight: \"bold\")[#(\"$96.00\")]], [#text(weight: \"bold\")[#(\"61\")]], [#text(weight: \"bold\")[#(\"0\")] \\ #text(size: 8.5pt)[#(\"Spend, no conversions\")]],"
        ), "{source}");
        assert!(source.contains(
            "[#bar(1.0000, 0.5000) #text(weight: \"bold\")[#(\"10.0%\")] \\ #text(size: 8.5pt, fill: muted)[#(\"30 conversions from 300 clicks\")] \\ #text(size: 8.5pt)[#(\"Above the Campaign rate\")]]"
        ), "{source}");
        assert!(source.contains("#(\"Campaign rate: 5.0%\")]\n#v(4pt)\n#text(size: 9pt, weight: \"bold\")[#(\"What you can do\")]"), "{source}");
        assert!(
            source.contains("[*#(\"Data limitation:\")* #(\"These are Google Ads numbers."),
            "{source}"
        );
        assert!(
            source.contains("#text(size: 8.5pt)[#(\"harbourdental.example/\u{200B}emergency\")]"),
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
        assert!(!source.contains("#section(\"Metrics\")"));
    }

    #[test]
    fn an_accent_colour_that_is_not_a_hex_colour_is_refused() {
        let mut bad = export(true);
        bad.accent = "red\")".into();
        assert!(source(&bad, english()).is_err());
    }
}

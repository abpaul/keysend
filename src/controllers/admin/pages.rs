use super::Admin;
use crate::language::english;
use crate::workspace::measurement::Operation;
use crate::{
    settings::Settings,
    workspace::operator::{self, CustomerQuery, OperatorStore},
};
use axum::{
    extract::{Path, Query},
    Extension,
};
use loco_rs::prelude::*;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct Metric {
    label: &'static str,
    value: String,
    definition: String,
}
#[derive(Serialize)]
struct Cell {
    text: String,
    href: Option<String>,
}
impl From<String> for Cell {
    fn from(text: String) -> Self {
        Self { text, href: None }
    }
}
impl From<&str> for Cell {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}
#[derive(Serialize)]
struct Section {
    title: &'static str,
    definition: &'static str,
    headers: Vec<&'static str>,
    rows: Vec<Vec<Cell>>,
    empty: &'static str,
    svg: Option<String>,
    table: bool,
    chart_summary: Option<String>,
    link: Option<Cell>,
}
#[derive(Serialize)]
struct Dashboard {
    title: &'static str,
    current: &'static str,
    recording: String,
    metrics: Vec<Metric>,
    sections: Vec<Section>,
    query: Option<CustomerQuery>,
    customers: Vec<CustomerRow>,
    next: bool,
    next_href: String,
    previous_href: String,
    previous: bool,
    page: i64,
}
#[derive(Serialize)]
struct CustomerRow {
    id: String,
    email: String,
    registered: String,
    source: String,
    status: String,
    ad_accounts: i64,
    last_activity: String,
    last_export: String,
    stage: String,
}
fn date(seconds: Option<i64>) -> String {
    seconds
        .and_then(chrono::DateTime::from_timestamp_secs)
        .map(|date| date.format("%d %b %Y").to_string())
        .unwrap_or_else(|| english().admin_dashboard_not_recorded.to_owned())
}
fn day(day: Option<i64>) -> String {
    date(day.map(|day| day * 86400))
}
fn money(cents: i64) -> String {
    format!("${:.2}", cents as f64 / 100.0)
}
fn ratio(n: i64, d: i64) -> String {
    if d > 0 {
        format!("{:.1}%", 100.0 * n as f64 / d as f64)
    } else {
        english().admin_dashboard_not_available.into()
    }
}
fn status(value: &str) -> &'static str {
    match value {
        "paying" => english().admin_dashboard_paying,
        "trial" => english().admin_dashboard_in_trial,
        "payment_due" => english().admin_dashboard_payment_due,
        "scheduled" => english().admin_dashboard_scheduled_cancellation,
        "canceled" => english().admin_dashboard_cancelled,
        "ended" => english().admin_dashboard_ended,
        _ => english().admin_dashboard_not_recorded,
    }
}
fn yes(value: bool) -> &'static str {
    if value {
        english().admin_dashboard_yes
    } else {
        english().admin_dashboard_no
    }
}
fn metric(label: &'static str, value: impl ToString, definition: &'static str) -> Metric {
    Metric {
        label,
        value: value.to_string(),
        definition: definition.into(),
    }
}
fn section(
    title: &'static str,
    definition: &'static str,
    headers: Vec<&'static str>,
    rows: Vec<Vec<Cell>>,
    empty: &'static str,
) -> Section {
    Section {
        title,
        definition,
        headers,
        rows,
        empty,
        svg: None,
        table: true,
        chart_summary: None,
        link: None,
    }
}
fn customer_row(c: operator::Customer) -> CustomerRow {
    CustomerRow {
        id: c.id,
        email: c.email,
        registered: date(c.created_at),
        source: c
            .source
            .unwrap_or_else(|| english().admin_dashboard_not_recorded.into()),
        status: status(&c.status).into(),
        ad_accounts: c.ad_accounts,
        last_activity: day(c.last_activity),
        last_export: day(c.last_export),
        stage: c.stage,
    }
}
fn customer_link(c: &operator::Customer) -> Cell {
    Cell {
        text: c.email.clone(),
        href: Some(format!("/admin/customers/{}", c.id)),
    }
}
async fn dashboard(
    store: &OperatorStore<'_>,
    title: &'static str,
    current: &'static str,
) -> Result<Dashboard> {
    Ok(Dashboard {
        title,
        current,
        recording: date(Some(store.recording_started().await?)),
        metrics: vec![],
        sections: vec![],
        query: None,
        customers: vec![],
        next: false,
        next_href: String::new(),
        previous_href: String::new(),
        previous: false,
        page: 1,
    })
}
fn render(v: &TeraView, page: Dashboard) -> Result<Response> {
    format::render().view(v, "admin/dashboard.html", data!({"dashboard":page}))
}
fn business_metrics(f: &operator::Figures) -> Vec<Metric> {
    vec![
        metric(
            english().admin_dashboard_mrr,
            money(f.mrr_cents),
            english().admin_dashboard_paying_workspaces_times_49_usd_per_month,
        ),
        metric(
            english().admin_dashboard_paying_workspaces,
            f.paying,
            english().admin_dashboard_subscription_active_with_a_paid_period_ending,
        ),
    ]
}
fn conversion(e: &operator::Engagement) -> Metric {
    metric(
        english().admin_dashboard_trial_to_paid,
        ratio(e.converted, e.trial_ended),
        english().admin_dashboard_of_retained_workspaces_whose_trial_ended_in,
    )
}
fn active(e: &operator::Engagement) -> Vec<Metric> {
    vec![
        metric(
            english().admin_dashboard_active_workspaces_7_days,
            e.active7,
            english().admin_dashboard_workspaces_with_viewed_exported_or_feedback_activity,
        ),
        metric(
            english().admin_dashboard_active_workspaces_30_days,
            e.active30,
            english().admin_dashboard_workspaces_with_viewed_exported_or_feedback_activity_in,
        ),
    ]
}
fn bars(values: &[i64]) -> String {
    let max = values.iter().copied().max().unwrap_or(0).max(1) as f64;
    let width = 600.0 / values.len().max(1) as f64;
    let mut svg=String::from("<svg viewBox=\"0 0 600 120\" aria-hidden=\"true\" focusable=\"false\" class=\"admin-chart\">");
    for (index, value) in values.iter().enumerate() {
        let h = (*value as f64 / max * 110.0).max(0.0);
        svg.push_str(&format!(
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{h:.2}\"/>",
            index as f64 * width,
            120.0 - h,
            (width - 1.0).max(0.5)
        ));
    }
    svg.push_str("</svg>");
    svg
}
async fn revenue_trend(store: &OperatorStore<'_>, settings: &Settings) -> Result<Section> {
    let points = store.business_trend(settings.now()).await?;
    let mut s = section(
        english().admin_dashboard_revenue_trend,
        english().admin_dashboard_one_recorded_utc_business_snapshot_per_day,
        vec![
            english().admin_dashboard_day,
            english().admin_dashboard_mrr,
            english().admin_dashboard_paying_workspaces,
        ],
        points
            .iter()
            .map(|p| {
                vec![
                    day(Some(p.day)).into(),
                    money(p.mrr_cents).into(),
                    p.paying.to_string().into(),
                ]
            })
            .collect(),
        english().admin_dashboard_trends_appear_after_the_first_daily_snapshot,
    );
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        s.chart_summary = Some(english().get(
            "admin_dashboard_revenue_chart_period",
            &[
                ("first", day(Some(first.day))),
                ("last", day(Some(last.day))),
                ("mrr", money(last.mrr_cents)),
            ],
        )?);
        s.svg = Some(bars(
            &points.iter().map(|p| p.mrr_cents).collect::<Vec<_>>(),
        ));
    }
    Ok(s)
}
fn operation_label(name: &str) -> &str {
    if let Some(op) = Operation::from_name(name) {
        return op.label();
    }
    let t = english();
    [
        ("MonitoringWorker", t.admin_worker_daily_run),
        ("EmailWorker", t.admin_worker_email),
        ("TranslationWorker", t.admin_worker_translation),
        ("RetentionWorker", t.admin_worker_retention),
    ]
    .into_iter()
    .find_map(|(worker, label)| (name == worker).then_some(label))
    .unwrap_or(t.admin_worker_unknown)
}
fn failures(rows: Vec<operator::Failure>) -> Section {
    section(
        english().admin_dashboard_latest_operational_failures,
        english().admin_dashboard_latest_retained_collection_google_and_deep_check,
        vec![
            english().admin_dashboard_date,
            english().admin_dashboard_workspace,
            english().admin_dashboard_ad_account,
            english().admin_dashboard_operation,
            english().admin_dashboard_error,
        ],
        rows.into_iter()
            .map(|f| {
                vec![
                    date(Some(f.occurred_at)).into(),
                    f.workspace_id.map_or_else(
                        || english().admin_dashboard_global.into(),
                        |id| Cell {
                            text: f
                                .email
                                .unwrap_or_else(|| english().admin_dashboard_not_recorded.into()),
                            href: Some(format!("/admin/customers/{id}")),
                        },
                    ),
                    f.customer_id.unwrap_or_default().into(),
                    operation_label(&f.source).into(),
                    f.error.into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_retained_operational_failures,
    )
}

pub(super) async fn overview(
    _admin: Admin,
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = OperatorStore::new(&ctx.db);
    let mut p = dashboard(&store, english().admin_dashboard_overview, "overview").await?;
    let f = store.figures(settings.now()).await?;
    let e = store.engagement(settings.now()).await?;
    p.metrics = business_metrics(&f);
    p.metrics.push(active(&e).remove(1));
    let mut registrations = metric(
        english().admin_dashboard_new_registrations_30_days,
        e.registrations,
        english().admin_dashboard_workspaces_with_a_known_registration_date_in,
    );
    registrations.definition.push(' ');
    registrations.definition.push_str(&english().get(
        "admin_dashboard_older_registration_dates",
        &[("count", e.unknown_registrations.to_string())],
    )?);
    p.metrics.push(registrations);
    p.metrics.push(conversion(&e));
    p.metrics.push(metric(
        english().admin_dashboard_connected_ad_accounts,
        f.ad_accounts,
        english().admin_dashboard_rows_in_ad_accounts_distinct_google_customer,
    ));
    let mut trend = revenue_trend(&store, &settings).await?;
    trend.table = false;
    trend.link = Some(Cell {
        text: english().admin_dashboard_all_revenue_figures.into(),
        href: Some("/admin/revenue".into()),
    });
    p.sections.push(trend);
    p.sections.push(section(
        english().admin_dashboard_registrations_and_activation_weeks,
        english().admin_dashboard_known_registrations_and_distinct_workspaces_recording_each,
        vec![
            english().admin_dashboard_iso_week,
            english().admin_dashboard_registered,
            english().admin_dashboard_connected,
            english().admin_dashboard_synced,
            english().admin_dashboard_assessed,
            english().admin_dashboard_exported,
        ],
        store
            .activation_trend(settings.now())
            .await?
            .into_iter()
            .map(|r| {
                vec![
                    chrono::DateTime::from_timestamp_secs(r.week * 86400)
                        .expect("recorded week")
                        .format("%G-W%V")
                        .to_string()
                        .into(),
                    r.registrations.to_string().into(),
                    r.connected.to_string().into(),
                    r.synced.to_string().into(),
                    r.assessed.to_string().into(),
                    r.exported.to_string().into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_registrations_or_activation_activity_recorded_in,
    ));
    p.sections.push(section(
        english().admin_dashboard_recent_registrations,
        english().admin_dashboard_the_5_most_recent_known_workspace_registration,
        vec![
            english().admin_dashboard_email,
            english().admin_dashboard_registered,
            english().admin_dashboard_status,
        ],
        store
            .recent(settings.now())
            .await?
            .iter()
            .map(|c| {
                vec![
                    customer_link(c),
                    date(c.created_at).into(),
                    status(&c.status).into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_known_registration_dates_recorded,
    ));
    p.sections.last_mut().expect("recent registrations").link = Some(Cell {
        text: english().admin_dashboard_all_customers.into(),
        href: Some("/admin/customers".into()),
    });
    p.sections.push(section(
        english().admin_dashboard_workspaces_needing_attention,
        english().admin_dashboard_payment_due_stale_ad_accounts_a_current,
        vec![
            english().admin_dashboard_email,
            english().admin_dashboard_why,
        ],
        store
            .attention(settings.now())
            .await?
            .iter()
            .map(|c| {
                let t = english();
                let why = if c.status == "payment_due" {
                    t.admin_dashboard_payment_due
                } else if c.status == "scheduled" {
                    t.admin_dashboard_scheduled_cancellation
                } else if c.failing > 0 {
                    t.admin_dashboard_deep_check_failure
                } else if c.stale > 0 {
                    t.admin_dashboard_stale_ad_account
                } else {
                    t.admin_dashboard_no_activity_30_days
                };
                vec![customer_link(c), why.into()]
            })
            .collect(),
        english().admin_dashboard_no_workspaces_need_attention_by_these_rules,
    ));
    let queue = store.queue().await?;
    p.sections
        .push(failures(store.failures(queue.is_some(), 5).await?));
    p.sections.last_mut().expect("latest failures").link = Some(Cell {
        text: english().admin_dashboard_all_operations.into(),
        href: Some("/admin/operations".into()),
    });
    render(&v, p)
}
pub(super) async fn customers(
    _admin: Admin,
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<CustomerQuery>,
) -> Result<Response> {
    let store = OperatorStore::new(&ctx.db);
    let mut p = dashboard(&store, english().admin_dashboard_customers, "customers").await?;
    let query = query.normalized();
    let mut rows = store.customers(settings.now(), &query).await?;
    let page_href = |page: i64| {
        let mut url = reqwest::Url::parse("http://localhost/admin/customers").expect("fixed URL");
        url.query_pairs_mut().extend_pairs([
            ("q", query.q.clone()),
            ("status", query.status.clone()),
            ("sort", query.sort.clone()),
            ("page", page.to_string()),
        ]);
        format!("/admin/customers?{}", url.query().unwrap_or_default())
    };
    p.next_href = page_href(query.number() + 1);
    p.previous_href = page_href(query.number() - 1);
    p.next = rows.len() > 50;
    rows.truncate(50);
    p.customers = rows.into_iter().map(customer_row).collect();
    p.page = query.number();
    p.previous = p.page > 1;
    p.query = Some(query);
    render(&v, p)
}
pub(super) async fn customer(
    _admin: Admin,
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<String>,
) -> Result<Response> {
    let store = OperatorStore::new(&ctx.db);
    let Some(c) = store.customer(settings.now(), &id).await? else {
        return Err(loco_rs::Error::NotFound);
    };
    let mut p = dashboard(&store, english().admin_dashboard_workspace, "customers").await?;
    p.sections.push(section(
        english().admin_dashboard_workspace,
        english().admin_dashboard_current_retained_user_and_workspace_no_campaign,
        vec![
            english().admin_dashboard_email,
            english().admin_dashboard_workspace_id,
            english().admin_dashboard_registered,
            english().admin_dashboard_source,
            english().admin_dashboard_status,
        ],
        vec![vec![
            c.email.into(),
            c.id.into(),
            date(c.created_at).into(),
            c.source
                .unwrap_or_else(|| english().admin_dashboard_not_recorded.into())
                .into(),
            status(&c.status).into(),
        ]],
        english().admin_dashboard_workspace_not_found,
    ));
    p.sections.push(section(
        english().admin_dashboard_subscription_now,
        english().admin_dashboard_current_state_fetched_from_stripe_after_a,
        vec![
            english().admin_dashboard_status,
            english().admin_dashboard_period_start,
            english().admin_dashboard_period_end,
            english().admin_dashboard_scheduled_cancellation,
            english().admin_dashboard_cancelled_on,
        ],
        store
            .subscription(&id)
            .await?
            .into_iter()
            .map(|s| {
                vec![
                    s.status.into(),
                    date(Some(s.period_start)).into(),
                    date(Some(s.period_end)).into(),
                    yes(s.cancel_at_period_end > 0).into(),
                    date(s.canceled_at).into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_subscription_recorded,
    ));
    p.sections.push(section(
        english().admin_dashboard_trial_dates,
        english().admin_dashboard_the_local_14_day_trial_a_remembered,
        vec![
            english().admin_dashboard_started,
            english().admin_dashboard_ends,
            english().admin_dashboard_started_trial,
        ],
        store
            .trial(&id)
            .await?
            .into_iter()
            .map(|t| {
                vec![
                    date(Some(t.started_at)).into(),
                    date(Some(t.ended_at)).into(),
                    yes(t.started > 0).into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_trial_recorded,
    ));
    p.sections.push(section(
        english().admin_dashboard_subscription_changes,
        english().admin_dashboard_observed_status_or_scheduled_cancellation_transitions_since,
        vec![
            english().admin_dashboard_observed,
            english().admin_dashboard_status,
            english().admin_dashboard_scheduled_cancellation,
        ],
        store
            .changes(&id)
            .await?
            .into_iter()
            .map(|c| {
                vec![
                    date(Some(c.observed_at)).into(),
                    c.status.into(),
                    yes(c.cancel_at_period_end > 0).into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_subscription_changes_recorded,
    ));
    p.sections.push(accounts(
        store.ad_accounts(settings.now(), Some(&id)).await?,
        false,
    ));
    p.sections.push(section(
        english().admin_dashboard_activity_30_days,
        english().admin_dashboard_recorded_counts_by_activity_kind_over_the,
        vec![
            english().admin_dashboard_kind,
            english().admin_dashboard_count,
            english().admin_dashboard_first_day,
            english().admin_dashboard_last_day,
        ],
        store
            .activity(settings.now(), &id, true)
            .await?
            .into_iter()
            .map(|a| {
                vec![
                    a.kind.into(),
                    a.count.to_string().into(),
                    day(Some(a.first_day)).into(),
                    day(Some(a.last_day)).into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_workspace_activity_recorded_in_this_period,
    ));
    p.sections.push(section(
        english().admin_dashboard_milestones,
        english().admin_dashboard_first_retained_recorded_day_per_kind_earlier,
        vec![
            english().admin_dashboard_kind,
            english().admin_dashboard_first_recorded_day,
        ],
        store
            .activity(settings.now(), &id, false)
            .await?
            .into_iter()
            .map(|a| vec![a.kind.into(), day(Some(a.first_day)).into()])
            .collect(),
        english().admin_dashboard_no_activation_or_user_activity_milestones_recorded,
    ));
    render(&v, p)
}
fn accounts(rows: Vec<operator::AdAccount>, stale: bool) -> Section {
    section(
        if stale {
            english().admin_dashboard_stale_ad_accounts
        } else {
            english().admin_dashboard_ad_accounts
        },
        english().admin_dashboard_last_success_is_missing_or_older_than,
        vec![
            english().admin_dashboard_ad_account,
            english().admin_dashboard_workspace,
            english().admin_dashboard_last_success,
            english().admin_dashboard_last_run,
            english().admin_dashboard_run_completed,
            english().admin_dashboard_deep_check,
            english().admin_dashboard_assessment,
            english().admin_dashboard_current_errors,
        ],
        rows.into_iter()
            .map(|a| {
                vec![
                    format!("{} ({})", a.name, a.customer_id).into(),
                    Cell {
                        text: a.email,
                        href: Some(format!("/admin/customers/{}", a.workspace_id)),
                    },
                    date(a.succeeded_at).into(),
                    day(a.last_run).into(),
                    a.completed
                        .map_or_else(|| english().admin_dashboard_not_recorded, |c| yes(c > 0))
                        .into(),
                    day(a.deep_day).into(),
                    yes(a.assessment > 0).into(),
                    [a.history_error, a.google_error, a.deep_error]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ")
                        .into(),
                ]
            })
            .collect(),
        if stale {
            english().admin_dashboard_no_stale_ad_accounts
        } else {
            english().admin_dashboard_no_ad_accounts_connected
        },
    )
}
pub(super) async fn revenue(
    _admin: Admin,
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = OperatorStore::new(&ctx.db);
    let mut p = dashboard(&store, english().admin_dashboard_revenue, "revenue").await?;
    let f = store.figures(settings.now()).await?;
    let e = store.engagement(settings.now()).await?;
    p.metrics = business_metrics(&f);
    p.metrics.extend([
        metric(
            english().admin_dashboard_arr,
            money(f.mrr_cents * 12),
            english().admin_dashboard_12_times_mrr_before_tax,
        ),
        metric(
            english().admin_dashboard_in_trial,
            f.trials,
            english().admin_dashboard_a_started_local_trial_that_has_not,
        ),
        metric(
            english().admin_dashboard_payment_due,
            f.payment_due,
            english().admin_dashboard_subscription_status_past_due_or_unpaid,
        ),
        metric(
            english().admin_dashboard_scheduled_cancellations,
            f.scheduled_cancellations,
            english().admin_dashboard_paying_workspaces_with_cancel_at_period_end,
        ),
        metric(
            english().admin_dashboard_completed_cancellations,
            f.canceled,
            english().admin_dashboard_current_subscription_status_canceled,
        ),
        metric(
            english().admin_dashboard_arpa,
            if f.paying > 0 {
                money(f.mrr_cents / f.paying)
            } else {
                english().admin_dashboard_not_available.into()
            },
            english().admin_dashboard_mrr_divided_by_paying_workspaces,
        ),
        conversion(&e),
    ]);
    p.sections.push(revenue_trend(&store, &settings).await?);
    p.sections.push(section(
        english().admin_dashboard_monthly_subscription_observations,
        english().admin_dashboard_first_observed_active_subscription_per_workspace_transitions,
        vec![
            english().admin_dashboard_month,
            english().admin_dashboard_first_observed_active,
            english().admin_dashboard_completed_cancellations,
            english().admin_dashboard_payment_failures,
            english().admin_dashboard_opening_paying,
            english().admin_dashboard_churn,
        ],
        store
            .months(settings.now())
            .await?
            .into_iter()
            .map(|m| {
                vec![
                    m.month.into(),
                    m.new_subscriptions.to_string().into(),
                    m.cancellations.to_string().into(),
                    m.payment_failures.to_string().into(),
                    m.opening_paying
                        .map_or_else(
                            || english().admin_dashboard_not_available.into(),
                            |n| n.to_string(),
                        )
                        .into(),
                    m.opening_paying
                        .filter(|_| m.cancellations > 0)
                        .map_or_else(
                            || english().admin_dashboard_not_available.into(),
                            |n| ratio(m.cancellations, n),
                        )
                        .into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_subscription_observations_or_daily_snapshots_recorded,
    ));
    render(&v, p)
}
pub(super) async fn usage(
    _admin: Admin,
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = OperatorStore::new(&ctx.db);
    let mut p = dashboard(&store, english().admin_dashboard_usage, "usage").await?;
    let f = store.figures(settings.now()).await?;
    let e = store.engagement(settings.now()).await?;
    p.metrics = active(&e);
    p.metrics.extend([
        metric(
            english().admin_dashboard_returning_workspaces,
            e.returning_workspaces,
            english().admin_dashboard_user_activity_in_at_least_two_distinct,
        ),
        metric(
            english().admin_dashboard_paying_without_activity_30_days,
            e.inactive_paying,
            english().admin_dashboard_paying_workspaces_without_viewed_exported_or_feedback,
        ),
        metric(
            english().admin_dashboard_registered_users,
            f.users,
            english().admin_dashboard_rows_in_users_now_deleted_users_are,
        ),
        metric(
            english().admin_dashboard_distinct_google_customer_ids,
            e.distinct_ad_accounts,
            english().admin_dashboard_distinct_customer_ids_across_all_connected_ad,
        ),
    ]);
    p.sections.push(section(
        english().admin_dashboard_activation_funnel,
        english().admin_dashboard_retained_registered_users_and_workspaces_with_connected,
        vec![
            english().admin_dashboard_stage,
            english().admin_dashboard_workspaces,
        ],
        [
            (english().admin_dashboard_registered, f.workspaces),
            (english().admin_dashboard_connected, e.connected),
            (english().admin_dashboard_synced, e.synced),
            (english().admin_dashboard_assessed, e.assessed),
            (english().admin_dashboard_exported, e.exported),
        ]
        .into_iter()
        .map(|(name, n)| vec![name.into(), n.to_string().into()])
        .collect(),
        english().admin_dashboard_no_workspaces_registered,
    ));
    p.sections.push(section(
        english().admin_dashboard_weekly_engagement,
        english().admin_dashboard_user_activity_counts_and_distinct_workspaces_per,
        vec![
            english().admin_dashboard_week_beginning,
            english().admin_dashboard_kind,
            english().admin_dashboard_count,
            english().admin_dashboard_workspaces,
        ],
        store
            .weeks(settings.now())
            .await?
            .into_iter()
            .map(|w| {
                vec![
                    day(Some(w.week)).into(),
                    w.kind.into(),
                    w.count.to_string().into(),
                    w.workspaces.to_string().into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_user_activity_recorded_in_this_period,
    ));
    render(&v, p)
}
pub(super) async fn operations(
    _admin: Admin,
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = OperatorStore::new(&ctx.db);
    let mut p = dashboard(&store, english().admin_dashboard_operations, "operations").await?;
    let queue = store.queue().await?;
    if let Some(q) = &queue {
        p.metrics.extend([
            metric(
                english().admin_dashboard_failed_background_jobs_7_days,
                q.failed,
                english().admin_dashboard_queue_rows_currently_failed_and_updated_in,
            ),
            metric(
                english().admin_dashboard_processing_jobs,
                q.processing,
                english().admin_dashboard_queue_rows_currently_processing,
            ),
            metric(
                english().admin_dashboard_oldest_due_queued_job,
                q.oldest_due
                    .clone()
                    .unwrap_or_else(|| english().admin_dashboard_none_due.into()),
                english().admin_dashboard_earliest_run_at_among_currently_queued_jobs,
            ),
        ]);
    } else {
        p.sections.push(section(
            english().admin_dashboard_background_queue,
            english().admin_dashboard_queue_inventory_is_available_only_when_the,
            vec![],
            vec![],
            english().admin_dashboard_background_queue_is_not_initialized,
        ));
    }
    p.sections.push(section(
        english().admin_dashboard_operations_14_days,
        english().admin_dashboard_recorded_attempts_by_operation_over_the_last,
        vec![
            english().admin_dashboard_operation,
            english().admin_dashboard_succeeded,
            english().admin_dashboard_failures,
            english().admin_dashboard_success_rate,
            english().admin_dashboard_average_duration_ms,
        ],
        store
            .operations(settings.now())
            .await?
            .into_iter()
            .map(|o| {
                let calls = o.succeeded + o.failed;
                vec![
                    operation_label(&o.operation).into(),
                    o.succeeded.to_string().into(),
                    o.failed.to_string().into(),
                    ratio(o.succeeded, calls).into(),
                    (if calls > 0 { o.elapsed_ms / calls } else { 0 })
                        .to_string()
                        .into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_operation_attempts_recorded_in_this_period,
    ));
    p.sections.push(section(
        english().admin_dashboard_ai_usage_14_days,
        english().admin_dashboard_daily_calls_failures_reported_tokens_and_average,
        vec![
            english().admin_dashboard_day,
            english().admin_dashboard_model,
            english().admin_dashboard_calls,
            english().admin_dashboard_failures,
            english().admin_dashboard_input_tokens,
            english().admin_dashboard_output_tokens,
            english().admin_dashboard_average_duration_ms,
            english().admin_dashboard_estimate,
        ],
        store
            .ai_days(settings.now())
            .await?
            .into_iter()
            .map(|a| {
                let cost = match (
                    settings.services.ai_prices.get(&a.model),
                    a.input_tokens,
                    a.output_tokens,
                ) {
                    (Some(price), Some(input), Some(output)) => format!(
                        "{} ${:.2}",
                        english().admin_dashboard_estimate,
                        crate::settings::estimate_ai_usd(input, output, *price)
                    ),
                    (None, _, _) => english().admin_dashboard_not_configured.into(),
                    _ => english().admin_dashboard_not_available.into(),
                };
                vec![
                    day(Some(a.day)).into(),
                    a.model.into(),
                    a.calls.to_string().into(),
                    a.failed.to_string().into(),
                    a.input_tokens
                        .map_or_else(
                            || english().admin_dashboard_not_recorded.into(),
                            |n| n.to_string(),
                        )
                        .into(),
                    a.output_tokens
                        .map_or_else(
                            || english().admin_dashboard_not_recorded.into(),
                            |n| n.to_string(),
                        )
                        .into(),
                    (if a.calls > 0 {
                        a.elapsed_ms / a.calls
                    } else {
                        0
                    })
                    .to_string()
                    .into(),
                    cost.into(),
                ]
            })
            .collect(),
        english().admin_dashboard_no_ai_calls_recorded_in_this_period,
    ));
    p.sections.push(accounts(
        store.ad_accounts(settings.now(), None).await?,
        true,
    ));
    p.sections
        .push(failures(store.failures(queue.is_some(), 30).await?));
    render(&v, p)
}

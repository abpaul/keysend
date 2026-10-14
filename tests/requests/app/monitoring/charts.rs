use super::*;
use keysend::workspace::profile::Profile;

const PAGE: &str = "/ad-accounts/4829153307";

fn count(page: &str, text: &str) -> usize {
    page.matches(text).count()
}

fn part<'a>(page: &'a str, from: &str, to: &str) -> &'a str {
    let start = page.find(&format!(r#"id="{from}""#)).unwrap();
    let end = page[start..].find(&format!(r#"id="{to}""#)).unwrap();
    &page[start..start + end]
}

async fn short_history(days: u64, spend: f64) -> Fixture {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    store
        .save_ad_account_metadata(AD, "USD", "UTC")
        .await
        .unwrap();
    let rows = (0..days)
        .filter(|_| spend > 0.0)
        .map(|offset| CampaignDay {
            day: end().checked_sub_days(Days::new(offset)).unwrap(),
            campaign_id: 701,
            device: 2,
            counts: Counts {
                impressions: 1000.0,
                clicks: 40.0,
                cost: spend,
                conversions: 2.0,
                value: 0.0,
            },
        })
        .collect::<Vec<_>>();
    store
        .save_history(
            AD,
            end().checked_sub_days(Days::new(days - 1)).unwrap(),
            end(),
            &[campaign()],
            &rows,
            &[],
        )
        .await
        .unwrap();
    store
        .save_monitoring_settings(
            AD,
            &AccountSettings {
                judge_by_conversions: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started=1 WHERE workspace_id=?",
            vec![signed.workspace.id().into()],
        ))
        .await
        .unwrap();
    f
}

#[tokio::test]
#[serial]
async fn the_ad_account_page_draws_its_five_charts_with_a_sentence_and_a_text_alternative() {
    let f = pages::harbour(true).await;
    let page = f.page(PAGE).await;

    let trend = part(&page, "trend", "campaign-shares");
    assert!(
        trend.starts_with(
            r#"id="trend">Spend per day, against its usual range</h2>
<p class="chart-say">Spend was unusual on 3 of the last 56 days, the last time on 19 Aug.</p>"#
        ),
        "{trend}"
    );
    assert!(trend.contains(
        r##"<a class="chart-skip" href="#trend-end">Skip the chart</a>
<figure class="chart chart--trend" aria-labelledby="trend">"##
    ));
    assert!(trend.contains(
        r#"</figure>
<span id="trend-end" tabindex="-1"></span>"#
    ));
    assert_eq!(count(&page, r#"<a class="chart-skip""#), 3);
    assert_eq!(count(trend, r#"<text class="tick--second" x=""#), 2);
    assert!(trend.contains(r#"<li><i class="key key--outside"></i>Unusual day</li>"#));
    assert_eq!(
        count(trend, r#"<g class="pt" tabindex="0" role="img">"#),
        56
    );
    assert_eq!(count(trend, "pt__dot--out"), 3);
    assert!(trend.contains("<title>Sun 11 Oct: $593. Usual: $540 to $581.</title>"));
    assert_eq!(
        trend
            .split(r#"<text class="chart__change"#)
            .skip(1)
            .filter_map(|rest| rest.split("</text>").next())
            .collect::<Vec<_>>(),
        [
            r#" wide" x="70.91%" dx="-6" y="10" text-anchor="end">3 Account changes · 25 Sep to 8 Oct"#,
            r#" narrow" x="70.91%" dx="-6" y="10" text-anchor="end">25 Sep to 8 Oct"#,
        ]
    );
    assert_eq!(count(trend, r#"<line class="change""#), 3);
    assert!(trend.contains("<caption>Spend per day, 17 Aug to 11 Oct</caption>"));
    assert!(trend.contains(r#"<tr><th scope="col">Day</th><th scope="col">Spend</th><th scope="col">Usual range</th><th scope="col">Account change</th></tr>"#));
    assert!(trend.contains(
        r#"<tr><th scope="row">Sun 11 Oct</th><td>$593</td><td>$540 to $581</td><td></td></tr>"#
    ));
    assert!(trend.contains(r#"<tr><th scope="row">Fri 25 Sep</th><td>$595</td><td>$174 to $649</td><td>Search - Emergency: Daily budget changed from $300.00 to $340.00</td></tr>"#));
    assert!(trend.contains("<td>$547, unusual day</td>"));
    assert!(trend.contains(r#"<li><i class="key key--band"></i>Usual range for the weekday</li>"#));

    let weeks = part(&page, "cost-weeks", "cost-weeks-end");
    assert!(weeks.starts_with(
        r#"id="cost-weeks">Cost per conversion, 12 weeks</h2>
<p class="chart-say">Cost per conversion was $46.48 in the week of 28 Sep. The last week is not complete: conversions can still arrive.</p>"#
    ), "{weeks}");
    assert!(!weeks.contains("Target"));
    assert_eq!(
        count(weeks, r#"<g class="pt" tabindex="0" role="img">"#),
        12
    );
    assert!(weeks
        .contains("<title>Week of 5 Oct (so far): $39.62. $4,120 spend, 104 conversions.</title>"));
    assert!(weeks.contains(
        r#"<tr><th scope="row">Week of 28 Sep</th><td>$46.48</td><td>$4,090</td><td>88</td></tr>"#
    ));
    assert!(weeks.contains(r#"<tr><th scope="row">Week of 20 Jul</th><td>Not available</td><td>$0.00</td><td>0</td></tr>"#));

    let shares = part(&page, "campaign-shares", "campaign-shares-end");
    assert!(shares.starts_with(
        r#"id="campaign-shares">Spend and conversions by Campaign, last 7 days</h2>
<p class="chart-say"><b class="campaign-name">Search - Implants</b> takes <mark class="chart-say__point">23% of spend and brings 12% of conversions.</mark></p>"#
    ), "{shares}");
    assert!(shares.contains(r#"<div class="chart__row"><a class="campaign-name" href="/ad-accounts/4829153307/campaigns/3">Search - Implants</a><svg class="chart__bars" height="29" role="group" aria-label="Search - Implants"><g class="pt" tabindex="0" role="img"><title>Search - Implants: 23% of spend, $936 of $4,120</title>"#));
    assert!(shares.contains(r#"<rect class="bar bar--spend" x="0" y="0" width="39.85%" height="13" rx="4"/><text x="39.85%" dx="6" y="11">23%</text>"#));
    assert!(shares.contains(r#"<rect class="bar bar--results" x="0" y="16" width="20.24%" height="13" rx="4"/><text x="20.24%" dx="6" y="27">12%</text>"#));
    assert!(shares.contains(r#"<tr><th scope="row">Search - Emergency</th><td>$2,349</td><td>57%</td><td>58</td><td>56%</td></tr>"#));
    assert!(shares.contains(r#"<li><i class="key key--results"></i>Share of conversions</li>"#));

    assert!(page.contains(r#"<th scope="col" class="spark-cell"><span aria-hidden="true">30 days</span><span class="visually-hidden">30 days of spend per day</span></th>"#));
    assert_eq!(count(&page, r#"<svg class="spark""#), 4);
    assert!(page.contains(r#"<td class="spark-cell"><svg class="spark" viewBox="0 0 100 32" width="100" height="28" role="img" aria-label="Spend per day from 12 Sep to 11 Oct: $296 on the first day, $340 on the last day, $341 at the highest."><title>Spend per day from 12 Sep to 11 Oct: $296 on the first day, $340 on the last day, $341 at the highest.</title><path d="M0.0,7.4L3.4,8.0L"#));

    let devices = part(&page, "device-shares", "campaigns");
    assert!(
        devices.starts_with(
            r#"id="device-shares">Spend and conversions by device, last 7 days</h2>
<p class="chart-say">Only one device had spend in these days: Mobile.</p>"#
        ),
        "{devices}"
    );
    assert!(!devices.contains("<figure") && !devices.contains("chart__row"));

    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert!(store
        .save_profile(
            AD,
            &Profile {
                goal: String::new(),
                target_cost: Some(45.0),
                note: String::new(),
            },
        )
        .await
        .unwrap());
    let page = f.page(PAGE).await;
    let weeks = part(&page, "cost-weeks", "cost-weeks-end");
    assert!(weeks.contains("Cost per conversion was $46.48 in the week of 28 Sep, above the target from the Ad Account profile ($45.00). The last week is not complete: conversions can still arrive."));
    assert!(weeks.contains(r#"text-anchor="end">Target $45</text>"#));
    assert!(
        weeks.contains("<caption>Cost per conversion per week. The target is $45.00.</caption>")
    );
    assert!(weeks.contains(r#"<li><i class="key key--above"></i>Above the target</li>"#));
}

#[tokio::test]
#[serial]
async fn each_metric_number_links_to_its_trend_chart_and_the_query_chooses_the_metric() {
    let f = pages::harbour(true).await;
    let page = f.page(PAGE).await;
    for link in [
        r#"<dd class="metric__link"><a href="/ad-accounts/4829153307?metric=spend#trend" data-swap aria-current="true">Show spend per day</a></dd>"#,
        r#"<dd class="metric__link"><a href="/ad-accounts/4829153307?metric=conversions#trend" data-swap>Show conversions per day</a></dd>"#,
        r#"<dd class="metric__link"><a href="/ad-accounts/4829153307?metric=cost-per-conversion#trend" data-swap>Show cost per conversion per day</a></dd>"#,
        r#"<dd class="metric__link"><a href="/ad-accounts/4829153307?metric=clicks#trend" data-swap>Show clicks per day</a></dd>"#,
    ] {
        assert!(page.contains(link), "{link}");
    }

    let cost = f
        .page(&format!("{PAGE}?window=30&metric=cost-per-conversion"))
        .await;
    assert!(cost.contains(
        r#"id="trend">Cost per conversion per day, against its usual range</h2>
<p class="chart-say">Cost per conversion had no unusual day in the last 32 days.</p>"#
    ));
    assert!(cost.contains(r#"<a href="/ad-accounts/4829153307?window=30&amp;metric=cost-per-conversion#trend" data-swap aria-current="true">Show cost per conversion per day</a>"#));
    assert!(cost.contains(r#"<a href="/ad-accounts/4829153307?window=30&amp;metric=spend#trend" data-swap>Show spend per day</a>"#));
    assert!(cost.contains(r#"<a href="/ad-accounts/4829153307?metric=cost-per-conversion" data-swap aria-label="Show the last 7 days">7 days</a>"#));
    assert!(cost.contains("<caption>Cost per conversion per day, 17 Aug to 11 Oct</caption>"));
    assert!(cost.contains(r#"<tr><th scope="row">Sun 11 Oct</th><td>$42.36</td><td>$48.09 to $52.85</td><td></td></tr>"#), "{cost}");

    let campaign = f
        .page(&format!("{PAGE}/campaigns/4?metric=conversions"))
        .await;
    assert!(campaign.contains(
        r#"id="trend">Conversions per day, against its usual range</h2>
<p class="chart-say">Conversions had no unusual day in the last 56 days.</p>"#
    ));
    assert!(campaign.contains(r#"<a href="/ad-accounts/4829153307/campaigns/4?metric=conversions#trend" data-swap aria-current="true">Show conversions per day</a>"#));
    assert!(campaign.contains(r#"<a href="/ad-accounts/4829153307/campaigns/4?window=30&amp;metric=conversions" data-swap aria-label="Show the last 30 days">30 days</a>"#));
    assert!(campaign.contains(
        r#"<text class="chart__change wide" x="94.55%" dx="-6" y="10" text-anchor="end">Daily budget $45 → $20 · 8 Oct</text>"#
    ));
    assert!(campaign.contains("<li>8 October: Daily budget changed from $45.00 to $20.00."));
    assert!(campaign.contains("KeySend checks the result on 24 October."));

    let unknown = f.page(&format!("{PAGE}?metric=impressions")).await;
    assert!(unknown.contains(r#"id="trend">Spend per day, against its usual range</h2>"#));
}

#[tokio::test]
#[serial]
async fn the_campaign_page_draws_the_trend_of_the_campaign_and_its_device_split() {
    let f = pages::harbour(true).await;
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO campaign_days(workspace_id,ad_account_customer_id,day,campaign_id,device,impressions,clicks,cost_micros,conversions,conversions_value) SELECT workspace_id,ad_account_customer_id,day,campaign_id,2,0,0,783000000,2,0 FROM campaign_days WHERE campaign_id=2 AND day=?",
            vec![day_number(end()).into()],
        ))
        .await
        .unwrap();
    let page = f.page(&format!("{PAGE}/campaigns/2")).await;
    let trend = part(&page, "trend", "updates");
    assert!(
        trend.starts_with(
            r#"id="trend">Spend per day, against its usual range</h2>
<p class="chart-say">Spend was unusual on 1 of the 7 days after the Account change of 4 Oct.</p>"#
        ),
        "{trend}"
    );
    assert_eq!(
        count(trend, r#"<g class="pt" tabindex="0" role="img">"#),
        56
    );
    assert!(trend.contains("<title>Sun 11 Oct: $1,123. Usual: $283 to $324. Unusual day.</title>"));
    assert_eq!(
        trend
            .split(r#"<text class="chart__change"#)
            .skip(1)
            .filter_map(|rest| rest.split("</text>").next())
            .collect::<Vec<_>>(),
        [
            r#" wide" x="87.27%" y="10" text-anchor="middle">4 Oct"#,
            r#" wide" x="70.91%" y="10" text-anchor="middle">25 Sep"#,
            r#" narrow" x="70.91%" dx="-6" y="10" text-anchor="end">25 Sep to 4 Oct"#,
        ]
    );
    assert!(trend.contains(r#"<h3 class="heading-s">Account changes in these 8 weeks</h3>"#));
    assert!(!trend.contains("to $20.00"));
    assert!(page.contains(r#"id="cost-weeks">Cost per conversion, 12 weeks</h2>"#));
    assert!(!page.contains(r#"id="campaign-shares""#));

    let devices = part(&page, "device-shares", "network-shares");
    assert!(devices.starts_with(
        r#"id="device-shares">Spend and conversions by device, last 7 days</h2>
<p class="chart-say"><b>Desktop</b> takes <mark class="chart-say__point">25% of spend and brings 3% of conversions.</mark></p>"#
    ), "{devices}");
    assert!(devices.contains(
        r#"<tr><th scope="row">Mobile</th><td>$2,349</td><td>75%</td><td>58</td><td>97%</td></tr>"#
    ));
    assert!(devices.contains(
        r#"<tr><th scope="row">Desktop</th><td>$783</td><td>25%</td><td>2</td><td>3%</td></tr>"#
    ));
}

#[tokio::test]
#[serial]
async fn an_ad_account_that_is_not_judged_by_conversions_has_no_conversion_word_in_a_chart() {
    let f = pages::harbour(false).await;
    let page = f.page(&format!("{PAGE}?metric=conversions")).await;
    assert_eq!(count(&page, r#"<dd class="metric__link">"#), 4);
    assert!(page.contains(
        r#"<a href="/ad-accounts/4829153307?metric=cost-per-click#trend" data-swap>Show cost per click per day</a>"#
    ));
    let impressions = f.page(&format!("{PAGE}?metric=impressions")).await;
    assert!(impressions.contains(r#"id="trend">Impressions per day, against its usual range</h2>"#));
    assert!(page.contains(
        r#"<a href="/ad-accounts/4829153307?metric=clicks#trend" data-swap>Show clicks per day</a>"#
    ));
    assert!(page.contains(r#"id="trend">Spend per day, against its usual range</h2>"#));
    assert!(!page.contains(r#"id="cost-weeks""#));
    let charts = part(&page, "trend", "campaigns");
    assert!(!charts.to_lowercase().contains("conversion"), "{charts}");
    assert!(charts.contains(
        r#"id="campaign-shares">Spend and clicks by Campaign, last 7 days</h2>
<p class="chart-say"><b class="campaign-name">Search - Emergency</b> takes <mark class="chart-say__point">57% of spend and brings 44% of clicks.</mark></p>"#
    ));
    assert!(charts.contains(r#"<tr><th scope="row">Search - Emergency</th><td>$2,349</td><td>57%</td><td>1,010</td><td>44%</td></tr>"#));
    assert!(charts.contains(r#"<li><i class="key key--results"></i>Share of clicks</li>"#));
    assert!(charts.contains(
        r#"id="device-shares">Spend and clicks by device, last 7 days</h2>
<p class="chart-say">Only one device had spend in these days: Mobile.</p>"#
    ));

    let campaign = f.page(&format!("{PAGE}/campaigns/2?metric=clicks")).await;
    let charts = part(&campaign, "trend", "deep-check");
    assert!(!charts.to_lowercase().contains("conversion"), "{charts}");
    assert!(charts.starts_with(
        r#"id="trend">Clicks per day, against its usual range</h2>
<p class="chart-say">Clicks were unusual on 3 of the last 56 days, the last time on 19 Aug.</p>"#
    ));
    assert!(charts.contains(
        r#"<text class="chart__change wide" x="70.91%" dx="-6" y="10" text-anchor="end">Daily budget $300 → $340 · 25 Sep</text>"#
    ));
    assert!(charts.contains("<caption>Clicks per day, 17 Aug to 11 Oct</caption>"));
    assert!(campaign.contains(r#"<a href="/ad-accounts/4829153307/campaigns/2?metric=clicks#trend" data-swap aria-current="true">Show clicks per day</a>"#));
}

#[tokio::test]
#[serial]
async fn thin_account_history_draws_no_usual_range_and_says_so() {
    let f = short_history(21, 30.0).await;
    let page = f.page(PAGE).await;
    let trend = part(&page, "trend", "campaign-shares");
    assert!(trend.starts_with(
        r#"id="trend">Spend per day</h2>
<p class="chart-say">Spend per day for 21 days. The chart has no usual range, because the Account history has fewer than 8 weeks.</p>"#
    ), "{trend}");
    assert_eq!(
        count(trend, r#"<g class="pt" tabindex="0" role="img">"#),
        21
    );
    assert!(!trend.contains(r#"class="band""#));
    assert!(!trend.contains("Usual range for the weekday"));
    assert!(trend.contains(
        r#"<tr><th scope="row">Mon 21 Sep</th><td>$30.00</td><td>Not available</td><td></td></tr>"#
    ));
    let weeks = part(&page, "cost-weeks", "cost-weeks-end");
    assert!(weeks.starts_with(
        r#"id="cost-weeks">Cost per conversion, 3 weeks</h2>
<p class="chart-say">Cost per conversion was $15.00 in the week of 28 Sep. The last week is not complete: conversions can still arrive.</p>"#
    ), "{weeks}");
    assert_eq!(count(weeks, r#"<rect class="bar "#), 3);
    assert!(page.contains(r#"aria-label="Spend per day from 21 Sep to 11 Oct: $30.00 on the first day, $30.00 on the last day, $30.00 at the highest."><title>Spend per day from 21 Sep to 11 Oct: $30.00 on the first day, $30.00 on the last day, $30.00 at the highest.</title><path d="M0.0,4.0L5.0,4.0L"#));
    assert!(page.contains(
        r#"<p class="chart-say">Only one Campaign had spend in these days: Search - Real.</p>"#
    ));
    assert!(page
        .contains(r#"<p class="chart-say">Only one device had spend in these days: Desktop.</p>"#));
}

#[tokio::test]
#[serial]
async fn a_chart_with_nothing_to_show_is_one_sentence() {
    let f = short_history(21, 0.0).await;
    let page = f.page(&format!("{PAGE}?metric=cost-per-conversion")).await;
    for sentence in [
        r#"id="trend">Cost per conversion per day</h2>
<p class="chart-say">No day of the last 21 had a cost per conversion.</p>"#,
        r#"id="cost-weeks">Cost per conversion, 12 weeks</h2>
<p class="chart-say">No week of the last 12 had conversions, so there is no cost per conversion to show.</p>"#,
        r#"id="campaign-shares">Spend and conversions by Campaign, last 7 days</h2>
<p class="chart-say">No Campaign had spend in these days.</p>"#,
        r#"id="device-shares">Spend and conversions by device, last 7 days</h2>
<p class="chart-say">No device had spend in these days.</p>"#,
    ] {
        assert!(page.contains(sentence), "{sentence}\n{page}");
    }
    assert!(!page.contains("<figure"));
    assert!(!page.contains("<svg class=\"chart"));
}

#[tokio::test]
#[serial]
async fn each_chart_and_the_metrics_explain_what_they_show_and_what_the_user_can_do() {
    let f = pages::harbour(true).await;
    let page = f.page(PAGE).await;
    assert!(page.contains("<summary>What this shows and what you can do</summary>"));
    assert!(page.contains("<p>Cost per conversion is the spend divided by the conversions. A lower cost per conversion means each result cost less.</p>"));
    assert!(
        page.contains("<li>Look at an unusual day first, and read the Update of that day.</li>")
    );
    assert!(page.contains(r#"<p class="chart-note"><b>Data limitation:</b> Google Ads can correct the numbers of the newest days.</p>"#));
}

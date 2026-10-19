use loco_rs::Result;

use super::{format::Format, Plural, Text};

macro_rules! say {
    ($text:expr, $field:ident $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::language::fill(
            stringify!($field),
            $text.written.$field,
            &[$((stringify!($name), ($value).to_string())),*] as &[(&str, String)],
        )
    };
}

macro_rules! say_plural {
    ($text:expr, $field:ident, $count:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $text.plural(
            &$text.written.$field,
            $count,
            &[$((stringify!($name), ($value).to_string().as_str())),*],
        )
    };
}

macro_rules! say_amount {
    ($text:expr, $field:ident, $value:expr $(, $name:ident = $item:expr)* $(,)?) => {
        $text.plural_amount(
            &$text.written.$field,
            $value,
            &[$((stringify!($name), ($item).to_string().as_str())),*],
        )
    };
}

pub(crate) use say;
pub(crate) use say_amount;
pub(crate) use say_plural;

macro_rules! written {
    (lines: [$($line:ident),* $(,)?], plurals: [$($plural:ident),* $(,)?] $(,)?) => {
        pub struct Written {
            pub format: Format,
            $(pub $line: &'static str,)*
            $(pub $plural: Plural,)*
        }

        impl Written {
            pub const KEYS: &'static [&'static str] =
                &[$(stringify!($line),)* $(stringify!($plural),)*];

            #[cfg(feature = "marker-language")]
            pub(super) fn marked(english: &Written, format: Format) -> Written {
                Written {
                    format,
                    $($line: super::marker::line(stringify!($line), english.$line),)*
                    $($plural: super::marker::plural(stringify!($plural), &english.$plural),)*
                }
            }

            pub fn templates(&self, key: &str) -> Option<Vec<&'static str>> {
                match key {
                    $(stringify!($line) => Some(vec![self.$line]),)*
                    $(stringify!($plural) => {
                        Some(vec![self.$plural.one, self.$plural.few, self.$plural.other])
                    })*
                    _ => None,
                }
            }
        }
    };
}

written! {
    lines: [
        page_economics_share_invalid,
        page_economics_not_number,
        page_customer_value_invalid,
        page_target_return_invalid,
        assessment_title,
        assessment_result,
        assessment_improved,
        assessment_steady,
        assessment_mixed,
        assessment_worse,
        assessment_seasonal,
        assessment_unclear,
        assessment_period,
        assessment_worked,
        assessment_attention,
        assessment_next,
        assessment_market,
        assessment_explanation,
        recommendation_measure,
        metric_spend,
        metric_clicks,
        metric_conversions,
        metric_cost_per_conversion,
        metric_cost_per_click,
        metric_impressions,
        missing_spend,
        missing_conversions,
        missing_clicks,
        missing_impressions,
        not_available,
        was_not_available,
        no_percentage_comparison,
        change_none,
        change_up,
        change_down,
        change_up_from,
        change_down_from,
        from_value,
        was_value,
        notice_not_judged_title,
        notice_not_judged,
        notice_not_set_up_title,
        notice_not_set_up,
        notice_too_few_title,
        notice_too_few,
        spark_spend,
        spark_metric,
        explain_not_results,
        explain_look_conversions,
        explain_look_clicks,
        explain_metrics_spend,
        explain_metrics_conversion,
        explain_metrics_cost_per_conversion,
        explain_metrics_click,
        explain_metrics_cost_per_click,
        explain_metrics_impression,
        explain_metrics_read_change,
        explain_metrics_choose_number,
        explain_metrics_limit_judged,
        explain_trend_line,
        explain_trend_band,
        explain_trend_points,
        explain_trend_unusual,
        explain_trend_change,
        explain_trend_limit,
        explain_search_estimate,
        explain_search_shown,
        explain_search_budget,
        explain_search_rank,
        explain_search_budget_action,
        explain_search_remove_action,
        explain_search_rank_action,
        explain_search_limit,
        explain_campaign_about,
        explain_campaign_bars_conversions,
        explain_campaign_bars_clicks,
        explain_campaign_action,
        explain_campaign_limit,
        explain_device_about,
        explain_device_bars_conversions,
        explain_device_bars_clicks,
        explain_device_action,
        explain_device_limit,
        explain_network_about,
        explain_network_bars_conversions,
        explain_network_bars_clicks,
        explain_network_action,
        explain_network_limit,
        explain_locations_about,
        explain_locations_bars_conversions,
        explain_locations_bars_clicks,
        explain_locations_action,
        explain_locations_limit,
        explain_ages_about,
        explain_ages_bars_conversions,
        explain_ages_bars_clicks,
        explain_ages_action,
        explain_ages_limit,
        explain_genders_about,
        explain_genders_bars_conversions,
        explain_genders_bars_clicks,
        explain_genders_action,
        explain_genders_limit,
        explain_hours_about_conversions,
        explain_hours_about_clicks,
        explain_hours_zone,
        explain_hours_answer,
        explain_hours_schedule,
        explain_hours_limit,
        explain_cost_weeks_bar,
        explain_cost_weeks_lower,
        explain_cost_weeks_compare,
        explain_cost_weeks_table,
        explain_cost_weeks_limit,
        explain_action_weeks_what,
        explain_action_weeks_colour,
        explain_action_weeks_value,
        explain_action_weeks_tag,
        explain_action_weeks_limit,
        results_unjudged,
        export_footer,
        export_sample_label,
        export_sample_footer,
        export_update,
        export_no_update,
        cause_confirmed,
        cause_likely,
        cause_contribution,
        export_campaign_updates,
        export_no_campaign_updates,
        export_metrics,
        export_charts,
        export_account_changes,
        export_change,
        export_no_account_changes,
        export_deep_check,
        export_deep_check_note,
        export_no_deep_check,
        export_no_deep_check_charts,
        export_campaign_deep_check_note,
        export_campaign_deep_check_empty,
        export_campaign_no_deep_check,
        export_what_to_do,
        export_data_limitation,
        export_campaigns,
        export_campaign,
        export_no_campaigns,
        export_recommendations,
        export_no_recommendations,
        export_no_campaign_recommendations,
        export_not_judged,
        export_refuse_window,
        export_refuse_untranslated,
        export_refuse_trial_ended,
        export_refuse_unread,
        email_subject_weekly,
        email_list_and,
        email_weekly_title,
        email_weekly_intro,
        email_block_range,
        email_metrics_heading,
        email_metric_line,
        email_share_row_conversions,
        email_share_row_clicks,
        email_earlier_updates,
        email_recommendation_heading,
        email_finding,
        email_open,
        email_footer,
        email_weekly_note,
        alert_subject,
        alert_stopped_headline,
        alert_stopped_lead,
        alert_stopped_action,
        alert_spend_headline,
        alert_spend_lead,
        alert_spend_action,
        alert_conversions_headline,
        alert_conversions_action,
        alert_title,
        alert_fact,
        alert_facts,
        alert_what_to_do,
        alert_open,
        alert_note,
        results_conversions,
        results_clicks,
        metric_spend_lower,
        metric_clicks_lower,
        metric_conversions_lower,
        metric_cost_per_conversion_lower,
        metric_cost_per_click_lower,
        metric_impressions_lower,
        metric_per_day_spend,
        metric_per_day_clicks,
        metric_per_day_conversions,
        metric_per_day_cost_per_conversion,
        metric_per_day_cost_per_click,
        metric_per_day_impressions,
        share_of_spend,
        share_of_results,
        shares_title,
        shares_title_bare,
        shares_takes,
        shares_takes_both,
        shares_takes_spend,
        shares_bar_spend_title,
        shares_bar_spend_tip,
        shares_bar_results_title,
        shares_bar_results_tip,
        shares_header_spend,
        shares_campaign_noun,
        shares_campaign_no_spend,
        shares_campaign_only_one,
        shares_campaign_no_results,
        shares_device_noun,
        shares_device_no_spend,
        shares_device_only_one,
        shares_device_no_results,
        shares_network_noun,
        shares_network_no_spend,
        shares_network_only_one,
        shares_network_no_results,
        shares_location_noun,
        shares_location_no_spend,
        shares_location_only_one,
        shares_location_no_results,
        shares_age_noun,
        shares_age_no_spend,
        shares_age_only_one,
        shares_age_no_results,
        shares_gender_noun,
        shares_gender_no_spend,
        shares_gender_only_one,
        shares_gender_no_results,
        trend_after_day_was,
        trend_after_day_were,
        trend_title_range,
        trend_title,
        trend_empty_short,
        trend_tip_day,
        trend_tip_usual,
        trend_unusual_day,
        trend_tip_below,
        trend_tip_above,
        trend_tip_unusual,
        trend_tip_change,
        trend_tip_changes,
        trend_value_none,
        trend_value_unusual,
        trend_range,
        trend_key_range,
        trend_key_outside,
        trend_key_change,
        trend_caption,
        trend_header_day,
        trend_header_usual,
        trend_header_change,
        trend_label_one,
        trend_label_more,
        trend_label_group,
        trend_dates,
        weeks_target,
        weeks_totals,
        weeks_line_cost,
        weeks_line_cost_open,
        weeks_line_none,
        weeks_line_none_open,
        weeks_row,
        weeks_row_open,
        weeks_axis_open,
        weeks_open,
        weeks_sentence,
        weeks_sentence_above_campaign,
        weeks_sentence_below_campaign,
        weeks_sentence_at_campaign,
        weeks_sentence_above_account,
        weeks_sentence_below_account,
        weeks_sentence_at_account,
        weeks_sentence_none,
        weeks_key_within,
        weeks_key_above,
        weeks_key_open,
        weeks_caption_target,
        weeks_caption,
        weeks_header_week,
        shares_other_locations,
        page_google_connections_not_configured,
        page_google_permission_refused,
        page_google_failed,
        page_choose_one_ad_account,
        page_ad_accounts_would_have,
        page_trial_ended_add,
        page_signin_not_configured,
        page_signin_state_invalid,
        page_consent_incomplete,
        page_connection_other_signin,
        page_subscription_limit,
        page_payments_unavailable,
        page_stripe_unavailable,
        page_payment_details_not_removed,
        page_google_permission_not_removed,
        page_target_not_number_or_empty,
        page_target_above_zero,
        page_note_length,
        page_target_not_number,
        page_profile_not_saved,
        page_branding_name,
        page_logo_rule,
        missing_locations_error,
        missing_locations_unread,
        missing_locations_none,
        missing_hours_error,
        missing_hours_unread,
        missing_hours_none,
        missing_quality_error,
        missing_quality_unread,
        missing_quality_none,
        missing_search_share_error,
        missing_search_share_unread,
        missing_search_share_none,
        missing_actions_error,
        missing_actions_unread,
        missing_actions_none,
        missing_networks_error,
        missing_networks_unread,
        missing_networks_none,
        missing_age_error,
        missing_age_unread,
        missing_gender_error,
        missing_gender_unread,
        network_search,
        network_search_partners,
        network_content,
        network_youtube,
        network_google_tv,
        network_mixed,
        network_discover,
        network_gmail,
        network_maps,
        network_owned,
        network_other,
        device_mobile,
        device_desktop,
        device_tablet,
        device_tv,
        device_other,
        demographic_18_24,
        demographic_25_34,
        demographic_35_44,
        demographic_45_54,
        demographic_55_64,
        demographic_65_up,
        demographic_female,
        demographic_male,
        demographic_unknown,
        action_weeks_other,
        action_line,
        action_line_open,
        action_part,
        action_header_all,
        action_has,
        action_caption,
        ss_label_shown,
        ss_label_budget,
        ss_label_rank,
        ss_title_shown,
        ss_title_budget,
        ss_title_rank,
        ss_tip,
        ss_named_budget,
        ss_named_rank,
        ss_alone,
        ss_all_limited,
        ss_all_zero,
        ss_none_exact,
        ss_under_one,
        ss_header_days,
        ss_under_ten,
        ss_days_of,
        ss_value,
        ss_aria_unknown,
        ss_unknown_text,
        hours_title,
        hours_between,
        hours_fewest_one,
        hours_most_lead,
        hours_most_of,
        hours_each,
        hours_after,
        hours_too_few,
        hours_day_line,
        hours_day_none,
        hours_cell,
        hours_header_weekday,
        hours_key_none,
        hours_key_fewer,
        hours_key_more,
        hours_caption,
        demographic_age_none,
        demographic_gender_none,
        demographic_age_thin,
        demographic_gender_thin,
        delay_note,
        change_of_campaign,
        export_file_fallback,
        deep_quality_title,
        deep_terms_title,
        deep_keywords_title,
        deep_pages_title,
        deep_ads_title,
        deep_name_keyword,
        deep_name_term,
        deep_name_page,
        deep_name_ad,
        deep_label_quality_score,
        deep_label_ctr,
        deep_label_relevance,
        deep_label_landing_page,
        deep_label_conversion_rate,
        deep_label_click_rate,
        deep_empty_terms,
        deep_empty_keywords,
        deep_empty_pages,
        deep_empty_ads,
        deep_no_score,
        deep_first_scores,
        deep_score_changes,
        deep_score_value,
        deep_score_aria,
        deep_score_up,
        deep_score_down,
        deep_score_same,
        deep_score_new,
        deep_score_none_before,
        deep_level_above,
        deep_level_average,
        deep_level_below,
        deep_level_none,
        deep_match_exact,
        deep_match_phrase,
        deep_match_broad,
        deep_quality_about_estimate,
        deep_quality_about_levels,
        deep_quality_about_auction,
        deep_term_ctr,
        deep_term_relevance,
        deep_term_landing_page,
        deep_term_exact,
        deep_term_phrase,
        deep_term_broad,
        deep_quality_action_ctr,
        deep_quality_action_relevance,
        deep_quality_action_landing_page,
        deep_quality_limit,
        deep_quality_none,
        deep_quality_one,
        deep_no_clicks,
        deep_no_conversions,
        deep_no_impressions,
        deep_spend_aria,
        deep_cost_per_click_is,
        deep_measured,
        deep_most_spend_term,
        deep_most_spend_keyword,
        deep_most_spend_page,
        deep_terms_one_converts,
        deep_terms_one_wasted,
        deep_flag_wasted,
        deep_terms_what,
        deep_terms_about_wasted,
        deep_terms_action_negative,
        deep_terms_action_add_converting,
        deep_terms_action_check_unconverting,
        deep_terms_action_add,
        deep_terms_limit_judged,
        deep_terms_limit,
        deep_keywords_one_none,
        deep_keywords_one_above,
        deep_keywords_one_below,
        deep_keywords_one_at,
        deep_keywords_one_converts,
        deep_vs_average_above,
        deep_vs_average_below,
        deep_vs_average_at,
        deep_vs_rate_above,
        deep_vs_rate_below,
        deep_vs_rate_at,
        deep_keywords_what,
        deep_keywords_about_cost,
        deep_keywords_about_verdict,
        deep_keywords_action_above,
        deep_keywords_action_wasted,
        deep_keywords_action_broad,
        deep_keywords_action_highest_cpc,
        deep_keywords_limit,
        deep_pages_one_above,
        deep_pages_one_below,
        deep_pages_one_at,
        deep_conversion_rate_aria,
        deep_conversion_rate_against_aria,
        deep_campaign_rate_key,
        deep_pages_what,
        deep_pages_about_rate,
        deep_pages_about_verdict,
        deep_pages_action_below,
        deep_pages_action_above,
        deep_pages_action_wrong,
        deep_pages_action_phone,
        deep_pages_limit_judged,
        deep_pages_limit,
        deep_strength_poor,
        deep_strength_average,
        deep_strength_good,
        deep_strength_excellent,
        deep_ads_none,
        deep_ads_one,
        deep_click_rate_aria,
        deep_ads_about_click_rate,
        deep_ads_about_strength,
        deep_ads_action_strength,
        deep_ads_action_lowest,
        deep_ads_action_none,
        deep_ads_limit,
        deep_ad_without_name,
        deep_count_from,
        follow_up_worked,
        follow_up_not_yet,
        follow_up_no_clear_effect,
        follow_up_cannot_judge,
        follow_up_period,
        follow_up_figures,
        follow_up_confounded,
        follow_up_not_serving,
        follow_up_few_results,
        follow_up_within_variation,
        follow_up_extra_spend,
        follow_up_spend_fell,
        follow_up_better,
        follow_up_worse,
        follow_up_paused,
        follow_up_enabled,
        follow_up_account_moved,
        follow_up_due,
    ],
    plurals: [
        export_period,
        email_left_out,
        alert_conversions_lead,
        shares_campaign_more,
        shares_device_more,
        shares_network_more,
        shares_location_more,
        shares_age_more,
        shares_gender_more,
        period_last_days,
        trend_no_range,
        trend_no_unusual,
        trend_after_days_was,
        trend_after_days_were,
        trend_last_was,
        trend_last_were,
        trend_empty_none,
        weeks_title,
        weeks_empty,
        period_days,
        page_ad_accounts_chosen,
        search_share_title,
        action_weeks_title,
        action_weeks_empty,
        action_mark,
        ss_coverage,
        ss_lower,
        hours_fewest_tied,
        hours_most_tied,
        export_file,
        deep_quality_low,
        deep_terms_all_convert,
        deep_terms_wasted,
        deep_keywords_above,
        deep_keywords_without,
        deep_keywords_all_convert,
        deep_keywords_none,
        deep_pages_below,
        deep_ads_best,
        follow_up_fewer_conversions,
        follow_up_fewer_clicks,
        deep_clicks_n,
        deep_conversions_n,
        deep_impressions_n,
    ],
}

impl Text {
    pub fn number(&self, n: f64, precision: usize) -> String {
        self.written.format.number(n, precision)
    }

    pub fn money(&self, value: f64, currency: &str) -> String {
        self.written.format.money(value, currency)
    }

    pub fn count(&self, value: f64) -> String {
        self.written.format.count(value)
    }

    pub fn whole_money(&self, value: f64, currency: &str) -> String {
        self.written.format.whole_money(value, currency)
    }

    fn percent_of_change(&self, current: f64, previous: f64) -> Option<String> {
        let percent = (current - previous).abs() / previous.abs() * 100.0;
        let tenths = (percent * 10.0).round() >= 100.0;
        if !tenths && format!("{percent:.1}") == "0.0" {
            return None;
        }
        Some(self.written.format.number(percent, usize::from(!tenths)))
    }

    pub fn change(&self, current: f64, previous: f64) -> Result<String> {
        if previous == 0.0 {
            return Ok(self.written.no_percentage_comparison.to_owned());
        }
        match self.percent_of_change(current, previous) {
            None => Ok(self.written.change_none.to_owned()),
            Some(percent) if current >= previous => say!(self, change_up, percent = percent),
            Some(percent) => say!(self, change_down, percent = percent),
        }
    }

    pub fn change_from(&self, current: f64, previous: f64, previous_text: &str) -> Result<String> {
        if previous == 0.0 {
            return Ok(self.written.no_percentage_comparison.to_owned());
        }
        match self.percent_of_change(current, previous) {
            None => Ok(self.written.change_none.to_owned()),
            Some(percent) if current >= previous => {
                say!(
                    self,
                    change_up_from,
                    percent = percent,
                    previous = previous_text
                )
            }
            Some(percent) => {
                say!(
                    self,
                    change_down_from,
                    percent = percent,
                    previous = previous_text
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::{
        language::{
            en,
            format::{CurrencyPlace, Part as DatePart, Range},
            text::placeholders,
            Language, Rule,
        },
        monitoring::{
            charts::{self, Subject},
            format,
        },
    };

    #[test]
    fn every_language_writes_the_placeholders_of_the_english_text() {
        let english = Language::En.text().unwrap();
        for language in Language::available() {
            let text = language.translation().unwrap();
            for key in Written::KEYS {
                let names = |written: &'static Written| -> Vec<Vec<&'static str>> {
                    written
                        .templates(key)
                        .unwrap()
                        .into_iter()
                        .map(|template| {
                            let mut names = placeholders(template).unwrap_or_else(|| {
                                panic!("{} `{key}` has a stray brace", language.code())
                            });
                            names.sort_unstable();
                            names.dedup();
                            names.retain(|name| *name != "count");
                            names
                        })
                        .collect()
                };
                let expected = names(&english.written)[0].clone();
                for found in names(&text.written) {
                    assert_eq!(found, expected, "{} `{key}`", language.code());
                }
            }
        }
    }

    fn other() -> Text {
        use DatePart::{Day, Literal, Month, Weekday};
        const SPACE: DatePart = Literal(" ");
        let english = en::TEXT;
        Text {
            rule: Rule::Romanian,
            written: Written {
                format: Format {
                    group: ".",
                    decimal: ",",
                    percent_suffix: " la sută",
                    usd_symbol: "US$",
                    currency_place: CurrencyPlace::After,
                    symbol_gap: " ",
                    code_gap: " ",
                    months: [
                        "ianuarie",
                        "februarie",
                        "martie",
                        "aprilie",
                        "mai",
                        "iunie",
                        "iulie",
                        "august",
                        "septembrie",
                        "octombrie",
                        "noiembrie",
                        "decembrie",
                    ],
                    weekdays: [
                        "luni",
                        "marți",
                        "miercuri",
                        "joi",
                        "vineri",
                        "sâmbătă",
                        "duminică",
                    ],
                    date_long: &[Weekday, Literal(", "), Day, SPACE, Month],
                    range_same_month: Range {
                        start: &[Day],
                        between: " până la ",
                        end: &[Day, SPACE, Month],
                    },
                    ..english.written.format
                },
                shares_takes: "ia ",
                shares_takes_both: "{spend} din cheltuieli și aduce {results} din {kind}.",
                shares_campaign_noun: "Campanie",
                results_conversions: "conversiile",
                metric_conversions: "Conversii",
                shares_header_spend: "Cheltuieli",
                share_of_spend: "Pondere din cheltuieli",
                share_of_results: "Pondere din {results}",
                shares_bar_spend_title: "{name}: {share} din cheltuieli, {amount} din {total}",
                shares_bar_spend_tip: "Cheltuieli: {amount} din {total}",
                shares_bar_results_title: "{name}: {share} din {kind}, {amount} din {total}",
                shares_bar_results_tip: "{kind}: {amount} din {total}",
                period_last_days: Plural::three(
                    "ultima zi",
                    "ultimele {count} zile",
                    "ultimele {count} de zile",
                ),
                ..english.written
            },
            ..english
        }
    }

    fn part(name: &str, spend: f64, results: f64) -> charts::Part {
        charts::Part {
            name: name.into(),
            href: None,
            spend,
            results,
        }
    }

    #[test]
    fn a_chart_an_amount_a_date_and_a_plural_read_from_the_table_of_the_language() {
        let other = other();
        let chart = charts::shares(
            &other,
            Subject::Campaign,
            "titlu",
            vec![part("A", 4121.0, 61.0), part("B", 780.0, 6.0)],
            true,
            "USD",
        )
        .unwrap();
        assert_eq!(chart.sentence.strong, "B");
        assert_eq!(chart.sentence.text, "ia ");
        assert_eq!(
            chart.sentence.mark,
            "16 la sută din cheltuieli și aduce 9 la sută din conversiile."
        );
        let table = chart.table.as_ref().unwrap();
        assert_eq!(
            table.headers,
            [
                "Campanie",
                "Cheltuieli",
                "Pondere din cheltuieli",
                "Conversii",
                "Pondere din conversiile"
            ]
        );
        assert_eq!(table.rows[0][1], "4.121 US$");
        assert_eq!(table.rows[1][1], "780 US$");
        assert_eq!(chart.rows[0].svg.matches("din cheltuieli").count(), 1);
        assert!(chart.rows[0]
            .svg
            .contains(r#"aria-label="A: 84 la sută din cheltuieli, 4.121 US$ din 4.901 US$""#));
        let everything = serde_json::to_string(&chart).unwrap();
        for english in [
            "Spend",
            "of spend",
            "Share",
            "takes",
            "brings",
            "Conversions",
            " of ",
        ] {
            assert!(!everything.contains(english), "{english} in {everything}");
        }

        assert_eq!(other.money(12.5, "EUR"), "12,50 EUR");
        assert_eq!(other.count(1234.5), "1.234,5");
        let monday = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        assert_eq!(format::long_date(&other, monday), "luni, 12 octombrie");
        assert_eq!(
            format::date_range(
                &other,
                monday,
                NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(),
                false
            ),
            "12 până la 18 octombrie"
        );
        assert_eq!(charts::last_days(&other, 1).unwrap(), "ultima zi");
        assert_eq!(charts::last_days(&other, 7).unwrap(), "ultimele 7 zile");
        assert_eq!(
            charts::last_days(&other, 30).unwrap(),
            "ultimele 30 de zile"
        );
    }

    #[test]
    fn english_writes_the_same_chart_with_its_own_table() {
        let english = en::TEXT;
        let chart = charts::shares(
            &english,
            Subject::Campaign,
            "title",
            vec![part("A", 4121.0, 61.0), part("B", 780.0, 6.0)],
            true,
            "USD",
        )
        .unwrap();
        assert_eq!(
            chart.sentence.words(),
            "B takes 16% of spend and brings 9% of conversions."
        );
        assert_eq!(english.money(12.5, "EUR"), "EUR 12.50");
    }
}

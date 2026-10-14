use loco_rs::{Error, Result};

use super::Written;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    One,
    Few,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub enum Rule {
    OneIsOne,
    ZeroAndOneAreOne,
    Romanian,
}

impl Rule {
    pub const fn form(self, count: u64) -> Form {
        match self {
            Rule::OneIsOne if count == 1 => Form::One,
            Rule::ZeroAndOneAreOne if count <= 1 => Form::One,
            Rule::Romanian if count == 1 => Form::One,
            Rule::Romanian if count == 0 || matches!(count % 100, 2..=19) => Form::Few,
            _ => Form::Other,
        }
    }

    pub fn form_of_amount(self, value: f64) -> Form {
        if value.fract() == 0.0 {
            self.form(value.abs() as u64)
        } else if matches!(self, Rule::ZeroAndOneAreOne) && value.abs() < 2.0 {
            Form::One
        } else {
            Form::Other
        }
    }
}

pub struct Plural {
    pub one: &'static str,
    pub few: &'static str,
    pub other: &'static str,
}

impl Plural {
    pub const fn two(one: &'static str, other: &'static str) -> Self {
        Self {
            one,
            few: other,
            other,
        }
    }

    pub const fn three(one: &'static str, few: &'static str, other: &'static str) -> Self {
        Self { one, few, other }
    }

    fn form(&self, form: Form) -> &'static str {
        match form {
            Form::One => self.one,
            Form::Few => self.few,
            Form::Other => self.other,
        }
    }
}

pub enum Entry {
    Line(&'static str),
    Plural(&'static Plural),
}

macro_rules! texts {
    (lines: [$($line:ident),* $(,)?], plurals: [$($plural:ident),* $(,)?] $(,)?) => {
        pub struct Text {
            pub rule: Rule,
            pub written: Written,
            $(pub $line: &'static str,)*
            $(pub $plural: Plural,)*
        }

        impl Text {
            pub const KEYS: &'static [&'static str] =
                &[$(stringify!($line),)* $(stringify!($plural),)*];

            #[cfg(feature = "marker-language")]
            pub(super) fn marked(english: &Text, rule: Rule, written: Written) -> Text {
                Text {
                    rule,
                    written,
                    $($line: super::marker::line(stringify!($line), english.$line),)*
                    $($plural: super::marker::plural(stringify!($plural), &english.$plural),)*
                }
            }

            pub fn entry(&'static self, key: &str) -> Option<Entry> {
                match key {
                    $(stringify!($line) => Some(Entry::Line(self.$line)),)*
                    $(stringify!($plural) => Some(Entry::Plural(&self.$plural)),)*
                    _ => None,
                }
            }
        }
    };
}

texts! {
    lines: [
        language_heading,
        language_hint,
        language_label,
        language_not_offered,
        save,
        back_to_ad_accounts,
        workspace_heading,
        google_connections_heading,
        remove,
        removing_connection_note,
        no_google_connection,
        delete_workspace,
        delete_workspace_summary,
        settings,
        cancel,
        continue_button,
        choice_yes,
        choice_no,
        choice_on,
        choice_off,
        error_message,
        error_title,
        error_summary_title,
        export_pdf,
        ad_accounts,
        subscription,
        connect_google_ads,
        sign_in_with_google,
        back_to,
        back_to_settings,
        step_of,
        hidden_detail,
        not_useful,
        saved_title,
        earlier_updates,
        target_label,
        note_label,
        tracking_title,
        tracking_none,
        use_account_setting,
        alerts_title,
        remove_title,
        remove_named,
        deep_check_heading,
        deep_check_none_title,
        explain_what_this_shows,
        explain_what_you_can_do,
        explain_data_limitation,
        subscription_start,
        skip_to_main,
        nav_main_label,
        sign_out,
        footer_read_only,
        connect_lead,
        connect_permission_note,
        delete_workspace_title,
        back_to_workspace,
        remove_workspace_lead,
        remove_workspace_detail,
        remove_connection_lead,
        remove_connection_detail,
        remove_confirm,
        select_title,
        select_none_title,
        select_none_lead,
        select_none_hint,
        select_already_added,
        auth_error_title,
        auth_start_again,
        auth_back_home,
        sign_in_lead,
        sign_in_trial,
        footer_privacy,
        footer_terms,
        content_description,
        content_resources_label,
        content_articles,
        content_thoughts,
        content_guides,
        content_tools,
        content_rss,
        content_empty,
        content_related,
        subscription_waiting_title,
        subscription_waiting_hint,
        subscription_unavailable,
        subscription_trial_note,
        subscription_inactive,
        subscription_payment_failed,
        subscription_payment_failed_action,
        subscription_trial_ended,
        subscription_restart,
        subscription_active,
        subscription_price_label,
        subscription_price_value,
        subscription_limit_value,
        subscription_first_payment,
        subscription_first_payment_start,
        subscription_first_payment_today,
        subscription_started,
        subscription_change_card,
        subscription_cancel,
        add_ad_account,
        add_ad_account_from,
        add_remove_first,
        chart_skip,
        deep_check_intro,
        deep_check_found_nothing,
        deep_check_none_text,
        explain_title,
        first_update,
        first_update_failed_lead,
        first_update_failed_text,
        first_update_reading,
        first_update_reading_text,
        first_update_check,
        window_label,
        recommendations_heading,
        recommendations_findings_heading,
        recommendation_finding,
        recommendation_impact,
        recommendation_action,
        recommendation_evidence,
        recommendation_done,
        findings_other_title,
        update_today,
        update_latest,
        update_comparison,
        update_campaign_line,
        update_marked_not_useful,
        update_not_useful_for,
        ad_accounts_update_of,
        ad_accounts_no_update,
        ad_accounts_empty,
        connections_and_workspace_link,
        ended_lead_subscribed,
        ended_lead_trial,
        ended_trial_updates,
        ended_no_updates,
        breakdowns_title,
        breakdowns_heading,
        breakdowns_deep_window,
        breakdowns_deep_none,
        remove_ad_account_lead,
        remove_ad_account_detail,
        ad_account_panel_empty_deep,
        ad_account_panel_empty_none,
        ad_account_first_title,
        ad_account_open_recommendations,
        ad_account_breakdowns_link,
        ad_account_campaigns_title,
        ad_account_campaign_column,
        ad_account_spark_days,
        ad_account_spark_label,
        ad_account_no_campaign,
        ad_account_no_earlier,
        campaign_alerts_off,
        campaign_alerts_off_effect,
        campaign_alerts_change,
        campaign_panel_empty,
        campaign_not_judged_title,
        campaign_not_judged_text,
        campaign_changes_title,
        campaign_changes_none,
        campaign_change_line,
        campaign_updates_title,
        campaign_updates_none,
        campaign_settings_title,
        campaign_settings_heading,
        campaign_profile_title,
        campaign_profile_hint,
        campaign_target_hint_set,
        campaign_target_hint_none,
        campaign_note_hint,
        campaign_tracking_working,
        campaign_tracking_few,
        campaign_yes_no_effect,
        campaign_judge_label,
        campaign_now_yes,
        campaign_now_no,
        campaign_now_on,
        campaign_now_off,
        campaign_judge_no_note,
        campaign_alerts_hint,
        campaign_alerts_label,
        settings_title,
        settings_profile_title,
        settings_profile_hint,
        settings_goal_label,
        settings_goal_none,
        settings_goal_leads,
        settings_goal_sales,
        settings_goal_calls,
        settings_goal_visits,
        settings_note_hint,
        settings_tracking_working,
        settings_tracking_few,
        settings_judge_label,
        settings_judge_no_note,
        settings_branding_title,
        settings_branding_hint,
        settings_brand_name,
        settings_brand_logo,
        settings_brand_logo_hint,
        settings_brand_logo_saved,
        settings_brand_colour,
        settings_weekly_title,
        settings_weekly_hint,
        settings_weekly_label,
        settings_remove_link,
        settings_your_ad_accounts,
        email_alert_for,
        email_what_to_do,
        email_open,
        email_weekly_heading,
        email_weekly_intro,
        email_spend_of,
        email_results_conversions,
        email_results_clicks,
        email_earlier_updates,
        email_no_other_updates,
        email_no_other_updates_no_recommendation,
        email_recommendation,
        email_finding,
        home_title,
        home_og_description,
        home_ld_description,
        home_hero_title,
        home_hero_lede,
        home_start_trial,
        home_campaign_emergency,
        home_update_title,
        home_update_intro,
        home_dashboard_title,
        home_dashboard_intro,
        home_missed_title,
        home_deep_title,
        home_deep_intro,
        home_rec_title,
        home_rec_intro,
        home_rec_outro,
        home_email_title,
        home_email_intro,
        home_alert_title,
        home_alert_text,
        home_alert_from,
        home_alert_subject,
        home_weekly_title,
        home_weekly_text,
        home_weekly_from,
        home_weekly_subject,
        home_export_title,
        home_export_text,
        home_export_note,
        home_trust_read,
        home_trust_privacy_link,
        home_trust_data,
        home_cannot_title,
        home_cannot_change,
        home_cannot_judge,
        home_cannot_sure,
        home_price_title,
        home_price_intro,
        home_price_trial,
        home_price_trial_days,
        home_price_trial_note,
        home_price_terms_link,
        home_price_after,
        home_faq_title,
        home_faq_change_q,
        home_faq_change_a,
        home_faq_know_q,
        home_faq_know_a,
        home_faq_start_q,
        home_faq_start_a,
        home_faq_tracking_q,
        home_faq_tracking_a,
        home_faq_advisor_q,
        home_faq_advisor_a,
        home_faq_agency_q,
        home_faq_ends_q,
        home_faq_ends_a,
        home_closing_title,
        home_nav_price,
        home_nav_sign_in,
        home_nav_start_trial,
        home_built_with,
        home_footer_label,
        home_problem_title,
        home_problem_intro,
        home_pain_stopped_title,
        home_pain_stopped_text,
        home_pain_clicks_title,
        home_pain_clicks_text,
        home_pain_budget_title,
        home_pain_budget_text,
        home_pain_columns_title,
        home_pain_columns_text,
        home_problem_close,
        home_point_morning_title,
        home_point_morning_text,
        home_point_why_title,
        home_point_why_text,
        home_point_calm_title,
        home_point_calm_text,
        home_point_quiet_title,
        home_point_quiet_text,
        home_missed_intro,
        home_shot_export_alt,
        home_hero_for,
        home_see_update,
        home_cta_note,
        home_definition,
        home_for_label,
        home_for_agency_clients,
        home_for_managers,
        home_start_title,
        home_start_1,
        home_start_2,
        home_start_3,
        home_start_note,
        home_founder_title,
        home_founder_hello,
        home_founder_start,
        home_founder_hard,
        home_founder_looks,
        home_founder_try,
        home_founder_sign,
        home_faq_consent_q,
        home_faq_consent_a,
        home_faq_more_q,
        home_faq_more_a,
        home_faq_ai_q,
        home_faq_ai_a,
        for_other_title,
        for_owners_link,
        for_agency_link,
        for_managers_link,
        for_home_link,
        content_cta_title,
        content_cta_text,
        sign_in_consent,
        for_owners_title,
        for_owners_description,
        for_owners_h1,
        for_owners_definition,
        for_owners_problem_title,
        for_owners_problem_text,
        for_owners_update_title,
        for_owners_alert_title,
        for_owners_deep_title,
        for_owners_rec_title,
        for_agency_title,
        for_agency_description,
        for_agency_h1,
        for_agency_lede,
        for_agency_definition,
        for_agency_problem_title,
        for_agency_problem_text,
        for_agency_rec_title,
        for_agency_rec_outro,
        for_agency_dash_title,
        for_agency_inbox_title,
        for_agency_inbox_intro,
        for_agency_faq_work_q,
        for_agency_faq_work_a,
        for_agency_faq_access_q,
        for_agency_faq_access_a,
        for_agency_faq_judge_q,
        for_agency_faq_judge_a,
        for_agency_closing,
        for_managers_title,
        for_managers_description,
        for_managers_h1,
        for_managers_lede,
        for_managers_definition,
        for_managers_problem_title,
        for_managers_problem_text,
        for_managers_update_title,
        for_managers_inbox_title,
        for_managers_inbox_intro,
        for_managers_faq_dashboard_q,
        for_managers_faq_dashboard_a,
        for_managers_closing,
        not_found_title,
        not_found_heading,
        not_found_text,
        not_found_back,
        content_title_blog,
        content_title_thoughts,
        content_title_guides,
        content_title_tools,
        content_title_resources,
    ],
    plurals: [
        google_connections_count,
        days_count,
        window_last_days,
        window_show_days,
        window_comparison,
        window_compared,
        subscription_trial_left,
        subscription_over_limit,
        subscription_over_limit_action,
        select_hint_limit,
        select_hint_found,
        add_usage,
        update_more_campaigns,
        show_more_updates,
        ended_price,
        ad_account_campaigns_caption,
        ad_account_export_note,
        home_description,
        home_ld_offer,
        home_price_per_month,
        home_faq_agency_a,
    ],
}

impl Text {
    pub fn get(&'static self, key: &str, values: &[(&str, String)]) -> Result<String> {
        match self.entry(key) {
            None => Err(Error::Message(format!("Unknown text key `{key}`"))),
            Some(Entry::Line(template)) => fill(key, template, values),
            Some(Entry::Plural(plural)) => {
                let count = values
                    .iter()
                    .find(|(name, _)| *name == "count")
                    .and_then(|(_, value)| value.parse::<u64>().ok())
                    .ok_or_else(|| {
                        Error::Message(format!("Text `{key}` needs a whole number `count`"))
                    })?;
                fill(key, plural.form(self.rule.form(count)), values)
            }
        }
    }

    pub fn plural(&self, forms: &Plural, count: u64, values: &[(&str, &str)]) -> Result<String> {
        let mut all = vec![("count", count.to_string())];
        all.extend(
            values
                .iter()
                .map(|(name, value)| (*name, (*value).to_owned())),
        );
        fill("plural", forms.form(self.rule.form(count)), &all)
    }

    pub fn plural_amount(
        &self,
        forms: &Plural,
        value: f64,
        values: &[(&str, &str)],
    ) -> Result<String> {
        let mut all = vec![("count", self.count(value))];
        all.extend(
            values
                .iter()
                .map(|(name, value)| (*name, (*value).to_owned())),
        );
        fill("plural", forms.form(self.rule.form_of_amount(value)), &all)
    }
}

enum Piece<'a> {
    Literal(&'a str),
    Slot(&'a str),
}

fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

fn pieces(template: &str) -> Option<Vec<Piece<'_>>> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find(['{', '}']) {
        let (name, tail) = rest[start..].strip_prefix('{')?.split_once('}')?;
        if !is_name(name) {
            return None;
        }
        pieces.push(Piece::Literal(&rest[..start]));
        pieces.push(Piece::Slot(name));
        rest = tail;
    }
    if rest.contains(['{', '}']) {
        return None;
    }
    pieces.push(Piece::Literal(rest));
    Some(pieces)
}

#[cfg(any(test, feature = "marker-language"))]
pub fn placeholders(template: &str) -> Option<Vec<&str>> {
    Some(
        pieces(template)?
            .into_iter()
            .filter_map(|piece| match piece {
                Piece::Slot(name) => Some(name),
                Piece::Literal(_) => None,
            })
            .collect(),
    )
}

pub fn fill<V: AsRef<str>>(key: &str, template: &str, values: &[(&str, V)]) -> Result<String> {
    let pieces = pieces(template)
        .ok_or_else(|| Error::Message(format!("Text `{key}` has a stray brace")))?;
    let mut out = String::with_capacity(template.len());
    let mut used = vec![false; values.len()];
    for piece in pieces {
        match piece {
            Piece::Literal(text) => out.push_str(text),
            Piece::Slot(name) => {
                let index = values
                    .iter()
                    .position(|(given, _)| *given == name)
                    .ok_or_else(|| {
                        Error::Message(format!("Text `{key}` needs a value for `{name}`"))
                    })?;
                used[index] = true;
                out.push_str(values[index].1.as_ref());
            }
        }
    }
    for ((name, _), used) in values.iter().zip(used) {
        if !used && *name != "count" {
            return Err(Error::Message(format!(
                "Text `{key}` has no place for `{name}`"
            )));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_named_placeholders_in_any_order() {
        assert_eq!(
            fill("k", "{b} then {a} then {b}", &[("a", "1"), ("b", "2")]).unwrap(),
            "2 then 1 then 2"
        );
    }

    #[test]
    fn refuses_a_missing_value_an_unused_value_and_a_stray_brace() {
        assert!(fill("k", "Hello {name}", &[("nme", "x")]).is_err());
        assert!(fill("k", "Hello", &[("name", "x")]).is_err());
        assert!(fill("k", "Hello {name", &[("name", "x")]).is_err());
        assert!(fill("k", "Hello }", &[("name", "x")]).is_err());
        assert!(fill("k", "Hello {Name}", &[("Name", "x")]).is_err());
        assert_eq!(fill("k", "One", &[("count", "1")]).unwrap(), "One");
    }

    #[test]
    fn reads_the_placeholders_of_a_template() {
        assert_eq!(placeholders("{a} and {b}"), Some(vec!["a", "b"]));
        assert_eq!(placeholders("none"), Some(vec![]));
        assert_eq!(placeholders("{a"), None);
    }

    #[test]
    fn rules_pick_the_form_of_each_language_group() {
        let forms = |rule: Rule| [0, 1, 2, 5, 19, 20, 21, 100, 101, 102, 119].map(|n| rule.form(n));
        use Form::{Few, One, Other};
        assert_eq!(
            forms(Rule::OneIsOne),
            [Other, One, Other, Other, Other, Other, Other, Other, Other, Other, Other]
        );
        assert_eq!(
            forms(Rule::ZeroAndOneAreOne),
            [One, One, Other, Other, Other, Other, Other, Other, Other, Other, Other]
        );
        assert_eq!(
            forms(Rule::Romanian),
            [Few, One, Few, Few, Few, Other, Other, Other, Other, Few, Few]
        );
    }

    #[test]
    fn a_plural_picks_its_form_and_fills_the_count() {
        static FORMS: Plural = Plural::three("o zi", "{count} zile", "{count} de zile");
        let text = crate::language::en::TEXT;
        let romanian = Text {
            rule: Rule::Romanian,
            ..text
        };
        for (count, expected) in [
            (1, "o zi"),
            (2, "2 zile"),
            (19, "19 zile"),
            (20, "20 de zile"),
            (101, "101 de zile"),
            (102, "102 zile"),
        ] {
            assert_eq!(romanian.plural(&FORMS, count, &[]).unwrap(), expected);
        }
    }

    #[test]
    fn an_amount_with_a_fraction_picks_its_form_and_writes_the_count_of_the_language() {
        use Form::{One, Other};
        assert_eq!(Rule::OneIsOne.form_of_amount(1.0), One);
        assert_eq!(Rule::OneIsOne.form_of_amount(1.5), Other);
        assert_eq!(Rule::ZeroAndOneAreOne.form_of_amount(1.5), One);
        assert_eq!(Rule::ZeroAndOneAreOne.form_of_amount(2.5), Other);
        assert_eq!(Rule::Romanian.form_of_amount(1.5), Other);
        static FORMS: Plural = Plural::two("{count} conversion", "{count} conversions");
        let text = crate::language::en::TEXT;
        assert_eq!(
            text.plural_amount(&FORMS, 1.0, &[]).unwrap(),
            "1 conversion"
        );
        assert_eq!(
            text.plural_amount(&FORMS, 12345.0, &[]).unwrap(),
            "12,345 conversions"
        );
        assert_eq!(
            text.plural_amount(&FORMS, 1.5, &[]).unwrap(),
            "1.5 conversions"
        );
    }
}

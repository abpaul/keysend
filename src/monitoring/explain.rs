use std::collections::BTreeMap;

use serde::Serialize;

use crate::language::Text;

#[derive(Debug, Serialize, PartialEq)]
pub struct Explain {
    pub about: Vec<String>,
    pub actions: Vec<String>,
    pub limitation: Option<String>,
}

fn explain(about: &[&str], actions: &[&str], limitation: Option<&str>) -> Explain {
    let owned = |texts: &[&str]| texts.iter().map(|text| (*text).to_owned()).collect();
    Explain {
        about: owned(about),
        actions: owned(actions),
        limitation: limitation.map(str::to_owned),
    }
}

pub fn charts(text: &Text, judged: bool) -> BTreeMap<&'static str, Explain> {
    let w = &text.written;
    let not_results = w.explain_not_results;
    let look = if judged {
        w.explain_look_conversions
    } else {
        w.explain_look_clicks
    };
    let shares = |about: &str, bars: &str, action: &str, limitation: &str| {
        explain(
            &[about, bars],
            &[look, action],
            Some(if judged { limitation } else { not_results }),
        )
    };
    let mut all = BTreeMap::from([
        (
            "metrics",
            explain(
                &if judged {
                    vec![
                        w.explain_metrics_spend,
                        w.explain_metrics_conversion,
                        w.explain_metrics_cost_per_conversion,
                        w.explain_metrics_click,
                    ]
                } else {
                    vec![
                        w.explain_metrics_spend,
                        w.explain_metrics_click,
                        w.explain_metrics_cost_per_click,
                        w.explain_metrics_impression,
                    ]
                },
                &[
                    w.explain_metrics_read_change,
                    w.explain_metrics_choose_number,
                ],
                Some(if judged {
                    w.explain_metrics_limit_judged
                } else {
                    not_results
                }),
            ),
        ),
        (
            "trend",
            explain(
                &[
                    w.explain_trend_line,
                    w.explain_trend_band,
                    w.explain_trend_points,
                ],
                &[w.explain_trend_unusual, w.explain_trend_change],
                Some(w.explain_trend_limit),
            ),
        ),
        (
            "search-share",
            explain(
                &[
                    w.explain_search_estimate,
                    w.explain_search_shown,
                    w.explain_search_budget,
                    w.explain_search_rank,
                ],
                &[
                    w.explain_search_budget_action,
                    w.explain_search_remove_action,
                    w.explain_search_rank_action,
                ],
                Some(w.explain_search_limit),
            ),
        ),
        (
            "campaign-shares",
            shares(
                w.explain_campaign_about,
                if judged {
                    w.explain_campaign_bars_conversions
                } else {
                    w.explain_campaign_bars_clicks
                },
                w.explain_campaign_action,
                w.explain_campaign_limit,
            ),
        ),
        (
            "device-shares",
            shares(
                w.explain_device_about,
                if judged {
                    w.explain_device_bars_conversions
                } else {
                    w.explain_device_bars_clicks
                },
                w.explain_device_action,
                w.explain_device_limit,
            ),
        ),
        (
            "network-shares",
            shares(
                w.explain_network_about,
                if judged {
                    w.explain_network_bars_conversions
                } else {
                    w.explain_network_bars_clicks
                },
                w.explain_network_action,
                w.explain_network_limit,
            ),
        ),
        (
            "locations",
            shares(
                w.explain_locations_about,
                if judged {
                    w.explain_locations_bars_conversions
                } else {
                    w.explain_locations_bars_clicks
                },
                w.explain_locations_action,
                w.explain_locations_limit,
            ),
        ),
        (
            "ages",
            shares(
                w.explain_ages_about,
                if judged {
                    w.explain_ages_bars_conversions
                } else {
                    w.explain_ages_bars_clicks
                },
                w.explain_ages_action,
                w.explain_ages_limit,
            ),
        ),
        (
            "genders",
            shares(
                w.explain_genders_about,
                if judged {
                    w.explain_genders_bars_conversions
                } else {
                    w.explain_genders_bars_clicks
                },
                w.explain_genders_action,
                w.explain_genders_limit,
            ),
        ),
        (
            "hours",
            explain(
                &[
                    if judged {
                        w.explain_hours_about_conversions
                    } else {
                        w.explain_hours_about_clicks
                    },
                    w.explain_hours_zone,
                ],
                &[w.explain_hours_answer, w.explain_hours_schedule],
                Some(w.explain_hours_limit),
            ),
        ),
    ]);
    if judged {
        all.insert(
            "cost-weeks",
            explain(
                &[w.explain_cost_weeks_bar, w.explain_cost_weeks_lower],
                &[w.explain_cost_weeks_compare, w.explain_cost_weeks_table],
                Some(w.explain_cost_weeks_limit),
            ),
        );
        all.insert(
            "action-weeks",
            explain(
                &[w.explain_action_weeks_what, w.explain_action_weeks_colour],
                &[w.explain_action_weeks_value, w.explain_action_weeks_tag],
                Some(w.explain_action_weeks_limit),
            ),
        );
    }
    all
}

#[cfg(test)]
mod tests {
    use super::charts;
    use crate::language::english;

    #[test]
    fn a_page_that_is_not_judged_by_conversions_explains_without_a_conversion_word() {
        for (id, explain) in charts(english(), false) {
            let mut texts = explain.about;
            texts.extend(explain.actions);
            texts.extend(explain.limitation);
            assert!(
                texts
                    .iter()
                    .all(|text| !text.to_lowercase().contains("conversion")),
                "{id}: {texts:?}"
            );
        }
        assert!(!charts(english(), false).contains_key("cost-weeks"));
        assert_eq!(
            charts(english(), true)["cost-weeks"].about[0],
            "Each bar is the cost per conversion of one week: the spend divided by the conversions."
        );
    }
}

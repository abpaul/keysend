//! What a conversion is worth to the business, from the optional economics of the Ad Account
//! profile, and the thresholds at which advertising stops paying.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::metrics::{english_money, english_number};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Economics {
    /// The gross margin of a sale, from 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    /// The revenue of one new customer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_value: Option<f64>,
    /// The share of conversions that become customers, from 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_return: Option<f64>,
}

fn percent(share: f64) -> String {
    format!("{}%", english_number(share * 100.0, 0))
}

impl Economics {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The return on ad spend below which the product margin does not cover the ad cost.
    pub fn break_even_return(&self) -> Option<f64> {
        self.margin.map(|margin| 1.0 / margin)
    }

    /// The revenue that one conversion brings on average.
    pub fn conversion_value(&self) -> Option<f64> {
        Some(self.customer_value? * self.close_rate?)
    }

    /// The cost per conversion above which a conversion costs more than the profit it brings,
    /// or than its revenue when no margin is given.
    pub fn break_even_cost(&self) -> Option<f64> {
        Some(self.conversion_value()? * self.margin.unwrap_or(1.0))
    }

    /// Each threshold with the sentence that derives it, for the evidence of the Assessment.
    pub fn facts(&self, currency: &str) -> Value {
        let money = |value: f64| english_money(value, currency);
        let mut facts = json!({});
        if let Some(margin) = self.margin {
            facts["margin"] = percent(margin).into();
        }
        if let Some((margin, threshold)) = self.margin.zip(self.break_even_return()) {
            facts["break_even_return_on_ad_spend"] = json!({
                "value": english_number(threshold, 2),
                "derivation": format!("1 divided by the margin of {}: below a return of {}, the product margin does not cover the ad cost", percent(margin), english_number(threshold, 2)),
            });
        }
        if let Some(target) = self.target_return {
            facts["target_return_on_ad_spend"] = english_number(target, 2).into();
        }
        if let Some(((value, rate), worth)) = self
            .customer_value
            .zip(self.close_rate)
            .zip(self.conversion_value())
        {
            facts["value_of_a_conversion"] = json!({
                "value": money(worth),
                "derivation": format!("a customer worth {} times {} of conversions becoming customers", money(value), percent(rate)),
            });
        }
        if let Some((cost, worth)) = self.break_even_cost().zip(self.conversion_value()) {
            facts["break_even_cost_per_conversion"] = json!({
                "value": money(cost),
                "derivation": match self.margin {
                    Some(margin) => format!("a conversion worth {} times the margin of {}: above a cost of {} per conversion, a conversion costs more than the profit it brings", money(worth), percent(margin), money(cost)),
                    None => format!("a conversion worth {}: above that cost per conversion, a conversion costs more than its revenue; no margin is given, so profit is not known", money(worth)),
                },
            });
        }
        facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thresholds_follow_from_the_margin_the_customer_value_and_the_close_rate() {
        let shop = Economics {
            margin: Some(0.25),
            ..Default::default()
        };
        assert_eq!(shop.break_even_return(), Some(4.0));
        assert_eq!(shop.break_even_cost(), None);
        let installer = Economics {
            margin: Some(0.4),
            customer_value: Some(1500.0),
            close_rate: Some(0.2),
            target_return: None,
        };
        assert_eq!(installer.conversion_value(), Some(300.0));
        assert_eq!(installer.break_even_cost(), Some(120.0));
        assert_eq!(
            installer.facts("USD")["break_even_cost_per_conversion"]["derivation"],
            "a conversion worth $300 times the margin of 40%: above a cost of $120 per conversion, a conversion costs more than the profit it brings"
        );
        let unknown_margin = Economics {
            margin: None,
            ..installer
        };
        assert_eq!(unknown_margin.break_even_cost(), Some(300.0));
    }
}

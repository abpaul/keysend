use super::{scenarios::*, *};

pub(super) fn all() -> Vec<Scenario> {
    let mut seasonal = case(
        "M_seasonal",
        "Is this fall seasonal compared with last year?",
        preset("seasonal_demand"),
        &["compare_periods"],
    );
    let mut device = preset("healthy_account");
    device.devices = vec![
        (
            "Mobile",
            crate::monitoring::eval::Row {
                campaign: 111,
                name: "",
                clicks: 1800.0,
                cost: 3600.0,
                conversions: 18.0,
                value: 0.0,
                strength: "",
            },
        ),
        (
            "Desktop",
            crate::monitoring::eval::Row {
                campaign: 111,
                name: "",
                clicks: 200.0,
                cost: 400.0,
                conversions: 20.0,
                value: 0.0,
                strength: "",
            },
        ),
    ];
    device.situation = "Current device totals show Mobile taking most spend at much worse CPA than Desktop. No device baseline is available; a shift or a device-specific cause cannot be established.";
    device.essentials = &[
        "Identifies Mobile's high spend and worse conversion efficiency versus Desktop.",
        "Does not claim a proven device shift or cause from current totals without a baseline.",
    ];
    device.mistakes = &["Claims a device shift without comparative evidence."];
    let mut device = case(
        "N_devices",
        "Could a shift to mobile explain worse results?",
        device,
        &["investigate_segments"],
    );
    device.unneeded = &["business_context", "investigate_search_terms"];
    let mut spend = case(
        "O_spend",
        "What was spend last week?",
        preset("healthy_account"),
        &["account_overview"],
    );
    spend.period = Period::Last7Days;
    spend.situation =
        "The User asks only for last week's spend. A single overview or comparison is sufficient.";
    spend.essentials = &[
        "Answers last seven days' spend correctly and directly.",
        "Avoids an unnecessary diagnostic investigation.",
    ];
    spend.mistakes = &["Uses last thirty days instead of last week."];
    spend.unneeded = &[
        "investigate_conversions",
        "compare_campaigns",
        "investigate_search_terms",
        "review_history",
        "review_economics",
        "business_context",
        "investigate_segments",
    ];
    seasonal.held_out = true;
    device.held_out = true;
    spend.held_out = true;
    vec![seasonal, device, spend]
}

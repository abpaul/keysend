use super::scenarios::*;

pub(super) fn all() -> Vec<Scenario> {
    let mut winner = preset("healthy_account");
    winner.business = "Alder Dental";
    winner.target_cost = Some(50.0);
    winner.margin = Some(0.5);
    winner.customer_value = Some(1000.0);
    winner.close_rate = Some(0.2);
    winner.campaigns = vec![plan(
        201,
        "Search - Implants",
        level(120.0, 60.0, 4.0),
        level(120.0, 60.0, 4.0),
    )];
    winner.campaigns[0].lost_to_budget = Some(0.4);
    let mut cheap = preset("healthy_account");
    cheap.business = "Birch Dental";
    cheap.target_cost = Some(10.0);
    cheap.margin = Some(0.2);
    cheap.customer_value = Some(200.0);
    cheap.close_rate = Some(0.2);
    cheap.campaigns = vec![plan(
        202,
        "Search - Checkups",
        level(60.0, 60.0, 4.0),
        level(60.0, 60.0, 4.0),
    )];
    let mut unknown = preset("healthy_account");
    unknown.business = "Maple Dental";
    unknown.target_cost = Some(40.0);
    unknown.margin = None;
    unknown.customer_value = None;
    unknown.close_rate = None;
    unknown.campaigns = vec![plan(
        203,
        "Search - Dentistry",
        level(100.0, 50.0, 4.0),
        level(100.0, 50.0, 4.0),
    )];
    let mut funding = case(
        "P_funding",
        "Across these three Ad Accounts, where should the next budget go given their economics?",
        winner,
        &["compare_campaigns", "review_economics"],
    );
    funding.accounts.extend([cheap, unknown]);
    funding.situation = "Three USD Ad Accounts with stable measured results. Alder has CPA 30, target 50, break-even CPA 100 and 40% lost to budget. Birch has cheaper CPA 15 but target 10 and break-even CPA 8. Maple has CPA 25 and target 40 but no economics. Alder is the conditional next-budget candidate; cheap leads alone do not make Birch profitable. No marginal return or lead quality is measured.";
    funding.essentials = &[
        "Prioritizes Alder for a conditional budget increase using its budget constraint and favorable target and break-even economics.",
        "Explains why Birch's cheaper CPA does not justify more budget: it exceeds its break-even cost.",
        "Treats Maple's missing economics and unmeasured marginal returns or lead quality as limits rather than inventing a precise allocation.",
    ];
    funding.mistakes = &[
        "Chooses Birch merely because its CPA is lowest.",
        "Invents a precise profitable budget allocation or missing economics.",
    ];
    funding.unneeded = &[
        "investigate_search_terms",
        "investigate_segments",
        "business_context",
    ];

    let mut display = preset("healthy_account");
    display.campaigns = vec![
        plan(
            211,
            "Display - Prospecting",
            level(100.0, 100.0, 2.0),
            level(200.0, 200.0, 2.0),
        ),
        plan(
            212,
            "Search - Appointments",
            level(100.0, 50.0, 5.0),
            level(100.0, 50.0, 5.0),
        ),
    ];
    display.campaigns[0].channel = "DISPLAY";
    display.search_terms.clear();
    display.keywords.clear();
    let mut channel = case(
        "Q_display",
        "Spend grew without more leads. Is Search responsible, or Display? What should I check?",
        display,
        &["compare_campaigns"],
    );
    channel.situation = "Display - Prospecting doubled spend and clicks with flat conversions, doubling CPA and halving conversion rate. Search - Appointments held spend and conversions. There are no search terms for Display and no evidence proving a targeting, creative or website cause.";
    channel.essentials = &[
        "Localizes the extra spend and worse efficiency to Display - Prospecting while Search - Appointments held steady.",
        "Explains Display's flat conversions despite extra spend and clicks without inventing a proven cause.",
        "Suggests a channel-appropriate investigation before scaling or cuts, without claiming missing search terms diagnose Display.",
    ];
    channel.mistakes = &[
        "Blames Search for Display's deterioration.",
        "Invents Display search terms or a proven targeting or creative cause.",
    ];
    channel.unneeded = &["investigate_search_terms", "business_context"];

    let mut leads = preset("healthy_account");
    leads.goal = "leads";
    leads.campaigns = vec![plan(
        221,
        "Search - Bookings",
        level(100.0, 50.0, 4.0),
        level(100.0, 50.0, 3.8),
    )];
    let mut small = case(
        "R_small_drop",
        "Is the drop in leads real or a tracking issue?",
        leads,
        &["compare_periods", "investigate_conversions"],
    );
    small.situation = "Working conversion actions, no lag and no stopped recording. Spend and clicks hold; recorded leads fall about 5%, from about 120 to 114 over 30 days. This is a real small fall in recorded leads, not evidence of broken tracking, and not proof of a statistically established underlying decline or a known cause.";
    small.essentials = &[
        "Directly distinguishes the small measured fall in recorded leads from a tracking failure, with tracking working.",
        "Quantifies or clearly describes the small decline with stable traffic without declaring a proven underlying trend or cause.",
        "Recommends proportionate monitoring or a focused check rather than an urgent tracking repair or major budget cut.",
    ];
    small.mistakes = &[
        "Calls working tracking broken or stopped.",
        "Treats the small fall as a proven collapse requiring a major budget cut.",
    ];
    small.unneeded = &[
        "investigate_search_terms",
        "investigate_segments",
        "business_context",
    ];
    for scenario in [&mut funding, &mut channel, &mut small] {
        scenario.held_out = true;
    }
    vec![funding, channel, small]
}

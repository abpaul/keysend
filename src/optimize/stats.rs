//! The counts that decide whether there is enough evidence to act.
//!
//! Conversions are counts. A Campaign that spent `S` at a target cost per conversion `T`
//! would record about `S / T` conversions; under a Poisson count with that mean, the chance
//! of recording `x` or fewer is [`poisson_at_most`]. A small chance is evidence that the cost
//! per conversion is above the target; a large one is not, however high the observed cost
//! looks. The same test with `x = 0` decides when spend without conversions is more than
//! chance: `e^(-S/T)`.
//!
//! Two periods are compared with [`later`]: both counts are uncertain, so neither is taken as
//! the true rate of the other. The tests apply only to whole counts ([`whole`]).

/// A count of conversions as a whole number. Data-driven attribution credits fractions of a
/// conversion; a sum of fractions is not a Poisson count, and rounding it would invent one, so
/// a fractional count gives `None` and no test judges it.
pub fn whole(conversions: f64) -> Option<u64> {
    (conversions >= 0.0 && (conversions - conversions.round()).abs() < 1e-9)
        .then(|| conversions.round() as u64)
}

/// The chance that a Poisson count with this mean is `count` or less. Counted in logarithms,
/// so a mean in the thousands does not underflow.
pub fn poisson_at_most(count: u64, mean: f64) -> f64 {
    if mean <= 0.0 {
        return 1.0;
    }
    let mut log_factorial = 0.0;
    let mut total = 0.0;
    for k in 0..=count {
        if k > 0 {
            log_factorial += (k as f64).ln();
        }
        total += (-mean + k as f64 * mean.ln() - log_factorial).exp();
    }
    total.min(1.0)
}

/// The chance that a Binomial count of `trials` with this `share` is `count` or less.
pub fn binomial_at_most(count: u64, trials: u64, share: f64) -> f64 {
    if count >= trials || share <= 0.0 {
        return 1.0;
    }
    if share >= 1.0 {
        return 0.0;
    }
    let (yes, no) = (share.ln(), (1.0 - share).ln());
    let mut log_choose = 0.0;
    let mut total = 0.0;
    for k in 0..=count {
        if k > 0 {
            log_choose += ((trials - k + 1) as f64).ln() - (k as f64).ln();
        }
        total += (log_choose + k as f64 * yes + (trials - k) as f64 * no).exp();
    }
    total.min(1.0)
}

/// The chance that `after` or more of the `before + after` counts (with `more` false, `after`
/// or fewer) fall in the later period if both periods had the same rate, the later period
/// having `share` of the exposure (its spend, or its days). For two independent Poisson counts
/// with equal rates, the later count given the total is Binomial with that share, whatever
/// the rate: the exact conditional test, which needs no estimate of the rate before.
pub fn later(before: u64, after: u64, share: f64, more: bool) -> f64 {
    let trials = before + after;
    if more {
        if after == 0 {
            1.0
        } else {
            1.0 - binomial_at_most(after - 1, trials, share)
        }
    } else {
        binomial_at_most(after, trials, share)
    }
}

/// The level below which a count is taken as evidence. One in twenty, chosen before any
/// measurement, as is usual for a one-sided test; it is not a calibrated error rate, since
/// daily rows are not independent draws.
pub const EVIDENCE: f64 = 0.05;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chance_of_a_count_matches_the_poisson_distribution() {
        // P(X = 0) for mean 3 is e^-3.
        assert!((poisson_at_most(0, 3.0) - 0.049_787).abs() < 1e-6);
        // P(X <= 2) for mean 5 is 0.124652.
        assert!((poisson_at_most(2, 5.0) - 0.124_652).abs() < 1e-6);
        // P(X <= 20) for mean 30 is 0.035285.
        assert!((poisson_at_most(20, 30.0) - 0.035_285).abs() < 1e-6);
        assert!(poisson_at_most(900, 1000.0) < 0.001);
        assert_eq!(poisson_at_most(4, 0.0), 1.0);
    }

    #[test]
    fn two_periods_are_compared_with_both_counts_uncertain() {
        assert!((later(40, 52, 0.5, true) - 0.125_661).abs() < 1e-6);
        assert!(
            (1.0 - poisson_at_most(51, 40.0) - 0.038_740).abs() < 1e-6,
            "the one-sample test that takes the count before as the true rate"
        );
        assert!((later(40, 60, 0.5, true) - 0.028_444).abs() < 1e-6);
        assert!((later(40, 20, 0.5, false) - 0.006_745).abs() < 1e-6);
        assert!((later(40, 100, 2.0 / 3.0, true) - 0.133_894).abs() < 1e-6);
        assert_eq!(later(0, 0, 0.5, true), 1.0);
        assert_eq!(later(0, 0, 0.5, false), 1.0);
    }

    #[test]
    fn only_a_whole_count_of_conversions_is_a_count() {
        assert_eq!(whole(0.0), Some(0));
        assert_eq!(whole(31.0), Some(31));
        assert_eq!(whole(0.4), None);
        assert_eq!(whole(19.84), None);
        assert_eq!(whole(-1.0), None);
    }
}

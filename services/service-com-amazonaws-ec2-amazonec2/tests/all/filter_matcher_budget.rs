// SPDX-License-Identifier: CC0-1.0

//! Two rules of the EC2 filter matcher every operation that filters shares, asserted
//! directly on [`ec2::state::FilterPattern`] rather than through one describe, because they
//! belong to the matcher and not to a caller:
//!
//! 1. A RUN OF `*` CHANGES NO ANSWER. "Zero or more characters" followed by "zero or more
//!    characters" is still zero or more, so a value carrying a run matches exactly what the
//!    same value with one `*` in that place matches. Each row states the answer it expects
//!    outright AND that the two patterns agree, so neither a matcher that says "no" to
//!    everything nor one that says "yes" to everything can satisfy it;
//! 2. EVERY COMPARISON IS CHARGED to the [`ec2::state::MatchBudget`] the caller gave —
//!    including one against a candidate value the matcher's character loop never enters —
//!    which is what makes one request's matcher work finite. (The per-token charging of a
//!    pattern's tail and the token ceiling are asserted by
//!    `every_comparison_the_shared_matcher_makes_is_charged_to_the_budget` in
//!    `tests/all/import_image_tasks.rs`; what is here is the entry charge and what a
//!    one-step budget can pay for.)
//!
//! Each row is a distinct input along what the rule generalises over: how long the wildcard
//! run is and where in the pattern it sits, what the candidate holds, and how much budget is
//! left.

use ec2::state::{FilterPattern, MatchBudget};
use service_com_amazonaws_ec2_amazonec2 as ec2;

/// Rule 1: a run of `*` matches what it should, and what a single `*` in the same place
/// matches.
#[test]
fn a_run_of_wildcards_matches_what_one_wildcard_matches() {
    // Rows: (the pattern carrying a run, the single-wildcard pattern it must agree with, the
    // candidate, the answer both must give).
    let rows: [(&str, &str, &str, bool); 16] = [
        ("**", "*", "", true),
        ("**", "*", "a", true),
        ("*****", "*", "sg-0123456789abcdef0", true),
        ("a**", "a*", "a", true),
        ("a**", "a*", "ab", true),
        ("a**", "a*", "b", false),
        ("**b", "*b", "b", true),
        ("**b", "*b", "ab", true),
        ("**b", "*b", "a", false),
        ("a***b", "a*b", "ab", true),
        ("a***b", "a*b", "axyb", true),
        ("a***b", "a*b", "a", false),
        ("a**?**b", "a*?*b", "axb", true),
        ("a**?**b", "a*?*b", "ab", false),
        ("**a**", "*a*", "xay", true),
        ("**a**", "*a*", "b", false),
    ];
    for (run, one, candidate, expected) in rows {
        let got = FilterPattern::compile(run).matches(candidate);
        assert_eq!(
            got, expected,
            "case {run:?} against {candidate:?}: expected {expected}, got {got}"
        );
        let agreed = FilterPattern::compile(one).matches(candidate);
        assert_eq!(
            got, agreed,
            "case {run:?} against {candidate:?}: expected the answer of {one:?} ({agreed}), \
             got {got}"
        );
    }
}

/// Rule 2: a comparison costs a step before the matcher looks at a character, so no shape of
/// pattern or candidate does matcher work for free.
#[test]
fn every_comparison_is_charged_to_the_budget() {
    // Rows: the pattern shapes whose cost would otherwise fall outside the loop over the
    // candidate — a bare wildcard, a wildcard run, a pattern with nothing left to consume, a
    // literal, and the empty value — each compared against the empty candidate, which that
    // loop never enters.
    let patterns = ["*", "**********", "a*", "a", ""];
    for pattern in patterns {
        let compiled = FilterPattern::compile(pattern);

        // No budget cannot pay for the comparison, so the answer is "no match" and the
        // budget says why.
        let mut spent = MatchBudget::new(0);
        assert!(
            !compiled.matches_within("", &mut spent),
            "case {pattern:?}: a comparison with no budget must answer no match"
        );
        assert!(
            spent.exhausted(),
            "case {pattern:?}: a comparison with no budget must leave it exhausted"
        );

        // A budget of one pays for exactly one comparison against the empty candidate, so
        // nothing is left afterwards — which is what proves that comparison was charged.
        let mut one_step = MatchBudget::new(1);
        let _ = compiled.matches_within("", &mut one_step);
        assert!(
            one_step.exhausted(),
            "case {pattern:?}: one comparison against the empty value must cost a step"
        );
    }

    // The same across candidates: one step cannot pay for a comparison per candidate.
    let candidates = ["", "a", "ab", "sg-0123456789abcdef0", "a*b", "a?b"];
    let compiled = FilterPattern::compile("*");
    let mut budget = MatchBudget::new(1);
    for candidate in candidates {
        let _ = compiled.matches_within(candidate, &mut budget);
    }
    assert!(
        budget.exhausted(),
        "a budget of one step cannot pay for {} comparisons",
        candidates.len()
    );
}

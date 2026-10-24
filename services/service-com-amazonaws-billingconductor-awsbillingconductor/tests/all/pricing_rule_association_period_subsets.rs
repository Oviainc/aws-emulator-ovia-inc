// SPDX-License-Identifier: CC0-1.0

//! Paging a BILLING-PERIOD-scoped read of a plan's pricing rule associations, end to
//! end over the service's real `restJson1` wire.
//!
//! RULE: the page sequence of a period-scoped read walks exactly the associations in
//! force in that period — in ARN order, each once, at most `MaxResults` per page —
//! whatever the page size, so pagination cuts the period's subset rather than the
//! plan's whole history.
//!
//! The world under test is the staggered one
//! `tests/all/pricing_rule_association_billing_periods.rs` builds (one association per
//! start period, made by both writers), over the harness
//! `tests/all/pricing_rule_association_periods.rs` owns. The rows differ along the
//! period read (a strict subset of the associations, then all of them) and the page
//! size (the modelled minimum, a size that does not divide the subset, the subset's own
//! size, and the modelled maximum).

use serde_json::{json, Value};

use crate::pricing_rule_association_billing_periods::{
    in_force, plan_with_staggered_associations, RULES_FOR_PLAN, START_PERIODS,
};
use crate::pricing_rule_association_periods::{arns, Emulator};

/// How many pages a walk may take before the test calls the pagination broken: one page
/// per association plus the empty last page a token-terminated walk may end on. A server
/// that hands back a `NextToken` for ever is a defect to REPORT rather than a loop to
/// hang in.
const MAX_PAGES: usize = START_PERIODS.len() + 1;

/// Every rule `ListPricingRulesAssociatedToPricingPlan` reports for `period` when the
/// caller walks pages of `max_results`, in page order.
async fn paged_rules_for_plan(
    em: &Emulator,
    case: &str,
    plan: &str,
    period: &str,
    max_results: i64,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page_number in 1..=MAX_PAGES {
        let mut request =
            json!({"PricingPlanArn": plan, "BillingPeriod": period, "MaxResults": max_results});
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = em.post(RULES_FOR_PLAN, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: page {page_number} expected 200, got {status} with {body:?}"
        );
        let page = arns(&body, "PricingRuleArns");
        assert!(
            i64::try_from(page.len()).expect("a page is small") <= max_results,
            "case {case}: page {page_number} must hold at most {max_results} elements, \
             got {page:?}"
        );
        seen.extend(page);
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return seen;
        }
    }
    panic!(
        "case {case}: the walk handed back a NextToken after {MAX_PAGES} pages, having \
         reported {seen:?}"
    );
}

#[tokio::test]
async fn paging_a_period_scoped_read_walks_exactly_that_period_subset() {
    let em = Emulator::new();
    let (plan, rules) = plan_with_staggered_associations(&em).await;

    for period in ["2023-12", "2024-02"] {
        let expected = in_force(&rules, period);
        for max_results in [1, 3, 4, 100] {
            let case = format!("read at {period:?} with MaxResults={max_results}");
            let walked = paged_rules_for_plan(&em, &case, &plan, period, max_results).await;
            assert_eq!(
                walked, expected,
                "case {case}: the pages must report the period's subset in ARN order exactly \
                 once, expected {expected:?}, got {walked:?}"
            );
        }
    }
}

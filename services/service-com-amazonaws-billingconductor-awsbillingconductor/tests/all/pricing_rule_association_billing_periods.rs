// SPDX-License-Identifier: CC0-1.0

//! A plan whose pricing rule associations started in DIFFERENT billing periods, read
//! end to end over the service's real `restJson1` wire.
//!
//! RULE: a read scoped to a billing period reports exactly the SUBSET of a plan's
//! associations whose start period is at or before it, and the whole surface agrees on
//! that subset: the two inverse listings
//! (`ListPricingRulesAssociatedToPricingPlan` / `ListPricingPlansAssociatedWithPricingRule`),
//! the counts derived from them (`PricingPlanListElement.Size`,
//! `PricingRuleListElement.AssociatedPricingPlanCount`) and the pages a `MaxResults`
//! read walks (`tests/all/pricing_rule_association_period_subsets.rs`). A period in
//! which no association is in force reports the list member not at all, because both
//! listings model it `@length(min: 1)`.
//!
//! `tests/all/pricing_rule_association_periods.rs` owns the rule for ONE association
//! against the period it started in, and owns the harness both suites drive; this
//! module is only about the SUBSETS a plan holding several associations with different
//! start periods reports. Rows differ along how many associations are in force (0, 1,
//! 2, 3, 4) and along the start months, which cross a year boundary and use both
//! spellings the modelled `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` allows.

use serde_json::json;

use crate::pricing_rule_association_periods::{arns, period_months, Emulator};

/// `ListPricingPlansAssociatedWithPricingRule` — the plans one rule is associated with.
const PLANS_FOR_RULE: &str = "/list-pricing-plans-associated-with-pricing-rule";

/// `ListPricingRulesAssociatedToPricingPlan` — the inverse read.
pub(crate) const RULES_FOR_PLAN: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// The billing periods the four associations of the table start in, in the order the
/// rules are created: two months of one year and two of the next, so the subsets cross
/// a year boundary, in the one- and the two-digit month form the modelled pattern both
/// allows.
pub(crate) const START_PERIODS: [&str; 4] = ["2023-11", "2023-12", "2024-1", "2024-02"];

/// A plan associating one rule per entry of [`START_PERIODS`], each association started
/// in that period: the plan ARN and the rule ARNs in the order the periods name them.
///
/// Half the associations are made by `CreatePricingPlan` and half by
/// `AssociatePricingRules`, so both writers' records are in the set under test.
pub(crate) async fn plan_with_staggered_associations(em: &Emulator) -> (String, Vec<String>) {
    let mut rules: Vec<String> = Vec::new();
    for index in 0..START_PERIODS.len() {
        rules.push(em.create_rule(&format!("Staggered-Rule-{index}")).await);
    }
    let split = rules.len() / 2;
    let plan = em.create_plan("staggered-plan", &rules[..split]).await;
    em.associate(&plan, &rules[split..]).await;
    for (rule, period) in rules.iter().zip(START_PERIODS) {
        em.start_association_in(&plan, rule, period);
    }
    (plan, rules)
}

/// The rules of `rules` whose association started at or before `period`, in ARN order —
/// the subset in force then.
pub(crate) fn in_force(rules: &[String], period: &str) -> Vec<String> {
    let mut selected: Vec<String> = rules
        .iter()
        .zip(START_PERIODS)
        .filter(|(_, start)| period_months(start) <= period_months(period))
        .map(|(rule, _)| rule.clone())
        .collect();
    selected.sort();
    selected
}

/// `ListPricingRulesAssociatedToPricingPlan` for one period: the reported rule ARNs,
/// and whether the response carried the member at all.
async fn rules_for_plan(
    em: &Emulator,
    case: &str,
    plan: &str,
    period: &str,
) -> (Vec<String>, bool) {
    let (status, body) = em
        .post(
            RULES_FOR_PLAN,
            &json!({"PricingPlanArn": plan, "BillingPeriod": period}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case}: ListPricingRulesAssociatedToPricingPlan expected 200, got {status} \
         with {body:?}"
    );
    (
        arns(&body, "PricingRuleArns"),
        body.get("PricingRuleArns").is_some(),
    )
}

/// `ListPricingPlansAssociatedWithPricingRule` for one period: the reported plan ARNs,
/// and whether the response carried the member at all.
async fn plans_for_rule(
    em: &Emulator,
    case: &str,
    rule: &str,
    period: &str,
) -> (Vec<String>, bool) {
    let (status, body) = em
        .post(
            PLANS_FOR_RULE,
            &json!({"PricingRuleArn": rule, "BillingPeriod": period}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case}: ListPricingPlansAssociatedWithPricingRule expected 200, got {status} \
         with {body:?}"
    );
    (
        arns(&body, "PricingPlanArns"),
        body.get("PricingPlanArns").is_some(),
    )
}

/// The member a response carries, for the message of a presence assertion.
fn presence(body_member: bool) -> &'static str {
    if body_member {
        "present"
    } else {
        "absent"
    }
}

/// RULE: a period-scoped read reports exactly the associations whose start period is at
/// or before it — nothing in an earlier period, the whole set once every start is
/// passed — and the list member is left out entirely in a period that holds none.
#[tokio::test]
async fn each_period_reports_exactly_the_associations_already_in_force() {
    let em = Emulator::new();
    let (plan, rules) = plan_with_staggered_associations(&em).await;

    // Rows differ along how many of the four associations have started: none, one, two,
    // three, all four, and all four a year later; the months also cross the year
    // boundary the start periods straddle.
    for period in [
        "2023-10", "2023-11", "2023-12", "2024-01", "2024-2", "2025-02",
    ] {
        let case = format!("read at {period:?}");
        let expected = in_force(&rules, period);
        let wanted_member = !expected.is_empty();

        let (reported, member_present) = rules_for_plan(&em, &case, &plan, period).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected the associations in force then {expected:?}, got {reported:?}"
        );
        assert_eq!(
            member_present,
            wanted_member,
            "case {case}: PricingRuleArns is modelled @length(min: 1), so it is reported \
             exactly when an association is in force; expected {}, got {}",
            presence(wanted_member),
            presence(member_present)
        );

        // The inverse index is derived from the same associations, so it agrees rule by
        // rule, and the two derived counts agree with both.
        for rule in &rules {
            let rule_case = format!("{case}, rule {rule:?}");
            let wanted = expected.contains(rule);
            let (plans, member_present) = plans_for_rule(&em, &rule_case, rule, period).await;
            let expected_plans: Vec<String> = if wanted { vec![plan.clone()] } else { vec![] };
            assert_eq!(
                plans, expected_plans,
                "case {rule_case}: the inverse read must agree, expected {expected_plans:?}, \
                 got {plans:?}"
            );
            assert_eq!(
                member_present,
                wanted,
                "case {rule_case}: PricingPlanArns is reported exactly when the association is \
                 in force; expected {}, got {}",
                presence(wanted),
                presence(member_present)
            );
            let count = em.rule_plan_count(&rule_case, rule, Some(period)).await;
            assert_eq!(
                count,
                i64::from(wanted),
                "case {rule_case}: AssociatedPricingPlanCount counts the plans associating the \
                 rule in that period, expected {}, got {count}",
                i64::from(wanted)
            );
        }

        let size = em.plan_size(&case, &plan, Some(period)).await;
        let expected_size = i64::try_from(expected.len()).expect("a small count fits in i64");
        assert_eq!(
            size, expected_size,
            "case {case}: Size counts the associations in force in that period, expected \
             {expected_size}, got {size}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `DisassociatePricingRules` end to end over the service's real `restJson1` wire
//! through [`MultiServiceRouter::dispatch`] — the same path the production router
//! serves.
//!
//! RULE: `DisassociatePricingRules` ends, from the billing period the call is made in
//! onward, exactly the pricing rule associations the request names — each rule in
//! either spelling the modelled `PricingRuleArn` pattern allows — on the pricing plan
//! it addresses, leaves every other association of that plan in force, and answers with
//! that plan's full ARN; the periods an ended association WAS in force in still report
//! it, because every read of an association is scoped to a billing period. It refuses,
//! as a whole and with nothing changed, a request that addresses a plan this account
//! does not hold (the modelled 404), a plan Amazon Web Services owns, a rule that
//! exists nowhere, a rule the plan does not associate, or a rule named twice.
//!
//! Each table's rows differ along what the rule generalises over: how many rules the
//! plan holds and how many of them one call ends (one of one, one of three, two of
//! three, all of three, and the modelled per-call maximum of thirty), the spelling each
//! rule is named with (bare id, full ARN, both in one call), the order they are named
//! in, the spelling the plan is addressed by, and — for the period dimension — how far
//! back the ended association had started.
//!
//! The reads that prove what was persisted are the service's own:
//! `ListPricingRulesAssociatedToPricingPlan` (the plan's side),
//! `ListPricingPlansAssociatedWithPricingRule` (the rule's side), `ListPricingPlans`
//! (the derived `Size`) and `DeletePricingRule`, which a rule no plan associates any
//! more is finally allowed to pass. The harness is the one
//! `tests/all/pricing_rule_association_periods.rs` owns, so the dispatch, signing and
//! backdating boilerplate exists once.

use serde_json::{json, Value};

use crate::pricing_rule_association_periods::{arns, shift_period, Emulator};

/// `PUT /disassociate-pricing-rules` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation", operation="DisassociatePricingRules")`).
const DISASSOCIATE_PATH: &str = "/disassociate-pricing-rules";

/// The plan-side reader of an association.
const LIST_RULES_OF_PLAN: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// The rule-side reader of an association.
const LIST_PLANS_OF_RULE: &str = "/list-pricing-plans-associated-with-pricing-rule";

/// The modelled maximum length of `DisassociatePricingRulesInput.PricingRuleArns`
/// (`PricingRuleArnsNonEmptyInput`, len 1..30), which is also the most rules
/// `CreatePricingPlanInput.PricingRuleArns` can name.
const MAX_RULES_PER_CALL: usize = 30;

/// The AWS-managed pricing plans the modelled `PricingPlanArn` pattern enumerates
/// beside a customer-minted id
/// (`^(arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/)?(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})$`),
/// so neither is a plan whose associations a caller may change.
const AWS_MANAGED_PLANS: [&str; 2] = ["BasicPricingPlan", "Passthrough"];

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The bare resource id of a full ARN (the segment after the `/`).
fn bare_id(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .expect("a full ARN ends in its resource id")
        .to_string()
}

/// `arns` sorted into the ARN order every reader of an association reports.
fn in_arn_order(values: &[String]) -> Vec<String> {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted
}

/// The billing period the service calls current, read from the same clock it answers
/// from so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// How a row names a resource: by the bare id the modelled pattern allows, or by the
/// full ARN. Both spell ONE resource.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    Bare,
    Full,
}

impl Spelling {
    /// `arn` as this spelling names it.
    fn of(self, arn: &str) -> String {
        match self {
            Self::Bare => bare_id(arn),
            Self::Full => arn.to_string(),
        }
    }
}

/// `DisassociatePricingRules` for one plan: the status and the parsed body.
async fn disassociate(em: &Emulator, plan: &str, rules: &[String]) -> (u16, Value) {
    em.put(
        DISASSOCIATE_PATH,
        &json!({"Arn": plan, "PricingRuleArns": rules}),
    )
    .await
}

/// The rules the plan associates in `period` (`None` names no period, i.e. the current
/// one), as the service reports them.
async fn rules_of(em: &Emulator, case: &str, plan: &str, period: Option<&str>) -> Vec<String> {
    let mut body = json!({"PricingPlanArn": plan});
    if let Some(period) = period {
        body["BillingPeriod"] = json!(period);
    }
    let (status, answered) = em.post(LIST_RULES_OF_PLAN, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPricingRulesAssociatedToPricingPlan expected 200, got {status} \
         with {answered:?}"
    );
    arns(&answered, "PricingRuleArns")
}

/// The plans the rule is associated with in `period`, as the service reports them.
async fn plans_of(em: &Emulator, case: &str, rule: &str, period: Option<&str>) -> Vec<String> {
    let mut body = json!({"PricingRuleArn": rule});
    if let Some(period) = period {
        body["BillingPeriod"] = json!(period);
    }
    let (status, answered) = em.post(LIST_PLANS_OF_RULE, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPricingPlansAssociatedWithPricingRule expected 200, got {status} \
         with {answered:?}"
    );
    arns(&answered, "PricingPlanArns")
}

/// A plan holding `held` freshly created rules, as (plan ARN, rule ARNs in ARN order).
async fn plan_holding(em: &Emulator, tag: &str, held: usize) -> (String, Vec<String>) {
    let mut rules = Vec::with_capacity(held);
    for index in 0..held {
        rules.push(em.create_rule(&format!("Rule-{tag}-{index}")).await);
    }
    let plan = em.create_plan(&format!("plan-{tag}"), &rules).await;
    (plan, in_arn_order(&rules))
}

/// One row of the disassociation table.
struct Row {
    case: &'static str,
    /// How many rules the plan holds before the call.
    held: usize,
    /// Which of them, by position in ARN order, the call ends.
    ended: Vec<usize>,
    /// How each ended rule is named — one per rule, cycled when shorter.
    spellings: Vec<Spelling>,
    /// Whether the call names them in ARN order or reversed.
    reversed: bool,
    /// How the call addresses the plan.
    plan_spelling: Spelling,
}

/// The table: rows differ along how many of a plan's associations one call ends, which
/// of them, the spelling each is named with, the order they are named in, and the
/// spelling the plan is addressed by.
fn disassociation_table() -> Vec<Row> {
    vec![
        Row {
            case: "the only rule, named by its full ARN, plan by its full ARN",
            held: 1,
            ended: vec![0],
            spellings: vec![Spelling::Full],
            reversed: false,
            plan_spelling: Spelling::Full,
        },
        Row {
            case: "the only rule, named by its bare id, plan by its bare id",
            held: 1,
            ended: vec![0],
            spellings: vec![Spelling::Bare],
            reversed: false,
            plan_spelling: Spelling::Bare,
        },
        Row {
            case: "the middle one of three, by bare id",
            held: 3,
            ended: vec![1],
            spellings: vec![Spelling::Bare],
            reversed: false,
            plan_spelling: Spelling::Full,
        },
        Row {
            case: "two of three in mixed spellings, named in reverse ARN order",
            held: 3,
            ended: vec![0, 2],
            spellings: vec![Spelling::Bare, Spelling::Full],
            reversed: true,
            plan_spelling: Spelling::Bare,
        },
        Row {
            case: "all three of three",
            held: 3,
            ended: vec![0, 1, 2],
            spellings: vec![Spelling::Full],
            reversed: false,
            plan_spelling: Spelling::Full,
        },
        Row {
            case: "the modelled per-call maximum of thirty rules",
            held: MAX_RULES_PER_CALL,
            ended: (0..MAX_RULES_PER_CALL).collect(),
            spellings: vec![Spelling::Bare, Spelling::Full],
            reversed: false,
            plan_spelling: Spelling::Full,
        },
    ]
}

#[tokio::test]
async fn a_disassociation_ends_exactly_the_named_associations_and_leaves_the_rest_in_force() {
    let em = Emulator::new();
    for (index, row) in disassociation_table().into_iter().enumerate() {
        let case = row.case;
        let (plan, held) = plan_holding(&em, &format!("ends-{index}"), row.held).await;
        let ended: Vec<String> = row.ended.iter().map(|at| held[*at].clone()).collect();
        let kept: Vec<String> = held
            .iter()
            .filter(|rule| !ended.contains(rule))
            .cloned()
            .collect();

        let mut named: Vec<String> = ended
            .iter()
            .enumerate()
            .map(|(at, arn)| row.spellings[at % row.spellings.len()].of(arn))
            .collect();
        if row.reversed {
            named.reverse();
        }

        let (status, body) = disassociate(&em, &row.plan_spelling.of(&plan), &named).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body:?}"
        );
        // "The Amazon Resource Name (ARN) of the pricing plan that the pricing rules
        // successfully disassociated from" — the plan's FULL ARN, whichever spelling
        // the request addressed it by.
        assert_eq!(
            body["Arn"].as_str(),
            Some(plan.as_str()),
            "case {case:?}: the answer must name the plan's full ARN, got {body:?}"
        );

        // The plan's side: exactly the associations the request did not name.
        let reported = rules_of(&em, case, &plan, None).await;
        assert_eq!(
            reported, kept,
            "case {case:?}: expected the plan to associate {kept:?}, got {reported:?}"
        );

        // The rule's side of the same associations, and the derived count.
        for rule in &ended {
            let holders = plans_of(&em, case, rule, None).await;
            assert_eq!(
                holders,
                Vec::<String>::new(),
                "case {case:?}: expected rule {rule:?} to be associated with no plan, \
                 got {holders:?}"
            );
        }
        for rule in &kept {
            let holders = plans_of(&em, case, rule, None).await;
            assert_eq!(
                holders,
                vec![plan.clone()],
                "case {case:?}: expected rule {rule:?} to still report plan {plan:?}, \
                 got {holders:?}"
            );
        }

        // `Size`: "The pricing rules count that's currently associated with this
        // pricing plan list element."
        let size = em.plan_size(case, &plan, None).await;
        let expected = i64::try_from(kept.len()).expect("a small count fits in i64");
        assert_eq!(
            size, expected,
            "case {case:?}: expected Size {expected}, got {size}"
        );
    }
}

#[tokio::test]
async fn a_rule_no_plan_associates_any_more_can_be_deleted_and_associated_again() {
    let em = Emulator::new();
    // Rows differ along how many of the plan's rules are released at once, and whether
    // the plan keeps an association of its own afterwards.
    for (index, (case, held, ended)) in [
        ("the only association of the plan", 1usize, 1usize),
        ("one of two associations", 2, 1),
        ("both associations at once", 2, 2),
    ]
    .into_iter()
    .enumerate()
    {
        let (plan, rules) = plan_holding(&em, &format!("reuse-{index}"), held).await;
        let released = &rules[..ended];

        // While the plan associates it, a rule cannot be deleted: that is the conflict
        // `DeletePricingRule` answers with
        // (`PRICING_RULE_ATTACHED_TO_PRICING_PLAN_DELETE_CONFLICT`), so the delete
        // below proves the association really ended rather than only stopped being
        // listed.
        let (status, body) = em
            .post("/delete-pricing-rule", &json!({"Arn": rules[0]}))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: an associated rule must not be deletable, got {status} with {body:?}"
        );

        let (status, body) = disassociate(&em, &plan, released).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body:?}"
        );

        // Re-associating a rule whose association ended is allowed again, and a later
        // read reports it.
        em.associate(&plan, released).await;
        let reported = rules_of(&em, case, &plan, None).await;
        assert_eq!(
            reported, rules,
            "case {case:?}: expected the re-associated plan to hold {rules:?}, got {reported:?}"
        );

        // And once it is released for good, the rule is deletable.
        let (status, body) = disassociate(&em, &plan, released).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second disassociation to be served, got {status} \
             with {body:?}"
        );
        for rule in released {
            let (status, body) = em
                .post("/delete-pricing-rule", &json!({"Arn": rule}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a released rule {rule:?} must be deletable, got {status} \
                 with {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_ended_association_is_still_reported_for_the_periods_it_was_in_force_in() {
    let em = Emulator::new();
    // Rows differ along how far back the ended association had started: the previous
    // billing period, two periods back, and a year and a month back — so the periods
    // the assertion walks cross a year boundary.
    for (index, months_back) in [1i64, 2, 13].into_iter().enumerate() {
        let now = current_period();
        let start = shift_period(&now, -months_back);
        let case = format!("an association started in {start:?}, ended in {now:?}");

        let rule = em.create_rule(&format!("Past-Rule-{index}")).await;
        let plan = em
            .create_plan(&format!("past-plan-{index}"), std::slice::from_ref(&rule))
            .await;
        // The wire cannot make an association in a PAST period — an associating call
        // happens now — so the stored start period is backdated, exactly as the
        // period-scoped suites of this crate set such a world up.
        em.start_association_in(&plan, &rule, &start);

        let (status, body) = disassociate(&em, &plan, std::slice::from_ref(&rule)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body:?}"
        );

        // The association is in force in every period from the one it started in up to,
        // but not including, the one it ended in — and in none from that period on.
        let mut periods: Vec<(String, bool)> = (0..=months_back)
            .map(|back| {
                let period = shift_period(&now, -back);
                let in_force = back > 0;
                (period, in_force)
            })
            .collect();
        periods.push((shift_period(&now, 1), false));
        periods.push((shift_period(&start, -1), false));

        for (period, in_force) in periods {
            let case = format!("{case} read at {period:?}");
            let expected: Vec<String> = if in_force {
                vec![rule.clone()]
            } else {
                vec![]
            };
            let reported = rules_of(&em, &case, &plan, Some(&period)).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );

            // The inverse index and the derived count are taken from the same
            // association, so they agree period for period.
            let expected_plans: Vec<String> = if in_force {
                vec![plan.clone()]
            } else {
                vec![]
            };
            let inverse = plans_of(&em, &case, &rule, Some(&period)).await;
            assert_eq!(
                inverse, expected_plans,
                "case {case:?}: the inverse index must agree, expected {expected_plans:?}, \
                 got {inverse:?}"
            );
            let size = em.plan_size(&case, &plan, Some(&period)).await;
            let expected_size = i64::try_from(expected.len()).expect("a small count fits in i64");
            assert_eq!(
                size, expected_size,
                "case {case:?}: Size must count the associations in force in the period, \
                 expected {expected_size}, got {size}"
            );
        }
    }
}

#[tokio::test]
async fn a_plan_this_account_does_not_hold_is_refused_with_the_modelled_not_found() {
    let em = Emulator::new();
    let rule = em.create_rule("Missing-Plan-Rule").await;
    // Rows differ along the spelling the plan that exists nowhere is addressed by.
    for (case, addressed) in [
        (
            "a full plan ARN naming no stored plan",
            "arn:aws:billingconductor::000000000000:pricingplan/NOSUCHPLAN".to_string(),
        ),
        ("a bare plan id naming no stored plan", "NOSUCHPLN2".to_string()),
    ] {
        let (status, body) = disassociate(&em, &addressed, std::slice::from_ref(&rule)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("pricingplan"),
            "case {case:?}: the refusal must name the resource type, got {body:?}"
        );
    }
}

#[tokio::test]
async fn the_associations_of_an_aws_managed_plan_are_not_the_callers_to_end() {
    let em = Emulator::new();
    let rule = em.create_rule("Managed-Plan-Rule").await;
    // Rows differ along which AWS-managed plan is addressed and the spelling it is
    // addressed by: the bare id, and the full ARN with the literal `aws` account
    // segment the modelled pattern allows for them.
    for managed in AWS_MANAGED_PLANS {
        for addressed in [
            managed.to_string(),
            format!("arn:aws:billingconductor::aws:pricingplan/{managed}"),
        ] {
            let case = format!("the AWS-managed plan addressed as {addressed:?}");
            let (status, body) = disassociate(&em, &addressed, std::slice::from_ref(&rule)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
            assert_eq!(
                body["Reason"].as_str(),
                Some("MISMATCHED_PRICINGPLAN_ARN"),
                "case {case:?}: expected the modelled reason, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_rule_the_plan_does_not_associate_is_refused_and_nothing_changes() {
    let em = Emulator::new();
    // Rows differ along WHY the named rule is not one of the plan's associations — it
    // exists nowhere, it exists but was never associated, it was associated with
    // another plan, or its association has already ended — along the spelling it is
    // named with, and along whether the request also names a rule the plan really does
    // associate (which must not be ended either: the request is refused as a whole).
    for (index, (case, kind, spelling, with_an_associated_rule)) in [
        (
            "a rule that exists nowhere, by full ARN",
            Absent::Nonexistent,
            Spelling::Full,
            false,
        ),
        (
            "a rule that exists nowhere, by bare id, beside an associated one",
            Absent::Nonexistent,
            Spelling::Bare,
            true,
        ),
        (
            "a rule that exists but was never associated",
            Absent::NeverAssociated,
            Spelling::Full,
            false,
        ),
        (
            "a rule associated with another plan, by bare id",
            Absent::AnotherPlan,
            Spelling::Bare,
            false,
        ),
        (
            "a rule whose association this plan already ended, beside an associated one",
            Absent::AlreadyEnded,
            Spelling::Full,
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (plan, held) = plan_holding(&em, &format!("absent-{index}"), 1).await;
        let named_rule = match kind {
            Absent::Nonexistent => "NOSUCHRUL1".to_string(),
            Absent::NeverAssociated => em.create_rule(&format!("Loose-Rule-{index}")).await,
            Absent::AnotherPlan => {
                let (_, other) = plan_holding(&em, &format!("other-{index}"), 1).await;
                other[0].clone()
            }
            Absent::AlreadyEnded => {
                let extra = em.create_rule(&format!("Ended-Rule-{index}")).await;
                em.associate(&plan, std::slice::from_ref(&extra)).await;
                let (status, body) = disassociate(&em, &plan, std::slice::from_ref(&extra)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: setup disassociation expected 200, got {status} with {body:?}"
                );
                extra
            }
        };

        let mut named = vec![spelling.of(&named_rule)];
        if with_an_associated_rule {
            named.push(held[0].clone());
        }
        let (status, body) = disassociate(&em, &plan, &named).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        // A rule that exists nowhere is refused for not existing; one that exists but
        // is not among the plan's associations, for not being associated — the two
        // reasons the model enumerates for these inputs.
        let expected_reason = match kind {
            Absent::Nonexistent => "PRICINGRULES_NOT_EXIST",
            _ => "PRICINGRULES_NOT_ASSOCIATED",
        };
        assert_eq!(
            body["Reason"].as_str(),
            Some(expected_reason),
            "case {case:?}: expected reason {expected_reason:?}, got {body:?}"
        );

        // Nothing was applied: the plan still associates exactly what it held.
        let reported = rules_of(&em, case, &plan, None).await;
        assert_eq!(
            reported, held,
            "case {case:?}: the refused request must leave the plan alone, got {reported:?}"
        );
    }
}

/// Why a rule a request names is not one of the addressed plan's associations.#[derive(Debug, Clone, Copy)]
enum Absent {
    /// No such pricing rule is stored at all.
    Nonexistent,
    /// The rule exists and no plan ever associated it.
    NeverAssociated,
    /// The rule exists and ANOTHER plan associates it.
    AnotherPlan,
    /// This plan associated the rule and that association has already ended.
    AlreadyEnded,
}

#[tokio::test]
async fn one_rule_named_twice_in_a_request_is_refused_and_nothing_changes() {
    let em = Emulator::new();
    // Rows differ along the pair of spellings the one rule is named by: a rule is the
    // rule, not the spelling, so two spellings of it in one request are a duplicate
    // too.
    for (index, (case, first, second)) in [
        ("the same bare id twice", Spelling::Bare, Spelling::Bare),
        ("the same full ARN twice", Spelling::Full, Spelling::Full),
        (
            "a bare id and the full ARN of one rule",
            Spelling::Bare,
            Spelling::Full,
        ),
        (
            "a full ARN and the bare id of one rule",
            Spelling::Full,
            Spelling::Bare,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (plan, held) = plan_holding(&em, &format!("twice-{index}"), 1).await;
        let (status, body) =
            disassociate(&em, &plan, &[first.of(&held[0]), second.of(&held[0])]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("DUPLICATE_PRICINGRULE_ARNS"),
            "case {case:?}: expected the modelled duplicate reason, got {body:?}"
        );
        let reported = rules_of(&em, case, &plan, None).await;
        assert_eq!(
            reported, held,
            "case {case:?}: the refused request must change nothing, got {reported:?}"
        );
    }
}

/// THE INVARIANT the kept history is bounded by: ending an association keeps it
/// readable for the periods it was in force in, but the record a plan carries cannot
/// grow without limit, and what is dropped to stay inside the bound is always the
/// OLDEST ENDED association — never one still in force.
///
/// Driven against the state record rather than the wire, because the number of ended
/// associations the bound allows is far more than a test can make through billing
/// periods the clock has to reach.
#[test]
fn the_kept_association_history_is_bounded_and_never_evicts_one_in_force() {
    use service_com_amazonaws_billingconductor_awsbillingconductor::state::{
        pricing_rule_arn, PricingPlanRecord, PricingRuleAssociationRecord,
        MAX_ENDED_PRICING_RULE_ASSOCIATIONS_PER_PRICING_PLAN as MAX_ENDED,
    };

    /// A rule id of the `[a-zA-Z0-9]{10}` shape the modelled pattern allows.
    fn rule_id(index: usize) -> String {
        format!("R{index:09}")
    }

    // Rows differ along how far past the bound the ended history is pushed: exactly at
    // it, one over, and many over.
    for ended in [MAX_ENDED, MAX_ENDED + 1, MAX_ENDED + 10] {
        let case = format!("{ended} ended associations against a bound of {MAX_ENDED}");
        let mut plan = PricingPlanRecord::default();

        // One association that still stands, started before all the others, so the
        // eviction has an in-force record it could wrongly take as the oldest.
        let live = pricing_rule_arn("LIVERULE01");
        plan.pricing_rule_associations
            .push(PricingRuleAssociationRecord {
                arn: live.clone(),
                inclusive_start_billing_period: "2000-01".to_string(),
                exclusive_end_billing_period: None,
            });

        // Then one association per period that starts in one month and ends in the
        // next, oldest first — each in force in exactly the period it started in.
        for index in 0..ended {
            let start = shift_period("2000-02", i64::try_from(index).expect("a small count"));
            let arn = pricing_rule_arn(&rule_id(index));
            plan.pricing_rule_associations
                .push(PricingRuleAssociationRecord {
                    arn: arn.clone(),
                    inclusive_start_billing_period: start.clone(),
                    exclusive_end_billing_period: None,
                });
            plan.end_rule_associations(
                std::slice::from_ref(&arn),
                &shift_period(&start, 1),
            );
        }

        let kept: Vec<&PricingRuleAssociationRecord> = plan
            .pricing_rule_associations
            .iter()
            .filter(|association| association.exclusive_end_billing_period.is_some())
            .collect();
        assert_eq!(
            kept.len(),
            MAX_ENDED,
            "case {case:?}: expected the ended history to be capped at {MAX_ENDED}, got {}",
            kept.len()
        );

        // The association in force is untouched, whatever was evicted.
        let in_force = plan.associated_rule_arns("2000-01");
        assert_eq!(
            in_force,
            vec![live.clone()],
            "case {case:?}: the association in force must survive, got {in_force:?}"
        );

        // The evicted ones are the OLDEST: the newest ended association is still
        // reported for the period it was in force in, and the oldest beyond the bound
        // is reported nowhere.
        let newest = ended - 1;
        let newest_period = shift_period("2000-02", i64::try_from(newest).expect("a small count"));
        let reported = plan.associated_rule_arns(&newest_period);
        assert!(
            reported.contains(&pricing_rule_arn(&rule_id(newest))),
            "case {case:?}: the newest ended association must still be reported for \
             {newest_period:?}, got {reported:?}"
        );
        for evicted in 0..ended.saturating_sub(MAX_ENDED) {
            let period = shift_period("2000-02", i64::try_from(evicted).expect("a small count"));
            let reported = plan.associated_rule_arns(&period);
            assert!(
                !reported.contains(&pricing_rule_arn(&rule_id(evicted))),
                "case {case:?}: the evicted association {evicted} must be reported for no \
                 period, got {reported:?} at {period:?}"
            );
        }
    }
}

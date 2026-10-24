// SPDX-License-Identifier: CC0-1.0

//! The BILLING PERIOD dimension of a pricing rule / pricing plan association, end to
//! end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE: an association between a pricing rule and a pricing plan is in force from
//! the billing period it was made in onward, so every read scoped to a billing period
//! reports it for that period and every later one and for no earlier one. That covers
//! `ListPricingRulesAssociatedToPricingPlan` and its inverse index
//! `ListPricingPlansAssociatedWithPricingRule`, and the counts the two listings derive
//! from the same associations (`PricingPlanListElement.Size`,
//! `PricingRuleListElement.AssociatedPricingPlanCount`). A read that names no period
//! asks about the current one.
//!
//! Each table's rows differ along what the rule generalises over: the period an
//! association started in (the epoch month, a December, a January, a far-future
//! month, and both spellings of one month the modelled `BillingPeriod` pattern
//! `^\d{4}-(0?[1-9]|1[012])$` allows) and the period a read asks about (the month
//! before the start, the start itself, the month after, a year on either side, both
//! spellings of the start, and no period at all).
//!
//! The wire cannot make an association in a PAST period — an associating call happens
//! now — so a row makes its association through the real wire and then backdates the
//! stored start period, which is the one thing the test cannot ask the service for.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A `BillingPeriod` as a count of months, so two periods compare and shift.
fn period_months(value: &str) -> i64 {
    let (year, month) = value
        .split_once('-')
        .expect("a test billing period is spelled YYYY-MM");
    let year: i64 = year.parse().expect("a test billing period has a year");
    let month: i64 = month.parse().expect("a test billing period has a month");
    year * 12 + month - 1
}

/// The `BillingPeriod` a month count names, in the zero-padded spelling the modelled
/// pattern allows.
fn period_from_months(total: i64) -> String {
    format!(
        "{:04}-{:02}",
        total.div_euclid(12),
        total.rem_euclid(12) + 1
    )
}

/// The period `months` after `period` (negative for before), crossing year
/// boundaries.
fn shift_period(period: &str, months: i64) -> String {
    period_from_months(period_months(period) + months)
}

/// A live emulator holding only this service, with the engine kept so a row can
/// backdate an association the wire could only make now.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this payer-account-only
    /// control plane is reachable at all (see `tests/all/payer_account_gate.rs` for
    /// the denial every other caller gets).
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// POST a JSON body to one of this service's routes and return the status plus
    /// the parsed response body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        self.send("POST", path, body).await
    }

    /// Send a JSON body to one of this service's routes with the method its modelled
    /// HTTP binding names (`smithy_query(query="operation", operation="...")`:
    /// `AssociatePricingRules` is a `PUT`, the listings are `POST`).
    async fn send(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/billingconductor/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a `GLOBAL`/`MARKUP` pricing rule and answer the ARN the service minted.
    async fn create_rule(&self, name: &str) -> String {
        let (status, body) = self
            .post(
                "/create-pricing-rule",
                &json!({"Name": name, "Scope": "GLOBAL", "Type": "MARKUP",
                        "ModifierPercentage": 10.0}),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreatePricingRule {name:?}: expected 200, got {status} with {body:?}"
        );
        body["Arn"]
            .as_str()
            .expect("setup CreatePricingRule: no Arn in the response")
            .to_string()
    }

    /// Create a pricing plan associating `rules`, and answer the ARN the service
    /// minted.
    async fn create_plan(&self, name: &str, rules: &[String]) -> String {
        let (status, body) = self
            .post(
                "/create-pricing-plan",
                &json!({"Name": name, "PricingRuleArns": rules}),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {name:?}: expected 200, got {status} with {body:?}"
        );
        body["Arn"]
            .as_str()
            .expect("setup CreatePricingPlan: no Arn in the response")
            .to_string()
    }

    /// `AssociatePricingRules`: add `rules` to the plan's associations.
    async fn associate(&self, plan: &str, rules: &[String]) {
        let (status, body) = self
            .send(
                "PUT",
                "/associate-pricing-rules",
                &json!({"Arn": plan, "PricingRuleArns": rules}),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup AssociatePricingRules on {plan:?}: expected 200, got {status} with {body:?}"
        );
    }

    /// Backdate every association the plan holds to `period`, as if the associating
    /// call had been made in that billing period.
    fn backdate(&self, plan: &str, period: &str) {
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let record = state
            .pricing_plans
            .get_mut(plan)
            .expect("setup backdate: the plan was created above");
        for association in &mut record.pricing_rule_associations {
            association.inclusive_start_billing_period = period.to_string();
        }
    }

    /// The rule ARNs `ListPricingRulesAssociatedToPricingPlan` reports for `plan` in
    /// `period` (`None` names no period at all).
    async fn rules_for_plan(&self, case: &str, plan: &str, period: Option<&str>) -> Vec<String> {
        let mut body = json!({"PricingPlanArn": plan});
        if let Some(period) = period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, answer) = self
            .post("/list-pricing-rules-associated-to-pricing-plan", &body)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: ListPricingRulesAssociatedToPricingPlan expected 200, got {status} \
             with {answer:?}"
        );
        arns(&answer, "PricingRuleArns")
    }

    /// The plan ARNs `ListPricingPlansAssociatedWithPricingRule` reports for `rule`
    /// in `period`.
    async fn plans_for_rule(&self, case: &str, rule: &str, period: Option<&str>) -> Vec<String> {
        let mut body = json!({"PricingRuleArn": rule});
        if let Some(period) = period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, answer) = self
            .post("/list-pricing-plans-associated-with-pricing-rule", &body)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: ListPricingPlansAssociatedWithPricingRule expected 200, got {status} \
             with {answer:?}"
        );
        arns(&answer, "PricingPlanArns")
    }

    /// The `Size` `ListPricingPlans` reports for `plan` in `period`.
    async fn plan_size(&self, case: &str, plan: &str, period: Option<&str>) -> i64 {
        let mut body = json!({"Filters": {"Arns": [plan]}});
        if let Some(period) = period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, answer) = self.post("/list-pricing-plans", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: ListPricingPlans expected 200, got {status} with {answer:?}"
        );
        answer["PricingPlans"][0]["Size"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: ListPricingPlans reported no Size: {answer:?}"))
    }

    /// The `AssociatedPricingPlanCount` `ListPricingRules` reports for `rule` in
    /// `period`.
    async fn rule_plan_count(&self, case: &str, rule: &str, period: Option<&str>) -> i64 {
        let mut body = json!({"Filters": {"Arns": [rule]}});
        if let Some(period) = period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, answer) = self.post("/list-pricing-rules", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: ListPricingRules expected 200, got {status} with {answer:?}"
        );
        answer["PricingRules"][0]["AssociatedPricingPlanCount"]
            .as_i64()
            .unwrap_or_else(|| {
                panic!("case {case}: ListPricingRules reported no count: {answer:?}")
            })
    }
}

/// The ARN list a response reports under `member`, as owned strings (an absent
/// member is an empty set, which is how both listings report "none": the member is
/// modelled `@length(min: 1)`).
fn arns(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .map(|arns| {
            arns.iter()
                .map(|arn| arn.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The billing period the service calls current, read from the same clock it answers
/// from so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// The periods an association starting in `start` must and must not be reported for:
/// the start itself and later periods, against earlier ones.
fn read_periods(start: &str) -> Vec<(String, bool)> {
    vec![
        (shift_period(start, -13), false),
        (shift_period(start, -1), false),
        (start.to_string(), true),
        (shift_period(start, 1), true),
        (shift_period(start, 13), true),
    ]
}

/// The periods a row starts its association in. Rows differ along the month form the
/// modelled `BillingPeriod` pattern allows (`0?[1-9]`, so `2026-7` and `2026-07` are
/// ONE period), the end-of-year and start-of-year months whose neighbours cross a
/// year, the epoch month, and a far-future one.
const START_PERIODS: &[&str] = &["1970-01", "1999-12", "2026-1", "2026-07", "2030-3", "9980-12"];

#[tokio::test]
async fn an_association_is_reported_from_the_period_it_started_in_onward_and_not_before() {
    let em = Emulator::new();
    for (index, start) in START_PERIODS.iter().enumerate() {
        let rule = em.create_rule(&format!("Period-Rule-{index}")).await;
        let plan = em
            .create_plan(&format!("period-plan-{index}"), std::slice::from_ref(&rule))
            .await;
        em.backdate(&plan, start);

        for (period, in_force) in read_periods(start) {
            let case = format!("start {start:?} read at {period:?}");
            let expected: Vec<String> = if in_force {
                vec![rule.clone()]
            } else {
                vec![]
            };
            let reported = em.rules_for_plan(&case, &plan, Some(&period)).await;
            assert_eq!(
                reported, expected,
                "case {case}: expected {expected:?}, got {reported:?}"
            );

            // The inverse index is derived from the same association, so it agrees
            // period for period.
            let expected_plans: Vec<String> = if in_force {
                vec![plan.clone()]
            } else {
                vec![]
            };
            let inverse = em.plans_for_rule(&case, &rule, Some(&period)).await;
            assert_eq!(
                inverse, expected_plans,
                "case {case}: the inverse index must agree, expected {expected_plans:?}, \
                 got {inverse:?}"
            );
        }

        // Both spellings of ONE month name one period, so they answer the same.
        let (year, month) = start
            .split_once('-')
            .expect("a start period is spelled YYYY-MM");
        let month: u32 = month.parse().expect("a start period has a numeric month");
        for spelling in [format!("{year}-{month}"), format!("{year}-{month:02}")] {
            let case = format!("start {start:?} read at {spelling:?}");
            let reported = em.rules_for_plan(&case, &plan, Some(&spelling)).await;
            assert_eq!(
                reported,
                vec![rule.clone()],
                "case {case}: either spelling of the start period reports the association, \
                 got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_derived_association_counts_are_taken_for_the_period_the_read_names() {
    let em = Emulator::new();
    for (index, start) in START_PERIODS.iter().enumerate() {
        let rule = em.create_rule(&format!("Count-Rule-{index}")).await;
        let plan = em
            .create_plan(&format!("count-plan-{index}"), std::slice::from_ref(&rule))
            .await;
        em.backdate(&plan, start);

        for (period, in_force) in read_periods(start) {
            let case = format!("start {start:?} counted at {period:?}");
            let expected = i64::from(in_force);
            let size = em.plan_size(&case, &plan, Some(&period)).await;
            assert_eq!(
                size, expected,
                "case {case}: PricingPlanListElement.Size counts the associations in force in \
                 the period, expected {expected}, got {size}"
            );
            let count = em.rule_plan_count(&case, &rule, Some(&period)).await;
            assert_eq!(
                count, expected,
                "case {case}: AssociatedPricingPlanCount counts the plans associating the rule \
                 in the period, expected {expected}, got {count}"
            );
        }
    }
}

#[tokio::test]
async fn an_association_made_now_is_in_force_now_and_in_no_earlier_period() {
    let em = Emulator::new();
    let created_with = em.create_rule("Now-Created").await;
    let associated_later = em.create_rule("Now-Associated").await;
    let plan = em
        .create_plan("now-plan", std::slice::from_ref(&created_with))
        .await;
    em.associate(&plan, std::slice::from_ref(&associated_later))
        .await;

    // Both ways of making an association — the create that names the rule and the
    // associating call that adds one — start it in the period the call ran in.
    let mut both = vec![created_with.clone(), associated_later.clone()];
    both.sort();
    let now = current_period();

    // Rows differ along the period asked about: none at all (the current one), the
    // current period named explicitly, a far-future period, and periods before the
    // association was made.
    let rows: Vec<(String, Option<String>, Vec<String>)> = vec![
        ("no period at all".to_string(), None, both.clone()),
        (
            "the current period".to_string(),
            Some(now.clone()),
            both.clone(),
        ),
        (
            "a far-future period".to_string(),
            Some("9999-12".to_string()),
            both.clone(),
        ),
        (
            "the period before".to_string(),
            Some(shift_period(&now, -1)),
            vec![],
        ),
        (
            "the epoch month".to_string(),
            Some("1970-01".to_string()),
            vec![],
        ),
    ];

    for (case, period, expected) in rows {
        let reported = em.rules_for_plan(&case, &plan, period.as_deref()).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );
        let size = em.plan_size(&case, &plan, period.as_deref()).await;
        let expected_size = i64::try_from(expected.len()).expect("a small count fits in i64");
        assert_eq!(
            size, expected_size,
            "case {case}: Size must count the same associations the listing reports, \
             expected {expected_size}, got {size}"
        );
    }

    // The reads changed nothing: the associations still stand for the current period.
    let reported = em.rules_for_plan("after the reads", &plan, None).await;
    assert_eq!(
        reported, both,
        "case \"after the reads\": a read must not change what a later caller sees, got \
         {reported:?}"
    );
}

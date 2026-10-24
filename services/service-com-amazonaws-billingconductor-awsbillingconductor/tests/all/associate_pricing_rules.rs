// SPDX-License-Identifier: CC0-1.0

//! `AssociatePricingRules` end to end over the service's real `restJson1` wire
//! through [`MultiServiceRouter::dispatch`] — the same path the production router
//! serves.
//!
//! RULE: `AssociatePricingRules` adds exactly the pricing rules the request names —
//! each in either spelling the modelled `PricingRuleArn` pattern allows — to the
//! association set of the pricing plan it addresses, persisting them so a later read
//! reports each as its FULL rule ARN, and answers with that plan's full ARN; it
//! refuses, as a whole and with nothing stored, a request that addresses a plan this
//! account does not hold (the modelled 404), a plan Amazon Web Services owns, a rule
//! that exists nowhere, a rule the plan already associates, a rule named twice, or a
//! set that would take the plan past the per-plan maximum.
//!
//! Every table's rows differ along what the rule generalises over: how many rules one
//! call associates (1, 2, 3, and the modelled per-call maximum of 30), the spelling
//! each rule is named with (bare id, full ARN, both in one call), the order they are
//! named in, the spelling the plan is addressed by, and how many associations the plan
//! already holds.
//!
//! The reads that prove persistence are the service's own:
//! `ListPricingRulesAssociatedToPricingPlan` (the plan's side),
//! `ListPricingPlansAssociatedWithPricingRule` (the rule's side) and
//! `ListPricingPlans` (the derived `Size` and `LastModifiedTime`).

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `PUT /associate-pricing-rules` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation", operation="AssociatePricingRules")`).
const ASSOCIATE_PATH: &str = "/associate-pricing-rules";

/// The plan-side reader of an association.
const LIST_RULES_OF_PLAN: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// The rule-side reader of an association.
const LIST_PLANS_OF_RULE: &str = "/list-pricing-plans-associated-with-pricing-rule";

/// The pricing-plan list, which carries a plan's derived `Size` and its
/// `LastModifiedTime`.
const LIST_PLANS: &str = "/list-pricing-plans";

/// The modelled maximum length of `AssociatePricingRulesInput.PricingRuleArns`
/// (`PricingRuleArnsNonEmptyInput`, len 1..30).
const MAX_RULES_PER_CALL: usize = 30;

/// How many pricing rules one plan may associate
/// (`crate::state::MAX_PRICING_RULES_PER_PRICING_PLAN`).
const MAX_RULES_PER_PLAN: usize = 30;

/// The limit code the per-plan refusal carries.
const PLAN_LIMIT_CODE: &str = "PricingRulesPerPricingPlan";

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
    /// How many resources this harness has created, so every generated name is unique
    /// ("The names must be unique to each pricing rule / pricing plan").
    minted: std::cell::Cell<usize>,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — Billing Conductor's control plane is authorized
    /// for the payer account alone (see the `crate::service` module docs), so this is
    /// the world in which the writes these tests are about are reachable at all. The
    /// denial every other caller gets is asserted in `tests/all/payer_account_gate.rs`.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            minted: std::cell::Cell::new(0),
        }
    }

    /// Send a JSON body to one of this service's routes and return the status plus the
    /// parsed response body.
    async fn call(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
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

    /// A name no earlier resource of this harness carries.
    fn unique(&self, prefix: &str) -> String {
        let next = self.minted.get() + 1;
        self.minted.set(next);
        format!("{prefix}-{next}")
    }

    /// Create a pricing plan associating `rules`, and answer the ARN the service
    /// minted.
    async fn create_plan(&self, rules: &[String]) -> String {
        let name = self.unique("plan");
        let (status, body) = self
            .call(
                "POST",
                "/create-pricing-plan",
                &json!({ "Name": name, "PricingRuleArns": rules }),
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

    /// Create a pricing rule and answer the ARN the service minted.
    async fn create_rule(&self) -> String {
        let name = self.unique("rule");
        let (status, body) = self
            .call(
                "POST",
                "/create-pricing-rule",
                &json!({
                    "Name": name,
                    "Scope": "GLOBAL",
                    "Type": "MARKUP",
                    "ModifierPercentage": 1.5,
                }),
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

    /// `count` freshly created pricing rules, as their full ARNs.
    async fn create_rules(&self, count: usize) -> Vec<String> {
        let mut arns = Vec::with_capacity(count);
        for _ in 0..count {
            arns.push(self.create_rule().await);
        }
        arns
    }

    /// `AssociatePricingRules` for one plan.
    async fn associate(&self, plan: &str, rules: &[String]) -> (u16, Value) {
        self.call(
            "PUT",
            ASSOCIATE_PATH,
            &json!({ "Arn": plan, "PricingRuleArns": rules }),
        )
        .await
    }

    /// The rules a plan associates, as the service reports them (every page).
    async fn rules_of(&self, plan: &str) -> Vec<String> {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "PricingPlanArn": plan });
            if let Some(next) = &token {
                body["NextToken"] = json!(next);
            }
            let (status, answered) = self.call("POST", LIST_RULES_OF_PLAN, &body).await;
            assert_eq!(
                status, 200,
                "read ListPricingRulesAssociatedToPricingPlan {plan:?}: expected 200, \
                 got {status} with {answered:?}"
            );
            seen.extend(string_list(&answered, "PricingRuleArns"));
            token = answered
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                return seen;
            }
        }
    }

    /// The plans a rule is associated with, as the service reports them.
    async fn plans_of(&self, rule: &str) -> Vec<String> {
        let (status, body) = self
            .call("POST", LIST_PLANS_OF_RULE, &json!({ "PricingRuleArn": rule }))
            .await;
        assert_eq!(
            status, 200,
            "read ListPricingPlansAssociatedWithPricingRule {rule:?}: expected 200, \
             got {status} with {body:?}"
        );
        string_list(&body, "PricingPlanArns")
    }

    /// One plan as `ListPricingPlans` reports it.
    async fn plan_element(&self, plan: &str) -> Value {
        let (status, body) = self
            .call(
                "POST",
                LIST_PLANS,
                &json!({ "Filters": { "Arns": [plan] } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "read ListPricingPlans {plan:?}: expected 200, got {status} with {body:?}"
        );
        body["PricingPlans"]
            .as_array()
            .and_then(|plans| plans.first())
            .cloned()
            .unwrap_or_else(|| panic!("ListPricingPlans reported no plan for {plan:?}: {body:?}"))
    }
}

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

/// A string list member of a response, as owned strings (an absent member is empty).
fn string_list(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The emulator's account id — the account every ARN this service mints carries.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The full `PricingPlanArn` of a plan id held by `account_id`.
fn plan_arn_in(account_id: &str, id: &str) -> String {
    format!("arn:aws:billingconductor::{account_id}:pricingplan/{id}")
}

/// The full `PricingRuleArn` of a rule id held by `account_id`.
fn rule_arn_in(account_id: &str, id: &str) -> String {
    format!("arn:aws:billingconductor::{account_id}:pricingrule/{id}")
}

/// The bare resource id of a full ARN (the segment after the `/`).
fn bare_id(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .expect("a full ARN ends in its resource id")
        .to_string()
}

/// `arns` sorted into the ARN order every reader of an association reports.
fn in_arn_order(arns: &[String]) -> Vec<String> {
    let mut sorted = arns.to_vec();
    sorted.sort();
    sorted
}

/// How a row names a resource: by the bare id the modelled pattern allows, or by the
/// full ARN.
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

/// One row of the association table.
struct Row {
    case: &'static str,
    /// How many rules the call associates.
    rules: usize,
    /// How each rule is named — one per rule, cycled when shorter.
    rule_spellings: Vec<Spelling>,
    /// Whether the call names the rules in ARN order or reversed.
    reversed: bool,
    /// How the call addresses the plan.
    plan_spelling: Spelling,
}

/// The table: rows differ along the number of rules one call associates, the spelling
/// each is named with, the order they are named in, and the spelling the plan is
/// addressed by.
fn association_table() -> Vec<Row> {
    vec![
        Row {
            case: "one rule, named by its full ARN, plan by its full ARN",
            rules: 1,
            rule_spellings: vec![Spelling::Full],
            reversed: false,
            plan_spelling: Spelling::Full,
        },
        Row {
            case: "one rule, named by its bare id, plan by its bare id",
            rules: 1,
            rule_spellings: vec![Spelling::Bare],
            reversed: false,
            plan_spelling: Spelling::Bare,
        },
        Row {
            case: "two rules in mixed spellings",
            rules: 2,
            rule_spellings: vec![Spelling::Bare, Spelling::Full],
            reversed: false,
            plan_spelling: Spelling::Full,
        },
        Row {
            case: "three rules named in reverse ARN order",
            rules: 3,
            rule_spellings: vec![Spelling::Full],
            reversed: true,
            plan_spelling: Spelling::Bare,
        },
        Row {
            case: "three bare ids, plan by its full ARN",
            rules: 3,
            rule_spellings: vec![Spelling::Bare],
            reversed: false,
            plan_spelling: Spelling::Full,
        },
        Row {
            case: "the modelled per-call maximum of thirty rules",
            rules: MAX_RULES_PER_CALL,
            rule_spellings: vec![Spelling::Bare, Spelling::Full],
            reversed: false,
            plan_spelling: Spelling::Full,
        },
    ]
}

#[tokio::test]
async fn an_association_adds_exactly_the_named_rules_and_a_later_read_reports_them() {
    let em = Emulator::new();
    for row in association_table() {
        let plan = em.create_plan(&[]).await;
        let rules = em.create_rules(row.rules).await;
        let ordered = in_arn_order(&rules);

        let mut named: Vec<String> = ordered
            .iter()
            .enumerate()
            .map(|(index, arn)| row.rule_spellings[index % row.rule_spellings.len()].of(arn))
            .collect();
        if row.reversed {
            named.reverse();
        }

        let addressed = row.plan_spelling.of(&plan);
        let (status, body) = em.associate(&addressed, &named).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body:?}",
            row.case
        );
        // "The PricingPlanArn that the PricingRuleArns are associated with" — the
        // plan's full ARN, whichever spelling the request addressed it by.
        assert_eq!(
            body["Arn"].as_str(),
            Some(plan.as_str()),
            "case {:?}: the answer must name the plan's full ARN, got {body:?}",
            row.case
        );

        // The plan's side of the association: every named rule, as its full ARN.
        let reported = em.rules_of(&plan).await;
        assert_eq!(
            reported, ordered,
            "case {:?}: expected the plan to associate {ordered:?}, got {reported:?}",
            row.case
        );

        // The rule's side of the same association.
        for rule in &ordered {
            let holders = em.plans_of(rule).await;
            assert_eq!(
                holders,
                vec![plan.clone()],
                "case {:?}: expected rule {rule:?} to report plan {plan:?}, got {holders:?}",
                row.case
            );
        }

        // `Size`: "The pricing rules count that's currently associated with this
        // pricing plan list element."
        let element = em.plan_element(&plan).await;
        assert_eq!(
            element["Size"].as_i64(),
            Some(row.rules as i64),
            "case {:?}: expected Size {}, got {element:?}",
            row.case,
            row.rules
        );
        // The plan changed, so its `LastModifiedTime` is not behind its creation time.
        let created = element["CreationTime"].as_i64().unwrap_or_default();
        let modified = element["LastModifiedTime"].as_i64().unwrap_or_default();
        assert!(
            modified >= created && created > 0,
            "case {:?}: expected LastModifiedTime >= CreationTime > 0, got {element:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn associations_accumulate_on_top_of_what_the_plan_already_holds() {
    let em = Emulator::new();
    // Rows differ along the batch sizes the plan accumulates its set through, and
    // along whether the plan started out with an association of its own.
    for (case, initial, batches) in [
        ("from an empty plan, one rule at a time", 0usize, vec![1, 1, 1]),
        ("from an empty plan, growing batches", 0, vec![1, 2, 3]),
        ("on top of one association made at create time", 1, vec![2, 1]),
        ("on top of three associations made at create time", 3, vec![1, 4]),
    ] {
        let preexisting = em.create_rules(initial).await;
        let plan = em.create_plan(&preexisting).await;
        let mut expected = preexisting.clone();
        for (index, size) in batches.iter().enumerate() {
            let batch = em.create_rules(*size).await;
            let (status, body) = em.associate(&plan, &batch).await;
            assert_eq!(
                status, 200,
                "case {case:?}: batch {index}: expected 200, got {status} with {body:?}"
            );
            expected.extend(batch);
            let reported = em.rules_of(&plan).await;
            assert_eq!(
                reported,
                in_arn_order(&expected),
                "case {case:?}: batch {index}: expected {:?}, got {reported:?}",
                in_arn_order(&expected)
            );
        }
    }
}

#[tokio::test]
async fn a_rule_the_plan_already_associates_is_refused_as_a_conflict_and_nothing_changes() {
    let em = Emulator::new();
    // Rows differ along the spelling the already-associated rule is named with, and
    // along whether the request also names a rule that is NOT associated yet (which
    // must not be applied either: the request is refused as a whole).
    for (case, spelling, with_a_new_rule) in [
        ("the associated rule alone, by full ARN", Spelling::Full, false),
        ("the associated rule alone, by bare id", Spelling::Bare, false),
        ("the associated rule beside a new one", Spelling::Full, true),
        (
            "the associated rule as a bare id beside a new one",
            Spelling::Bare,
            true,
        ),
    ] {
        let held = em.create_rule().await;
        let plan = em.create_plan(std::slice::from_ref(&held)).await;
        let mut named = vec![spelling.of(&held)];
        let fresh = if with_a_new_rule {
            let fresh = em.create_rule().await;
            named.push(fresh.clone());
            Some(fresh)
        } else {
            None
        };

        let (status, body) = em.associate(&plan, &named).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(held.as_str()),
            "case {case:?}: the refusal must name the rule already associated, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("pricingrule"),
            "case {case:?}: the refusal must name the resource type, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("PRICING_RULE_IN_PRICING_PLAN_CONFLICT"),
            "case {case:?}: expected the modelled conflict reason, got {body:?}"
        );

        // Nothing was applied: the plan still associates exactly what it held, and the
        // rule that was new to it is associated with no plan at all.
        let reported = em.rules_of(&plan).await;
        assert_eq!(
            reported,
            vec![held.clone()],
            "case {case:?}: the refused request must leave the plan alone, got {reported:?}"
        );
        if let Some(fresh) = fresh {
            let holders = em.plans_of(&fresh).await;
            assert_eq!(
                holders,
                Vec::<String>::new(),
                "case {case:?}: the refused request must not have associated {fresh:?}, \
                 got {holders:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_rule_named_twice_in_a_request_is_refused_and_nothing_is_stored() {
    let em = Emulator::new();
    // Rows differ along the pair of spellings the one rule is named by: a rule is the
    // rule, not the spelling, so two spellings of it in one request are a duplicate
    // too.
    for (case, first, second) in [
        ("the same bare id twice", Spelling::Bare, Spelling::Bare),
        ("the same full ARN twice", Spelling::Full, Spelling::Full),
        ("a bare id and the full ARN of one rule", Spelling::Bare, Spelling::Full),
        ("a full ARN and the bare id of one rule", Spelling::Full, Spelling::Bare),
    ] {
        let plan = em.create_plan(&[]).await;
        let rule = em.create_rule().await;
        let (status, body) = em
            .associate(&plan, &[first.of(&rule), second.of(&rule)])
            .await;
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
        let reported = em.rules_of(&plan).await;
        assert_eq!(
            reported,
            Vec::<String>::new(),
            "case {case:?}: the refused request must store nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_rule_that_exists_nowhere_is_refused_and_nothing_is_stored() {
    let em = Emulator::new();
    let existing = em.create_rule().await;
    // Rows differ along the spelling of the rule that was never created, the account
    // its ARN names, and whether a rule that DOES exist is named beside it (which must
    // not be applied: the request is refused as a whole).
    for (case, missing, with_existing) in [
        ("a bare id nobody holds", "NOSUCHRUL1".to_string(), false),
        (
            "a full ARN in this account that nobody holds",
            rule_arn_in(account(), "NOSUCHRUL2"),
            false,
        ),
        (
            "a full ARN in another account",
            rule_arn_in("123456789012", &bare_id(&existing)),
            false,
        ),
        (
            "a missing rule beside one that exists",
            "NOSUCHRUL3".to_string(),
            true,
        ),
    ] {
        let plan = em.create_plan(&[]).await;
        let mut named = vec![missing.clone()];
        if with_existing {
            named.push(existing.clone());
        }
        let (status, body) = em.associate(&plan, &named).await;
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
            Some("PRICINGRULES_NOT_EXIST"),
            "case {case:?}: expected the modelled missing-rule reason, got {body:?}"
        );
        let reported = em.rules_of(&plan).await;
        assert_eq!(
            reported,
            Vec::<String>::new(),
            "case {case:?}: the refused request must store nothing, got {reported:?}"
        );
    }

    // The rule that exists was never associated by any of the refusals above.
    let holders = em.plans_of(&existing).await;
    assert_eq!(
        holders,
        Vec::<String>::new(),
        "a refused request must not have associated the rule that exists, got {holders:?}"
    );
}

#[tokio::test]
async fn a_plan_this_account_does_not_hold_is_refused_with_the_modelled_not_found() {
    let em = Emulator::new();
    let rule = em.create_rule().await;
    // One plan DOES exist, so the refusals below are about the plan addressed and not
    // about an empty store.
    let existing = em.create_plan(&[]).await;

    // Rows differ along the spelling of the plan that was never created and the
    // account its ARN names.
    for plan in [
        "NOSUCHPLAN".to_string(),
        "0000000000".to_string(),
        plan_arn_in(account(), "NOSUCHPLAN"),
        plan_arn_in("123456789012", &bare_id(&existing)),
    ] {
        let (status, body) = em.associate(&plan, std::slice::from_ref(&rule)).await;
        assert_eq!(
            status, 404,
            "case {plan:?}: expected 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {plan:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("pricingplan"),
            "case {plan:?}: the refusal must name the resource type, got {body:?}"
        );
        let normalized = if plan.starts_with("arn:") {
            plan.clone()
        } else {
            plan_arn_in(account(), &plan)
        };
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(normalized.as_str()),
            "case {plan:?}: the refusal must name the plan addressed, got {body:?}"
        );
    }

    // No refusal minted the plan it refused, and none associated the rule.
    let holders = em.plans_of(&rule).await;
    assert_eq!(
        holders,
        Vec::<String>::new(),
        "a refused request must not have associated the rule, got {holders:?}"
    );
    let reported = em.rules_of(&existing).await;
    assert_eq!(
        reported,
        Vec::<String>::new(),
        "the plan that does exist must still associate nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn an_aws_managed_plan_cannot_take_an_association() {
    let em = Emulator::new();
    let rule = em.create_rule().await;
    // The plans Amazon Web Services owns rather than the caller, as the
    // `PricingPlanArn` pattern enumerates them beside a customer-minted id:
    // `(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})`, with the `aws` account segment
    // the same pattern allows.
    for plan in [
        "BasicPricingPlan".to_string(),
        "Passthrough".to_string(),
        plan_arn_in("aws", "BasicPricingPlan"),
        plan_arn_in("aws", "Passthrough"),
    ] {
        let (status, body) = em.associate(&plan, std::slice::from_ref(&rule)).await;
        assert_eq!(
            status, 400,
            "case {plan:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {plan:?}: expected ValidationException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("MISMATCHED_PRICINGPLAN_ARN"),
            "case {plan:?}: expected the modelled mismatched-plan reason, got {body:?}"
        );
        // An AWS-managed plan still associates no rule of the caller's.
        let reported = em.rules_of(&plan).await;
        assert_eq!(
            reported,
            Vec::<String>::new(),
            "case {plan:?}: an AWS-managed plan must still carry no rule, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_plan_associates_at_most_the_per_plan_maximum_of_rules() {
    let em = Emulator::new();
    let plan = em.create_plan(&[]).await;
    let held = em.create_rules(MAX_RULES_PER_PLAN).await;
    let (status, body) = em.associate(&plan, &held).await;
    assert_eq!(
        status, 200,
        "case \"the plan filled to its maximum\": expected 200, got {status} with {body:?}"
    );
    let full = em.rules_of(&plan).await;
    assert_eq!(
        full,
        in_arn_order(&held),
        "case \"the plan filled to its maximum\": expected {:?}, got {full:?}",
        in_arn_order(&held)
    );

    // Rows differ along how many rules the request past the bound names.
    for over in [1usize, 2, 5] {
        let extra = em.create_rules(over).await;
        let (status, body) = em.associate(&plan, &extra).await;
        assert_eq!(
            status, 402,
            "case \"{over} rule(s) past the bound\": expected 402, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceLimitExceededException",
            "case \"{over} rule(s) past the bound\": expected ServiceLimitExceededException, \
             got {body:?}"
        );
        assert_eq!(
            body["LimitCode"].as_str(),
            Some(PLAN_LIMIT_CODE),
            "case \"{over} rule(s) past the bound\": expected the per-plan limit code, \
             got {body:?}"
        );
        assert_eq!(
            body["ServiceCode"].as_str(),
            Some("billingconductor"),
            "case \"{over} rule(s) past the bound\": expected the service code, got {body:?}"
        );
        let reported = em.rules_of(&plan).await;
        assert_eq!(
            reported, full,
            "case \"{over} rule(s) past the bound\": the refusal must displace nothing, \
             got {reported:?}"
        );
    }
}

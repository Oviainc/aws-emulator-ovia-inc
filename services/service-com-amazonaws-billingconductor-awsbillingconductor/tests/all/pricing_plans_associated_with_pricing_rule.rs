// SPDX-License-Identifier: CC0-1.0

//! `ListPricingPlansAssociatedWithPricingRule` end to end over the service's real
//! `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same path the
//! production router serves.
//!
//! RULE: the operation REPORTS, for the pricing rule the request names (in either
//! spelling the modelled `PricingRuleArn` pattern allows), exactly the set of pricing
//! plans whose stored associations name that rule — each as its FULL plan ARN,
//! deduplicated, in ARN order, bounded by the request's `MaxResults` with a
//! `NextToken` naming the rest — while a rule this account does not hold is refused
//! with the modelled 404. It is the inverse index of
//! `ListPricingRulesAssociatedToPricingPlan` over one set of associations, so the two
//! agree. Nothing is written: a later caller sees exactly what an earlier one did.
//!
//! Every table's rows differ along what the rule generalises over: how many plans
//! associate the rule (0, 1, 2, 3, 12), the spelling each association was recorded
//! with (bare id vs full ARN, both spellings in one plan), whether the associating
//! plan also associates other rules, the page size, the rule spelling the read uses,
//! the period asked about, and which account the rule ARN names.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /list-pricing-plans-associated-with-pricing-rule` — the operation's modelled
/// HTTP binding (`smithy_query(query="operation",
/// operation="ListPricingPlansAssociatedWithPricingRule")`).
const LIST_PATH: &str = "/list-pricing-plans-associated-with-pricing-rule";

/// The inverse read, whose answer this one has to agree with
/// (`smithy_query(query="operation",
/// operation="ListPricingRulesAssociatedToPricingPlan")`).
const INVERSE_PATH: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// The upper bound of the modelled `MaxPricingRuleResults` range (1..100), which is
/// also the page size the service applies when a request names none.
const MAX_RESULTS_CEILING: i64 = 100;

/// How many plans the pagination table associates with one rule. Well under the
/// modelled `PricingPlanArns` maximum (len 1..100) and more than the smallest page
/// size, so every page size in the table yields several pages.
const PAGED_PLAN_COUNT: usize = 12;

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account.
    ///
    /// Billing Conductor is a payer-account-only API (see the `crate::service` module
    /// docs): its control plane is authorized for the payer account alone, and the
    /// store records no payer account by default — so this harness designates one,
    /// which is the world in which the reads these tests are about are reachable at
    /// all. The denial every other caller gets is asserted in
    /// `tests/all/payer_account_gate.rs`.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// POST a JSON body to one of this service's routes and return the status plus
    /// the parsed response body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
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

    /// Create a pricing plan associating `rules` (the spellings the caller sends), and
    /// answer the ARN the service minted.
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

    /// `ListPricingPlansAssociatedWithPricingRule` for one rule, with the optional
    /// members a row sets.
    async fn list(
        &self,
        rule: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
        billing_period: Option<&str>,
    ) -> (u16, Value) {
        let mut body = json!({"PricingRuleArn": rule});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token {
            body["NextToken"] = json!(token);
        }
        if let Some(period) = billing_period {
            body["BillingPeriod"] = json!(period);
        }
        self.post(LIST_PATH, &body).await
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

/// The emulator's account id — the account every ARN this service mints carries.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The full `PricingRuleArn` of a rule id held by `account_id`, in the form the
/// modelled pattern
/// `^(arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingrule/)?[a-zA-Z0-9]{10}$`
/// spells it.
fn rule_arn_in(account_id: &str, id: &str) -> String {
    format!("arn:aws:billingconductor::{account_id}:pricingrule/{id}")
}

/// The bare resource id of a full ARN this service minted.
fn resource_id(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .expect("a minted ARN ends in its resource id")
        .to_string()
}

/// The `PricingPlanArns` a response reports, as owned strings (an absent member is an
/// empty set).
fn reported(body: &Value) -> Vec<String> {
    body.get("PricingPlanArns")
        .and_then(Value::as_array)
        .map(|arns| {
            arns.iter()
                .map(|arn| arn.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Does `value` have the shape of the modelled `BillingPeriod`
/// (`^\d{4}-(0?[1-9]|1[012])$`, i.e. `YYYY-MM` with a month in 1..=12)?
fn is_billing_period(value: &str) -> bool {
    let Some((year, month)) = value.split_once('-') else {
        return false;
    };
    year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && month
            .parse::<u32>()
            .is_ok_and(|month| (1..=12).contains(&month))
        && (1..=2).contains(&month.len())
}

/// How one plan of a table row spells the rule it associates.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    /// The bare `[a-zA-Z0-9]{10}` resource id.
    BareId,
    /// The full `arn:aws:billingconductor::<account>:pricingrule/<id>` ARN.
    FullArn,
    /// BOTH spellings, in one plan's `PricingRuleArns` — one association, twice named.
    BothSpellings,
}

impl Spelling {
    /// The `PricingRuleArns` entries this spelling contributes for `rule`.
    fn entries(self, rule_arn: &str) -> Vec<String> {
        let bare = resource_id(rule_arn);
        match self {
            Spelling::BareId => vec![bare],
            Spelling::FullArn => vec![rule_arn.to_string()],
            Spelling::BothSpellings => vec![bare, rule_arn.to_string()],
        }
    }
}

/// One plan a row creates: how it spells the rule under test, and how many OTHER
/// rules it also associates.
#[derive(Debug, Clone, Copy)]
struct PlanSpec {
    spelling: Spelling,
    other_rules: usize,
}

/// One row of the association table.
struct AssociationRow {
    case: &'static str,
    /// The plans that DO associate the rule under test.
    plans: Vec<PlanSpec>,
}

/// The table: rows differ along how many plans associate the rule, the spelling each
/// association was recorded with, whether one plan names the rule twice, and whether
/// the associating plan also associates other rules.
fn association_table() -> Vec<AssociationRow> {
    vec![
        AssociationRow {
            case: "no plan associates the rule",
            plans: vec![],
        },
        AssociationRow {
            case: "one plan, the association recorded as a bare rule id",
            plans: vec![PlanSpec {
                spelling: Spelling::BareId,
                other_rules: 0,
            }],
        },
        AssociationRow {
            case: "one plan, the association recorded as a full rule ARN",
            plans: vec![PlanSpec {
                spelling: Spelling::FullArn,
                other_rules: 0,
            }],
        },
        AssociationRow {
            case: "both spellings in ONE plan is ONE association",
            plans: vec![PlanSpec {
                spelling: Spelling::BothSpellings,
                other_rules: 0,
            }],
        },
        AssociationRow {
            case: "two plans in mixed spellings",
            plans: vec![
                PlanSpec {
                    spelling: Spelling::FullArn,
                    other_rules: 0,
                },
                PlanSpec {
                    spelling: Spelling::BareId,
                    other_rules: 0,
                },
            ],
        },
        AssociationRow {
            case: "three plans, two of which also associate other rules",
            plans: vec![
                PlanSpec {
                    spelling: Spelling::BareId,
                    other_rules: 2,
                },
                PlanSpec {
                    spelling: Spelling::FullArn,
                    other_rules: 1,
                },
                PlanSpec {
                    spelling: Spelling::BothSpellings,
                    other_rules: 0,
                },
            ],
        },
        AssociationRow {
            case: "twelve plans associate one rule",
            plans: (0..PAGED_PLAN_COUNT)
                .map(|index| PlanSpec {
                    spelling: if index % 2 == 0 {
                        Spelling::BareId
                    } else {
                        Spelling::FullArn
                    },
                    other_rules: index % 3,
                })
                .collect(),
        },
    ]
}

#[tokio::test]
async fn a_rule_reports_exactly_the_plans_that_associate_it_as_full_arns_in_arn_order() {
    let em = Emulator::new();
    for (row_index, row) in association_table().into_iter().enumerate() {
        // Each row gets its own rule, plus a DECOY rule associated by a plan that
        // must never be reported for the rule under test.
        let rule = em.create_rule(&format!("Rule-{row_index}")).await;
        let decoy_rule = em.create_rule(&format!("Decoy-Rule-{row_index}")).await;
        let decoy_plan = em
            .create_plan(
                &format!("decoy-plan-{row_index}"),
                std::slice::from_ref(&decoy_rule),
            )
            .await;

        let mut expected: Vec<String> = Vec::new();
        for (plan_index, spec) in row.plans.iter().enumerate() {
            let mut rules = spec.spelling.entries(&rule);
            for other in 0..spec.other_rules {
                rules.push(
                    em.create_rule(&format!("Other-{row_index}-{plan_index}-{other}"))
                        .await,
                );
            }
            expected.push(
                em.create_plan(&format!("plan-{row_index}-{plan_index}"), &rules)
                    .await,
            );
        }
        expected.sort();

        // The read answers the same for either spelling of the rule the modelled
        // pattern allows: the bare id and the full ARN name ONE rule.
        for spelling in [rule.clone(), resource_id(&rule)] {
            let (status, body) = em.list(&spelling, None, None, None).await;
            assert_eq!(
                status, 200,
                "case {:?} read by {spelling:?}: expected 200, got {status} with {body:?}",
                row.case
            );
            let actual = reported(&body);
            assert_eq!(
                actual, expected,
                "case {:?} read by {spelling:?}: expected {expected:?}, got {actual:?}",
                row.case
            );
            assert!(
                !actual.contains(&decoy_plan),
                "case {:?} read by {spelling:?}: a plan associating only another rule must \
                 not be reported, got {actual:?}",
                row.case
            );
            assert!(
                actual.windows(2).all(|pair| pair[0] < pair[1]),
                "case {:?} read by {spelling:?}: the plans must be reported in strict ARN \
                 order, got {actual:?}",
                row.case
            );
            assert_eq!(
                body["PricingRuleArn"].as_str(),
                Some(rule.as_str()),
                "case {:?} read by {spelling:?}: the response must echo the rule's full ARN, \
                 got {body:?}",
                row.case
            );
            // `PricingPlanArns` is modelled `@length(min: 1)`, so a rule no plan
            // associates reports NO member rather than an empty list.
            assert_eq!(
                body.get("PricingPlanArns").is_some(),
                !expected.is_empty(),
                "case {:?} read by {spelling:?}: an empty association set must omit \
                 PricingPlanArns, got {body:?}",
                row.case
            );
            assert_eq!(
                body.get("NextToken"),
                None,
                "case {:?} read by {spelling:?}: a page that holds everything must hand back \
                 no NextToken, got {body:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn the_two_association_reads_agree_over_one_set_of_associations() {
    let em = Emulator::new();
    let rule = em.create_rule("Shared-Rule").await;
    let other = em.create_rule("Other-Rule").await;

    // Rows differ along which rules a plan associates: only the rule under test, it
    // plus another, only the other, and none at all.
    let plans = [
        ("only the rule under test", vec![rule.clone()]),
        ("the rule and another", vec![rule.clone(), other.clone()]),
        ("only another rule", vec![other.clone()]),
        ("no rule at all", vec![]),
    ];
    let mut expected: Vec<String> = Vec::new();
    for (index, (case, rules)) in plans.iter().enumerate() {
        let plan = em.create_plan(&format!("agree-plan-{index}"), rules).await;
        if rules.contains(&rule) {
            expected.push(plan.clone());
        }

        // The inverse read reports the rule exactly when this read reports the plan.
        let (status, inverse) = em
            .post(INVERSE_PATH, &json!({"PricingPlanArn": plan}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the inverse read expected 200, got {status} with {inverse:?}"
        );
        let associates = inverse
            .get("PricingRuleArns")
            .and_then(Value::as_array)
            .is_some_and(|arns| arns.iter().any(|arn| arn.as_str() == Some(rule.as_str())));
        assert_eq!(
            associates,
            rules.contains(&rule),
            "case {case:?}: the inverse read must agree on the association, got {inverse:?}"
        );
    }
    expected.sort();

    let (status, body) = em.list(&rule, None, None, None).await;
    assert_eq!(
        status, 200,
        "the forward read expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        reported(&body),
        expected,
        "the forward read must report exactly the plans the inverse read associates, \
         expected {expected:?}, got {body:?}"
    );
}

#[tokio::test]
async fn the_reported_period_is_the_one_the_request_named_and_otherwise_absent() {
    let em = Emulator::new();
    let rule = em.create_rule("Period-Rule").await;
    let plan = em
        .create_plan("period-plan", std::slice::from_ref(&rule))
        .await;

    // Rows differ along the period asked for: months at each end of the modelled
    // `BillingPeriod` pattern, a past year and a future one, and the one-digit month
    // form the pattern also allows (`0?[1-9]`).
    for period in ["2020-01", "2026-9", "2026-10", "2031-12"] {
        let (status, body) = em.list(&rule, None, None, Some(period)).await;
        assert_eq!(
            status, 200,
            "case {period:?}: expected 200, got {status} with {body:?}"
        );
        assert_eq!(
            body["BillingPeriod"].as_str(),
            Some(period),
            "case {period:?}: the response must name the period the request asked about, \
             got {body:?}"
        );
        assert!(
            is_billing_period(body["BillingPeriod"].as_str().unwrap_or_default()),
            "case {period:?}: the reported period must satisfy the modelled YYYY-MM form, \
             got {body:?}"
        );
        assert_eq!(
            reported(&body),
            vec![plan.clone()],
            "case {period:?}: the associations must not depend on the period, got {body:?}"
        );
    }

    // A request that named no period leaves the member absent rather than inventing
    // one — the rule `ListPricingRulesAssociatedToPricingPlan` applies to this member.
    let (status, body) = em.list(&rule, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"no BillingPeriod\": expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        body.get("BillingPeriod"),
        None,
        "case \"no BillingPeriod\": a request that named no period must not be answered with \
         one, got {body:?}"
    );
    assert_eq!(
        reported(&body),
        vec![plan],
        "case \"no BillingPeriod\": the associations must still be reported, got {body:?}"
    );
}

#[tokio::test]
async fn every_page_size_pages_the_whole_association_set_exactly_once() {
    let em = Emulator::new();
    let rule = em.create_rule("Paged-Rule").await;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..PAGED_PLAN_COUNT {
        expected.push(
            em.create_plan(
                &format!("paged-plan-{index}"),
                std::slice::from_ref(&rule),
            )
                .await,
        );
    }
    expected.sort();

    // Rows differ along the page size: the modelled minimum, sizes that do and do not
    // divide the set, the exact set size, the modelled maximum, and no size at all
    // (the server's own page size).
    for max_results in [Some(1), Some(2), Some(5), Some(11), Some(12), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, body) = em.list(&rule, max_results, token.as_deref(), None).await;
            assert_eq!(
                status, 200,
                "case {case}: page {pages}: expected 200, got {status} with {body:?}"
            );
            let page = reported(&body);
            let requested = max_results.unwrap_or(MAX_RESULTS_CEILING);
            let limit = usize::try_from(requested.min(MAX_RESULTS_CEILING))
                .expect("a positive page size fits in usize");
            assert!(
                !page.is_empty() && page.len() <= limit,
                "case {case}: page {pages}: expected 1..={limit} plans, got {page:?}"
            );
            seen.extend(page);
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len(),
                "case {case}: paging did not terminate: {pages} pages for {} plans",
                expected.len()
            );
        }
        assert_eq!(
            seen, expected,
            "case {case}: the pages must concatenate to the whole association set in ARN \
             order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_and_the_bounds_are_served() {
    let em = Emulator::new();
    let rule = em.create_rule("Bounds-Rule").await;
    let plan = em
        .create_plan("bounds-plan", std::slice::from_ref(&rule))
        .await;

    // The published bounds of `MaxPricingRuleResults` (range 1..100) and one past each
    // of them.
    for (max_results, expected_status) in [(1, 200), (100, 200), (0, 400), (101, 400)] {
        let (status, body) = em.list(&rule, Some(max_results), None, None).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected {expected_status}, got {status} with \
             {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case MaxResults={max_results}: expected ValidationException, got {body:?}"
            );
        } else {
            assert_eq!(
                reported(&body),
                vec![plan.clone()],
                "case MaxResults={max_results}: expected the single association, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_rule_this_account_does_not_hold_is_refused_with_the_modelled_not_found() {
    let em = Emulator::new();
    // One rule DOES exist, and a plan associates it, so the refusals below are about
    // the rule asked for and not about an empty store.
    let existing = em.create_rule("Existing-Rule").await;
    let plan = em
        .create_plan("existing-plan", std::slice::from_ref(&existing))
        .await;

    // Rows differ along the spelling of a rule that was never created, and along the
    // account an ARN form names: this service holds one account's rules, so an ARN in
    // another account is not a rule this caller holds either.
    let foreign_cases = [
        ("a bare id nobody holds", "NOSUCHRULE".to_string()),
        ("another bare id nobody holds", "0000000000".to_string()),
        (
            "a full ARN nobody holds",
            rule_arn_in(account(), "NOSUCHRULE"),
        ),
        (
            "another full ARN nobody holds",
            rule_arn_in(account(), "ZZZZZZZZZZ"),
        ),
        (
            "another account's ARN for an id this account holds",
            rule_arn_in("123456789012", &resource_id(&existing)),
        ),
        (
            "another account's ARN for an id nobody holds",
            rule_arn_in("999999999999", "NOSUCHRULE"),
        ),
    ];
    for (case, rule) in &foreign_cases {
        let (status, body) = em.list(rule, None, None, None).await;
        assert_eq!(
            status, 404,
            "case {case} ({rule:?}): expected 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case} ({rule:?}): expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("pricingrule"),
            "case {case} ({rule:?}): the refusal must name the resource type, got {body:?}"
        );
        let normalized = if rule.starts_with("arn:") {
            rule.clone()
        } else {
            rule_arn_in(account(), rule)
        };
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(normalized.as_str()),
            "case {case} ({rule:?}): the refusal must name the rule asked for, got {body:?}"
        );
    }

    // The refusals left the store alone: the rule that does exist still reports its
    // plan.
    let (status, body) = em.list(&existing, None, None, None).await;
    assert_eq!(
        status, 200,
        "the existing rule must still be readable, got {status} with {body:?}"
    );
    assert_eq!(
        reported(&body),
        vec![plan],
        "the existing rule must still report its association, got {body:?}"
    );
}

#[tokio::test]
async fn reading_the_associations_changes_nothing_a_later_caller_sees() {
    let em = Emulator::new();
    let rule = em.create_rule("Stable-Rule").await;
    let mut expected = vec![
        em.create_plan("stable-plan-0", &[resource_id(&rule)]).await,
        em.create_plan("stable-plan-1", std::slice::from_ref(&rule))
            .await,
    ];
    expected.sort();

    let (_, first) = em.list(&rule, None, None, None).await;

    // Rows differ along the read that comes before: a full read, a paged read, a read
    // for another period, a read by the bare id, a refused read of a rule that does
    // not exist, and a refused read of a rule ARN in another account.
    for max_results in [None, Some(1)] {
        let _ = em.list(&rule, max_results, None, None).await;
    }
    let _ = em.list(&rule, None, None, Some("2026-01")).await;
    let _ = em.list(&resource_id(&rule), None, None, None).await;
    let _ = em.list("NOSUCHRULE", None, None, None).await;
    let _ = em
        .list(&rule_arn_in("123456789012", "NOSUCHRULE"), None, None, None)
        .await;

    let (status, last) = em.list(&rule, None, None, None).await;
    assert_eq!(
        status, 200,
        "after the reads above: expected 200, got {status} with {last:?}"
    );
    assert_eq!(
        reported(&last),
        reported(&first),
        "a read must not change what a later caller sees: first {:?}, later {:?}",
        reported(&first),
        reported(&last)
    );
    assert_eq!(
        reported(&last),
        expected,
        "the rule must still report exactly the plans that associate it, expected \
         {expected:?}, got {last:?}"
    );

    // A refused read did not mint the rule it refused either.
    let (status, body) = em.list("NOSUCHRULE", None, None, None).await;
    assert_eq!(
        status, 404,
        "a refused read must not have created the rule, got {status} with {body:?}"
    );
}

#[tokio::test]
async fn deleting_a_plan_or_a_rule_is_reflected_in_what_the_read_reports() {
    let em = Emulator::new();
    let rule = em.create_rule("Lifecycle-Rule").await;
    let keep = em
        .create_plan("lifecycle-keep", std::slice::from_ref(&rule))
        .await;
    let removed = em
        .create_plan("lifecycle-drop", &[resource_id(&rule)])
        .await;
    let mut both = vec![keep.clone(), removed.clone()];
    both.sort();

    let (status, body) = em.list(&rule, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"both plans\": expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        reported(&body),
        both,
        "case \"both plans\": expected {both:?}, got {body:?}"
    );

    // A plan that no longer exists is no longer an association: the report is derived
    // from the stored plans, never from a count kept beside them.
    let (status, deleted) = em
        .post("/delete-pricing-plan", &json!({"Arn": removed}))
        .await;
    assert_eq!(
        status, 200,
        "setup DeletePricingPlan: expected 200, got {status} with {deleted:?}"
    );
    let (status, body) = em.list(&rule, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"one plan deleted\": expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        reported(&body),
        vec![keep.clone()],
        "case \"one plan deleted\": expected only {keep:?}, got {body:?}"
    );

    // And once the last plan is gone the rule associates nothing, so the modelled
    // `@length(min: 1)` member is absent.
    let (status, deleted) = em
        .post("/delete-pricing-plan", &json!({"Arn": keep}))
        .await;
    assert_eq!(
        status, 200,
        "setup DeletePricingPlan (last): expected 200, got {status} with {deleted:?}"
    );
    let (status, body) = em.list(&rule, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"every plan deleted\": expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        body.get("PricingPlanArns"),
        None,
        "case \"every plan deleted\": an empty association set must omit PricingPlanArns, \
         got {body:?}"
    );

    // A rule that no longer exists is not an empty association set but the modelled
    // 404.
    let (status, deleted) = em
        .post("/delete-pricing-rule", &json!({"Arn": rule.clone()}))
        .await;
    assert_eq!(
        status, 200,
        "setup DeletePricingRule: expected 200, got {status} with {deleted:?}"
    );
    let (status, body) = em.list(&rule, None, None, None).await;
    assert_eq!(
        status, 404,
        "case \"the rule deleted\": expected 404, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"the rule deleted\": expected ResourceNotFoundException, got {body:?}"
    );
}

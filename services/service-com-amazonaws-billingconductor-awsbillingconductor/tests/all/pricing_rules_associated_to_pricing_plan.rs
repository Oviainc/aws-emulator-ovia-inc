// SPDX-License-Identifier: CC0-1.0

//! `ListPricingRulesAssociatedToPricingPlan` end to end over the service's real
//! `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same path the
//! production router serves.
//!
//! RULE: the operation REPORTS, for the pricing plan the request names (in either
//! spelling the modelled `PricingPlanArn` pattern allows), exactly the set of
//! pricing rules persisted as that plan's associations — each as its FULL rule ARN,
//! deduplicated, in ARN order, bounded by the request's `MaxResults` with a
//! `NextToken` naming the rest — while a plan this account does not hold is refused
//! with the modelled 404. Nothing is written: a later caller sees exactly what an
//! earlier one did.
//!
//! Every table's rows differ along what the rule generalises over: how many rules a
//! plan associates (0, 1, 2, 3, 30 — the modelled input maximum), the spelling each
//! association was recorded with (bare id vs full ARN, both spellings of one rule),
//! the order they were recorded in, the page size, the plan spelling the read uses,
//! and which account the plan ARN names.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /list-pricing-rules-associated-to-pricing-plan` — the operation's modelled
/// HTTP binding (`smithy_query(query="operation",
/// operation="ListPricingRulesAssociatedToPricingPlan")`).
const LIST_PATH: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// The upper bound of the modelled `MaxPricingPlanResults` range (1..100), which is
/// also the page size the service applies when a request names none.
const MAX_RESULTS_CEILING: i64 = 100;

/// The modelled maximum length of `CreatePricingPlanInput.PricingRuleArns`
/// (`PricingRuleArnsInput`, len 0..30).
const MAX_ASSOCIATIONS_PER_CREATE: usize = 30;

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
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

    /// Create a pricing plan associating `rules` (the spellings the caller sends),
    /// and answer the ARN the service minted.
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

    /// `ListPricingRulesAssociatedToPricingPlan` for one plan, with the optional
    /// members a row sets.
    async fn list(
        &self,
        plan: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
        billing_period: Option<&str>,
    ) -> (u16, Value) {
        let mut body = json!({"PricingPlanArn": plan});
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

/// The full `PricingRuleArn` of a rule id, in the form the modelled pattern
/// `^(arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingrule/)?[a-zA-Z0-9]{10}$`
/// spells it.
fn rule_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::{}:pricingrule/{id}", account())
}

/// The full `PricingPlanArn` of a plan id held by `account_id`.
fn plan_arn_in(account_id: &str, id: &str) -> String {
    format!("arn:aws:billingconductor::{account_id}:pricingplan/{id}")
}

/// A well-formed 10-character rule id (`[a-zA-Z0-9]{10}`) derived from `index`.
fn rule_id(index: usize) -> String {
    format!("RULE{index:06}")
}

/// The `PricingRuleArns` a response reports, as owned strings (an absent member is
/// an empty set).
fn reported(body: &Value) -> Vec<String> {
    body.get("PricingRuleArns")
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

/// One row of the association table: the spellings a plan was created with, and the
/// full rule ARNs the service owes back for it.
struct AssociationRow {
    case: &'static str,
    created_with: Vec<String>,
    expected: Vec<String>,
}

/// The table: rows differ along the number of associations, the spelling each was
/// recorded with, whether two spellings name one rule, and the order they arrived
/// in.
fn association_table() -> Vec<AssociationRow> {
    let many: Vec<String> = (0..MAX_ASSOCIATIONS_PER_CREATE).map(rule_id).collect();
    let mut many_expected: Vec<String> = many.iter().map(|id| rule_arn(id)).collect();
    many_expected.sort();
    vec![
        AssociationRow {
            case: "no association at all",
            created_with: vec![],
            expected: vec![],
        },
        AssociationRow {
            case: "one association, recorded as a bare rule id",
            created_with: vec!["AAAAAAAAA1".to_string()],
            expected: vec![rule_arn("AAAAAAAAA1")],
        },
        AssociationRow {
            case: "one association, recorded as a full rule ARN",
            created_with: vec![rule_arn("BBBBBBBBB2")],
            expected: vec![rule_arn("BBBBBBBBB2")],
        },
        AssociationRow {
            case: "two associations in mixed spellings",
            created_with: vec!["CCCCCCCCC3".to_string(), rule_arn("DDDDDDDDD4")],
            expected: vec![rule_arn("CCCCCCCCC3"), rule_arn("DDDDDDDDD4")],
        },
        AssociationRow {
            case: "both spellings of ONE rule is ONE association",
            created_with: vec!["EEEEEEEEE5".to_string(), rule_arn("EEEEEEEEE5")],
            expected: vec![rule_arn("EEEEEEEEE5")],
        },
        AssociationRow {
            case: "three associations recorded out of ARN order",
            created_with: vec![
                "ZZZZZZZZZ9".to_string(),
                "AAAAAAAAA0".to_string(),
                "MMMMMMMMM5".to_string(),
            ],
            expected: vec![
                rule_arn("AAAAAAAAA0"),
                rule_arn("MMMMMMMMM5"),
                rule_arn("ZZZZZZZZZ9"),
            ],
        },
        AssociationRow {
            case: "the modelled maximum of thirty associations",
            created_with: many,
            expected: many_expected,
        },
    ]
}

#[tokio::test]
async fn a_plan_reports_exactly_the_rules_it_associates_as_full_arns_in_arn_order() {
    let em = Emulator::new();
    for (index, row) in association_table().into_iter().enumerate() {
        let plan_arn = em
            .create_plan(&format!("plan-{index}"), &row.created_with)
            .await;
        let plan_id = plan_arn
            .rsplit('/')
            .next()
            .expect("the minted plan ARN ends in its resource id")
            .to_string();

        // The read answers the same for either spelling of the plan the modelled
        // pattern allows: the bare id and the full ARN name ONE plan.
        for spelling in [plan_arn.clone(), plan_id.clone()] {
            let (status, body) = em.list(&spelling, None, None, None).await;
            assert_eq!(
                status, 200,
                "case {:?} read by {spelling:?}: expected 200, got {status} with {body:?}",
                row.case
            );
            assert_eq!(
                reported(&body),
                row.expected,
                "case {:?} read by {spelling:?}: expected {:?}, got {body:?}",
                row.case,
                row.expected
            );
            assert_eq!(
                body["PricingPlanArn"].as_str(),
                Some(plan_arn.as_str()),
                "case {:?} read by {spelling:?}: the response must echo the plan's full ARN, \
                 got {body:?}",
                row.case
            );
            // `PricingRuleArns` is modelled `@length(min: 1)`, so a plan with no
            // association reports NO member rather than an empty list.
            assert_eq!(
                body.get("PricingRuleArns").is_some(),
                !row.expected.is_empty(),
                "case {:?} read by {spelling:?}: an empty association set must omit \
                 PricingRuleArns, got {body:?}",
                row.case
            );
            assert_eq!(
                body.get("NextToken"),
                None,
                "case {:?} read by {spelling:?}: a single page must hand back no NextToken, \
                 got {body:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn the_reported_period_is_the_one_the_request_named_and_otherwise_absent() {
    let em = Emulator::new();
    let plan = em
        .create_plan("period-plan", &["AAAAAAAAA1".to_string()])
        .await;

    // Rows differ along the period asked for: months at each end of the modelled
    // `BillingPeriod` pattern, a past year and a future one.
    for period in ["2020-01", "2026-09", "2026-10", "2031-12"] {
        let (status, body) = em.list(&plan, None, None, Some(period)).await;
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
            vec![rule_arn("AAAAAAAAA1")],
            "case {period:?}: the associations must not depend on the period, got {body:?}"
        );
    }

    // A request that named no period leaves the member absent rather than inventing
    // one — the rule `ListPricingRules` applies to the same member.
    let (status, body) = em.list(&plan, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"no BillingPeriod\": expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        body.get("BillingPeriod"),
        None,
        "case \"no BillingPeriod\": a request that named no period must not be answered \
         with one, got {body:?}"
    );
    assert_eq!(
        reported(&body),
        vec![rule_arn("AAAAAAAAA1")],
        "case \"no BillingPeriod\": the associations must still be reported, got {body:?}"
    );
}

#[tokio::test]
async fn every_page_size_pages_the_whole_association_set_exactly_once() {
    let em = Emulator::new();
    let created: Vec<String> = (0..MAX_ASSOCIATIONS_PER_CREATE).map(rule_id).collect();
    let plan = em.create_plan("paged-plan", &created).await;
    let mut expected: Vec<String> = created.iter().map(|id| rule_arn(id)).collect();
    expected.sort();

    // Rows differ along the page size: the modelled minimum, sizes that do and do
    // not divide the set, the exact set size, the modelled maximum, and no size at
    // all (the server's own page size).
    for max_results in [Some(1), Some(2), Some(7), Some(29), Some(30), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, body) = em.list(&plan, max_results, token.as_deref(), None).await;
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
                "case {case}: page {pages}: expected 1..={limit} rules, got {page:?}"
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
                "case {case}: paging did not terminate: {pages} pages for {} rules",
                expected.len()
            );
        }
        assert_eq!(
            seen, expected,
            "case {case}: the pages must concatenate to the whole association set in ARN order, \
             got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_and_the_bounds_are_served() {
    let em = Emulator::new();
    let plan = em
        .create_plan("bounds-plan", &["AAAAAAAAA1".to_string()])
        .await;

    // The published bounds of `MaxPricingPlanResults` (range 1..100) and one past
    // each of them.
    for (max_results, expected_status) in [(1, 200), (100, 200), (0, 400), (101, 400)] {
        let (status, body) = em.list(&plan, Some(max_results), None, None).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected {expected_status}, got {status} with {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case MaxResults={max_results}: expected ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_aws_managed_plan_exists_and_associates_no_rule_of_the_callers() {
    let em = Emulator::new();
    // The plans Amazon Web Services owns rather than the caller, as the
    // `PricingPlanFullArn` pattern enumerates them beside a customer-minted id:
    // `(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})`, with the `aws` account
    // segment the same pattern allows.
    for plan in [
        "BasicPricingPlan".to_string(),
        "Passthrough".to_string(),
        plan_arn_in("aws", "BasicPricingPlan"),
        plan_arn_in("aws", "Passthrough"),
    ] {
        let (status, body) = em.list(&plan, None, None, None).await;
        assert_eq!(
            status, 200,
            "case {plan:?}: an AWS-managed plan exists, expected 200, got {status} with {body:?}"
        );
        assert_eq!(
            reported(&body),
            Vec::<String>::new(),
            "case {plan:?}: an AWS-managed plan carries no rule of the caller's, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_plan_this_account_does_not_hold_is_refused_with_the_modelled_not_found() {
    let em = Emulator::new();
    // One plan DOES exist, so the refusals below are about the plan asked for and
    // not about an empty store.
    let existing = em
        .create_plan("existing-plan", &["AAAAAAAAA1".to_string()])
        .await;

    // Rows differ along the spelling of the plan that was never created.
    for plan in [
        "NOSUCHPLAN".to_string(),
        "0000000000".to_string(),
        plan_arn_in(account(), "NOSUCHPLAN"),
        plan_arn_in(account(), "ZZZZZZZZZZ"),
    ] {
        let (status, body) = em.list(&plan, None, None, None).await;
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
            "case {plan:?}: the refusal must name the plan asked for, got {body:?}"
        );
    }

    // The refusals left the store alone: the plan that does exist still reports its
    // association.
    let (status, body) = em.list(&existing, None, None, None).await;
    assert_eq!(
        status, 200,
        "the existing plan must still be readable, got {status} with {body:?}"
    );
    assert_eq!(
        reported(&body),
        vec![rule_arn("AAAAAAAAA1")],
        "the existing plan must still report its association, got {body:?}"
    );
}

#[tokio::test]
async fn a_plan_arn_naming_another_account_holds_nothing_this_caller_can_read() {
    let em = Emulator::new();
    let plan_arn = em
        .create_plan("owned-plan", &["AAAAAAAAA1".to_string()])
        .await;
    let owned_id = plan_arn
        .rsplit('/')
        .next()
        .expect("the minted plan ARN ends in its resource id")
        .to_string();

    // Rows differ along the foreign account AND along whether the plan id exists in
    // THIS account: this service holds one account's plans, so an ARN another
    // account's id prefix names is not a plan this caller holds either way.
    for account_id in ["123456789012", "999999999999", "000000000001"] {
        for (existence, id) in [
            ("an id this account holds", owned_id.as_str()),
            ("an id nobody holds", "NOSUCHPLAN"),
        ] {
            let foreign = plan_arn_in(account_id, id);
            let (status, body) = em.list(&foreign, None, None, None).await;
            assert_eq!(
                status, 404,
                "case {foreign:?} ({existence}): expected 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {foreign:?} ({existence}): expected ResourceNotFoundException, got {body:?}"
            );
            assert_eq!(
                body["ResourceId"].as_str(),
                Some(foreign.as_str()),
                "case {foreign:?} ({existence}): the refusal must name the plan asked for, \
                 got {body:?}"
            );
        }
    }

    // The caller's own plan is unaffected by the refusals.
    let (status, body) = em.list(&plan_arn, None, None, None).await;
    assert_eq!(
        status, 200,
        "the caller's own plan must still be readable, got {status} with {body:?}"
    );
    assert_eq!(
        reported(&body),
        vec![rule_arn("AAAAAAAAA1")],
        "the caller's own plan must still report its association, got {body:?}"
    );
}

#[tokio::test]
async fn reading_the_associations_changes_nothing_a_later_caller_sees() {
    let em = Emulator::new();
    let plan = em
        .create_plan(
            "stable-plan",
            &["AAAAAAAAA1".to_string(), rule_arn("BBBBBBBBB2")],
        )
        .await;

    // Rows differ along the read that comes before: a full read, a paged read, a
    // read for another period, a refused read of a plan that does not exist, and a
    // refused read of a plan ARN in another account.
    let (_, first) = em.list(&plan, None, None, None).await;
    for max_results in [None, Some(1)] {
        let _ = em.list(&plan, max_results, None, None).await;
    }
    let _ = em.list(&plan, None, None, Some("2026-01")).await;
    let _ = em.list("NOSUCHPLAN", None, None, None).await;
    let _ = em
        .list(&plan_arn_in("123456789012", "NOSUCHPLAN"), None, None, None)
        .await;

    let (status, last) = em.list(&plan, None, None, None).await;
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
        vec![rule_arn("AAAAAAAAA1"), rule_arn("BBBBBBBBB2")],
        "the plan must still report exactly the rules it associates, got {last:?}"
    );

    // A refused read did not mint the plan it refused either.
    let (status, body) = em.list("NOSUCHPLAN", None, None, None).await;
    assert_eq!(
        status, 404,
        "a refused read must not have created the plan, got {status} with {body:?}"
    );
}

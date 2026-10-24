// SPDX-License-Identifier: CC0-1.0

//! `ListPricingPlans` end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE: the operation REPORTS the pricing plans this account holds — every plan
//! `CreatePricingPlan` persisted and none it deleted — each element carrying that
//! plan's stored `Arn`, `Name`, `Description`, creation instant and last-modified
//! instant plus a `Size` DERIVED from the pricing rules currently associated with it,
//! in ARN order, restricted to the ARNs `Filters.Arns` names (in either spelling the
//! modelled `PricingPlanArn` pattern allows) and bounded by the request's `MaxResults`
//! with a `NextToken` naming the rest. Nothing is written: a later caller sees exactly
//! what an earlier one did.
//!
//! Every table's rows differ along what the rule generalises over: which optional
//! members a plan carries (no description, a description, the empty description a
//! length of 0 admits, tags), how many pricing rules it associates (0, 1, two
//! spellings of one rule, 3, the modelled create maximum of 30), how the filter spells
//! an ARN and how many it names, which class an ARN falls in (this account's, nobody's,
//! another account's, Amazon Web Services'), the page size (the modelled bounds, sizes
//! that do and do not divide the set, none at all) and the billing period asked for.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /list-pricing-plans` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation",operation="ListPricingPlans")`).
const LIST_PATH: &str = "/list-pricing-plans";

/// `POST /create-pricing-plan`, which gives this suite something to list.
const CREATE_PATH: &str = "/create-pricing-plan";

/// `PUT /update-pricing-plan` — the operation that moves a plan's `Name`,
/// `Description` and `LastModifiedTime`.
const UPDATE_PATH: &str = "/update-pricing-plan";

/// `POST /delete-pricing-plan`, which takes a plan out of the listing again.
const DELETE_PATH: &str = "/delete-pricing-plan";

/// The upper bound of the modelled `MaxPricingPlanResults` range (1..100), which is
/// also the page size the service applies when a request names none.
const MAX_RESULTS_CEILING: i64 = 100;

/// The modelled maximum length of `CreatePricingPlanInput.PricingRuleArns`
/// (`PricingRuleArnsInput`, len 0..30) — the largest association set one create can
/// record, and so the largest `Size` this suite can set up.
const MAX_ASSOCIATIONS_PER_CREATE: usize = 30;

/// The pricing plan ids Amazon Web Services owns rather than the caller, enumerated by
/// the modelled `PricingPlanArn` pattern beside the customer-minted `[a-zA-Z0-9]{10}`
/// form (`smithy_query(query="shape",shape="PricingPlanListElement")`, member `Arn`).
const AWS_MANAGED_PLAN_IDS: &[&str] = &["BasicPricingPlan", "Passthrough"];

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account.
    ///
    /// Billing Conductor is a payer-account-only API (see the `crate::service` module
    /// docs): its control plane is authorized for the payer account alone, and the store
    /// records no payer account by default — so this harness designates one, which is
    /// the world in which the read these tests are about is reachable at all. The
    /// denial every other caller gets is asserted in `tests/all/payer_account_gate.rs`,
    /// over the default store.
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

    /// Send one request over the wire and return the status plus the parsed body.
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

    /// `CreatePricingPlan`: the full ARN the service minted for `body`.
    async fn create_plan(&self, body: &Value) -> String {
        let (status, parsed) = self.call("POST", CREATE_PATH, body).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {body}: expected 200, got {status} with {parsed}"
        );
        let arn = parsed
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_plan_arn(&arn),
            "setup CreatePricingPlan {body}: expected a full PricingPlanArn, got {arn:?}"
        );
        arn
    }

    /// `ListPricingPlans` with the optional members a row sets.
    async fn list(
        &self,
        filter_arns: Option<&[String]>,
        max_results: Option<i64>,
        next_token: Option<&str>,
        billing_period: Option<&str>,
    ) -> (u16, Value) {
        let mut body = json!({});
        if let Some(arns) = filter_arns {
            body["Filters"] = json!({ "Arns": arns });
        }
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token {
            body["NextToken"] = json!(token);
        }
        if let Some(period) = billing_period {
            body["BillingPeriod"] = json!(period);
        }
        self.call("POST", LIST_PATH, &body).await
    }

    /// Every element `ListPricingPlans` reports for `filter_arns`, following
    /// `NextToken` to the last page.
    async fn list_all(&self, filter_arns: Option<&[String]>) -> Vec<Value> {
        let mut collected: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (status, body) = self.list(filter_arns, None, token.as_deref(), None).await;
            assert_eq!(
                status, 200,
                "ListPricingPlans {filter_arns:?}: expected 200, got {status} with {body}"
            );
            collected.extend(elements(&body));
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                return collected;
            }
        }
    }

    /// The one element `ListPricingPlans` reports for `arn`.
    async fn stored_plan(&self, arn: &str) -> Value {
        let listed = self.list_all(Some(&[arn.to_string()])).await;
        assert_eq!(
            listed.len(),
            1,
            "expected exactly the plan {arn:?} to be reported, got {listed:?}"
        );
        listed.into_iter().next().unwrap_or(Value::Null)
    }
}

/// The emulator's account id — the account every ARN this service mints carries.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The full `PricingPlanArn` of a plan id held by `account_id`.
fn plan_arn_in(account_id: &str, id: &str) -> String {
    format!("arn:aws:billingconductor::{account_id}:pricingplan/{id}")
}

/// The full `PricingRuleArn` of a rule id held by this account.
fn rule_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::{}:pricingrule/{id}", account())
}

/// A well-formed 10-character rule id (`[a-zA-Z0-9]{10}`) derived from `index`.
fn rule_id(index: usize) -> String {
    format!("RULE{index:06}")
}

/// Does `value` match the FULL form of the modelled `PricingPlanArn` pattern,
/// `arn:aws:billingconductor::[0-9]{12}:pricingplan/[a-zA-Z0-9]{10}`?
fn is_plan_arn(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("arn:aws:billingconductor::") else {
        return false;
    };
    let Some((account_id, resource)) = rest.split_once(':') else {
        return false;
    };
    let Some(id) = resource.strip_prefix("pricingplan/") else {
        return false;
    };
    account_id.len() == 12
        && account_id.bytes().all(|byte| byte.is_ascii_digit())
        && id.len() == 10
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// The bare resource id of a full ARN — the other spelling the modelled
/// `PricingPlanArn` pattern admits for the same plan.
fn bare_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
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

/// The `PricingPlans` a response reports (an absent member is an empty page).
fn elements(body: &Value) -> Vec<Value> {
    body.get("PricingPlans")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Arn` of every element, in the order they were reported.
fn arns(elements: &[Value]) -> Vec<String> {
    elements
        .iter()
        .map(|element| {
            element
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A member of an element, as a string (absent reads as `None`).
fn text(element: &Value, member: &str) -> Option<String> {
    element
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A member of an element, as an integer (absent reads as `None`).
fn integer(element: &Value, member: &str) -> Option<i64> {
    element.get(member).and_then(Value::as_i64)
}

/// `Size` as the element reports it. The member is modelled with `default: 0`, so a
/// plan that associates no rule may carry it explicitly or leave it absent; both spell
/// the same count.
fn size(element: &Value) -> i64 {
    integer(element, "Size").unwrap_or_default()
}

/// Does `value` have the shape of the modelled `BillingPeriod`
/// (`^\d{4}-(0?[1-9]|1[012])$`, i.e. `YYYY-MM` with a month in 1..=12)?
fn is_billing_period(value: &str) -> bool {
    let Some((year, month)) = value.split_once('-') else {
        return false;
    };
    year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && (1..=2).contains(&month.len())
        && month
            .parse::<u32>()
            .is_ok_and(|month| (1..=12).contains(&month))
}

/// One row of the filter table: the `Filters.Arns` a read sends, and the plan ARNs the
/// listing owes back for it, in ARN order.
struct FilterRow {
    case: &'static str,
    filter: Option<Vec<String>>,
    expected: Vec<String>,
}

/// One row of the read-repetition table: the optional members one read carries.
struct ReadRow {
    case: &'static str,
    filter: Option<Vec<String>>,
    max_results: Option<i64>,
    billing_period: Option<&'static str>,
}

/// One row of the create table: the request that persisted a plan, and the `Size` the
/// listing owes for it (its distinct rule associations).
struct PlanRow {
    case: &'static str,
    request: Value,
    expected_size: i64,
}

/// The table: rows differ along which optional members the create carries and how
/// many distinct pricing rules the plan associates.
fn plan_table() -> Vec<PlanRow> {
    let many: Vec<String> = (0..MAX_ASSOCIATIONS_PER_CREATE).map(rule_id).collect();
    vec![
        PlanRow {
            case: "the smallest request the model accepts",
            request: json!({ "Name": "Minimal" }),
            expected_size: 0,
        },
        PlanRow {
            case: "a description and no association",
            request: json!({ "Name": "Described", "Description": "a pricing plan" }),
            expected_size: 0,
        },
        PlanRow {
            case: "the empty description a length of 0 admits",
            request: json!({ "Name": "Empty.Description", "Description": "" }),
            expected_size: 0,
        },
        PlanRow {
            case: "one association, recorded as a bare rule id",
            request: json!({ "Name": "One_Rule", "PricingRuleArns": ["AAAAAAAAA1"] }),
            expected_size: 1,
        },
        PlanRow {
            case: "one association, recorded as a full rule ARN",
            request: json!({ "Name": "One+Rule+Arn",
                             "PricingRuleArns": [rule_arn("BBBBBBBBB2")] }),
            expected_size: 1,
        },
        PlanRow {
            case: "both spellings of ONE rule is ONE association",
            request: json!({ "Name": "Deduplicated",
                             "PricingRuleArns": ["CCCCCCCCC3", rule_arn("CCCCCCCCC3")] }),
            expected_size: 1,
        },
        PlanRow {
            case: "three associations, with tags beside them",
            request: json!({ "Name": "Three-Rules", "Description": "three",
                             "PricingRuleArns": ["DDDDDDDDD4", "EEEEEEEEE5", "FFFFFFFFF6"],
                             "Tags": { "team": "finance" } }),
            expected_size: 3,
        },
        PlanRow {
            case: "the modelled maximum of thirty associations",
            request: json!({ "Name": "Thirty@Rules", "PricingRuleArns": many }),
            expected_size: i64::try_from(MAX_ASSOCIATIONS_PER_CREATE)
                .expect("thirty fits in an i64"),
        },
    ]
}

/// RULE: every plan `CreatePricingPlan` persisted reads back out of state through
/// `ListPricingPlans` member for member — its minted ARN, its name, the description it
/// carries (and none when it carries none), the instants the create stamped, and a
/// `Size` counting the distinct pricing rules it associates — and the whole set is
/// reported in ARN order, which is the order the ids were minted in.
#[tokio::test]
async fn every_created_pricing_plan_reads_back_out_of_state_member_for_member() {
    let em = Emulator::new();
    let mut minted: Vec<String> = Vec::new();

    for row in plan_table() {
        let arn = em.create_plan(&row.request).await;
        assert!(
            !minted.contains(&arn),
            "case {:?}: expected a fresh ARN, got {arn:?} which was already minted",
            row.case
        );
        minted.push(arn.clone());

        let stored = em.stored_plan(&arn).await;
        assert_eq!(
            text(&stored, "Arn").as_deref(),
            Some(arn.as_str()),
            "case {:?}: expected the element to carry the minted ARN, got {stored}",
            row.case
        );
        assert_eq!(
            text(&stored, "Name"),
            row.request
                .get("Name")
                .and_then(Value::as_str)
                .map(str::to_string),
            "case {:?}: expected the stored Name, got {stored}",
            row.case
        );
        assert_eq!(
            text(&stored, "Description"),
            row.request
                .get("Description")
                .and_then(Value::as_str)
                .map(str::to_string),
            "case {:?}: expected the stored Description, got {stored}",
            row.case
        );
        assert_eq!(
            size(&stored),
            row.expected_size,
            "case {:?}: expected Size {} (the plan's distinct rule associations), got {stored}",
            row.case,
            row.expected_size
        );
        let created = integer(&stored, "CreationTime").unwrap_or_default();
        let modified = integer(&stored, "LastModifiedTime").unwrap_or_default();
        assert!(
            created > 0 && modified == created,
            "case {:?}: expected one creation instant in epoch millis, also reported as the \
             last modification of a plan nothing has modified, got CreationTime {created} and \
             LastModifiedTime {modified}",
            row.case
        );
        // `PricingPlanListElement` models no `Tags` member, so a create's tags are
        // stored (and readable through `ListTagsForResource`) but never reported here.
        assert_eq!(
            stored.get("Tags"),
            None,
            "case {:?}: PricingPlanListElement models no Tags member, got {stored}",
            row.case
        );
    }

    let mut expected = minted.clone();
    expected.sort();
    assert_eq!(
        arns(&em.list_all(None).await),
        expected,
        "expected every created plan to be reported in ARN order"
    );
}

/// RULE: `Filters.Arns` restricts the answer to exactly the plans it names, in either
/// spelling the modelled `PricingPlanArn` pattern allows; an ARN that names no plan of
/// this account selects nothing, and an absent filter selects every stored plan.
#[tokio::test]
async fn the_arn_filter_selects_exactly_the_plans_it_names() {
    let em = Emulator::new();
    let first = em.create_plan(&json!({ "Name": "Filter-First" })).await;
    let second = em
        .create_plan(&json!({ "Name": "Filter-Second", "PricingRuleArns": ["AAAAAAAAA1"] }))
        .await;
    let third = em.create_plan(&json!({ "Name": "Filter-Third" })).await;
    let mut all = vec![first.clone(), second.clone(), third.clone()];
    all.sort();
    let mut first_and_third = vec![first.clone(), third.clone()];
    first_and_third.sort();

    // Rows differ along how many ARNs the filter names, how each is spelled, and
    // which class the named ARN falls in.
    let rows: Vec<FilterRow> = vec![
        FilterRow {
            case: "no filter at all",
            filter: None,
            expected: all.clone(),
        },
        FilterRow {
            case: "one full ARN",
            filter: Some(vec![second.clone()]),
            expected: vec![second.clone()],
        },
        FilterRow {
            case: "one bare plan id",
            filter: Some(vec![bare_id(&second).to_string()]),
            expected: vec![second.clone()],
        },
        FilterRow {
            case: "both spellings of ONE plan name that one plan once",
            filter: Some(vec![second.clone(), bare_id(&second).to_string()]),
            expected: vec![second.clone()],
        },
        FilterRow {
            case: "two plans, named out of ARN order",
            filter: Some(vec![third.clone(), first.clone()]),
            expected: first_and_third.clone(),
        },
        FilterRow {
            case: "every stored plan, named explicitly",
            filter: Some(vec![third.clone(), first.clone(), second.clone()]),
            expected: all.clone(),
        },
        FilterRow {
            case: "a well-formed id no plan was ever minted for",
            filter: Some(vec!["ZZZZZZZZZ9".to_string()]),
            expected: vec![],
        },
        FilterRow {
            case: "a stored plan beside one that was never minted",
            filter: Some(vec![first.clone(), "ZZZZZZZZZ9".to_string()]),
            expected: vec![first.clone()],
        },
        FilterRow {
            case: "a plan ARN of ANOTHER account",
            filter: Some(vec![plan_arn_in("210987654321", bare_id(&second))]),
            expected: vec![],
        },
    ];

    for row in rows {
        let listed = em.list_all(row.filter.as_deref()).await;
        assert_eq!(
            arns(&listed),
            row.expected,
            "case {:?}: expected exactly {:?} in ARN order, got {listed:?}",
            row.case,
            row.expected
        );
    }

    // The plans Amazon Web Services owns belong to the account `aws` rather than to
    // the caller and this service stores no record of them, so they are not among this
    // account's plans and the listing reports neither.
    for managed in AWS_MANAGED_PLAN_IDS {
        for spelling in [(*managed).to_string(), plan_arn_in("aws", managed)] {
            let listed = em.list_all(Some(std::slice::from_ref(&spelling))).await;
            assert!(
                listed.is_empty(),
                "case {spelling:?}: an Amazon Web Services-managed plan is not one of this \
                 account's pricing plans, got {listed:?}"
            );
        }
    }
}

/// RULE: the reported `BillingPeriod` is the one the request asked about, and a
/// request that named none is not answered with one; which plans are reported does not
/// depend on it, since this emulator keeps no per-period history of a plan.
#[tokio::test]
async fn the_reported_period_is_the_one_the_request_named_and_otherwise_absent() {
    let em = Emulator::new();
    let plan = em.create_plan(&json!({ "Name": "Period-Plan" })).await;

    // Rows differ along the period asked for: a one-digit and a two-digit month, the
    // months at each end of the modelled pattern, a past year and a future one.
    for period in ["2020-01", "2026-9", "2026-09", "2031-12", "2031-1"] {
        let (status, body) = em.list(None, None, None, Some(period)).await;
        assert_eq!(
            status, 200,
            "case {period:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            text(&body, "BillingPeriod").as_deref(),
            Some(period),
            "case {period:?}: the response must name the period the request asked about, \
             got {body}"
        );
        assert!(
            is_billing_period(text(&body, "BillingPeriod").unwrap_or_default().as_str()),
            "case {period:?}: the reported period must satisfy the modelled YYYY-MM form, \
             got {body}"
        );
        assert_eq!(
            arns(&elements(&body)),
            vec![plan.clone()],
            "case {period:?}: the plans reported must not depend on the period, got {body}"
        );
    }

    let (status, body) = em.list(None, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"no BillingPeriod\": expected 200, got {status} with {body}"
    );
    assert_eq!(
        body.get("BillingPeriod"),
        None,
        "case \"no BillingPeriod\": a request that named no period must not be answered with \
         one, got {body}"
    );
    assert_eq!(
        arns(&elements(&body)),
        vec![plan],
        "case \"no BillingPeriod\": the plans must still be reported, got {body}"
    );
}

/// RULE: a page holds at most the requested `MaxResults` elements (at most the
/// server's own page size when the request names none) and the `NextToken` resumes at
/// the element the page left out, so following the tokens reports the whole set in ARN
/// order exactly once, whatever the page size.
#[tokio::test]
async fn every_page_size_pages_the_whole_plan_set_exactly_once() {
    let em = Emulator::new();
    let total = usize::try_from(MAX_RESULTS_CEILING).expect("the ceiling fits in usize") + 3;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..total {
        expected.push(em.create_plan(&json!({ "Name": format!("Paged-{index}") })).await);
    }
    expected.sort();

    // Rows differ along the page size: the modelled minimum, sizes that do and do not
    // divide the set, the modelled maximum, and no size at all (the server's own).
    for max_results in [Some(1), Some(2), Some(7), Some(51), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let limit = usize::try_from(max_results.unwrap_or(MAX_RESULTS_CEILING))
            .expect("a positive page size fits in usize");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (status, body) = em.list(None, max_results, token.as_deref(), None).await;
            assert_eq!(
                status, 200,
                "case {case}: page {pages}: expected 200, got {status} with {body}"
            );
            let page = elements(&body);
            assert!(
                !page.is_empty() && page.len() <= limit,
                "case {case}: page {pages}: expected 1..={limit} plans, got {}",
                page.len()
            );
            seen.extend(arns(&page));
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total,
                "case {case}: paging did not terminate: {pages} pages for {total} plans"
            );
        }
        assert_eq!(
            seen, expected,
            "case {case}: the pages must concatenate to the whole set in ARN order"
        );
    }
}

/// RULE: `MaxResults` is refused outside the modelled `MaxPricingPlanResults` range
/// (1..100) with this service's 400 `ValidationException`, and served at each bound.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_and_the_bounds_are_served() {
    let em = Emulator::new();
    em.create_plan(&json!({ "Name": "Bounds-Plan" })).await;

    // The published bounds of `MaxPricingPlanResults` (range 1..100) and one past each.
    for (max_results, expected_status) in [(1, 200), (100, 200), (0, 400), (101, 400)] {
        let (status, body) = em.list(None, Some(max_results), None, None).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected {expected_status}, got {status} with {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case MaxResults={max_results}: expected the modelled ValidationException, \
                 got {body}"
            );
        } else {
            assert_eq!(
                elements(&body).len(),
                1,
                "case MaxResults={max_results}: expected the one stored plan, got {body}"
            );
        }
    }
}

/// RULE: the listing REPORTS stored state, so it follows what the write operations
/// did: an update is reported under the plan's unchanged creation instant with a later
/// last-modified instant, and a deleted plan is reported by nobody while its
/// neighbours are untouched.
#[tokio::test]
async fn the_listing_follows_the_writes_that_moved_the_plans() {
    let em = Emulator::new();
    let kept = em
        .create_plan(&json!({ "Name": "Kept", "Description": "kept",
                              "PricingRuleArns": ["AAAAAAAAA1"] }))
        .await;
    let moved = em
        .create_plan(&json!({ "Name": "Moved", "Description": "before" }))
        .await;
    let doomed = em.create_plan(&json!({ "Name": "Doomed" })).await;

    let before = em.stored_plan(&moved).await;

    // Rows differ along which members the update names: the description alone, the
    // name alone, then both at once.
    for (case, patch, expected_name, expected_description) in [
        ("the description alone", json!({ "Description": "after" }), "Moved", "after"),
        ("the name alone", json!({ "Name": "Renamed" }), "Renamed", "after"),
        (
            "both members at once",
            json!({ "Name": "Renamed_Again", "Description": "last" }),
            "Renamed_Again",
            "last",
        ),
    ] {
        let mut body = json!({ "Arn": moved.clone() });
        for (member, value) in patch.as_object().expect("the patch is an object") {
            body[member] = value.clone();
        }
        let (status, answered) = em.call("PUT", UPDATE_PATH, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdatePricingPlan to be served, got {status} with {answered}"
        );

        let after = em.stored_plan(&moved).await;
        assert_eq!(
            text(&after, "Name").as_deref(),
            Some(expected_name),
            "case {case:?}: expected the updated Name to be reported, got {after}"
        );
        assert_eq!(
            text(&after, "Description").as_deref(),
            Some(expected_description),
            "case {case:?}: expected the updated Description to be reported, got {after}"
        );
        assert_eq!(
            integer(&after, "CreationTime"),
            integer(&before, "CreationTime"),
            "case {case:?}: an update must not move the creation instant, got {after}"
        );
        assert!(
            integer(&after, "LastModifiedTime").unwrap_or_default()
                >= integer(&before, "LastModifiedTime").unwrap_or_default(),
            "case {case:?}: expected the last-modified instant to follow the update, got {after}"
        );
        assert_eq!(
            size(&after),
            0,
            "case {case:?}: an update names no association, so Size must stay 0, got {after}"
        );
    }

    // The plan the create-time description and associations were never touched for is
    // still reported exactly as it was stored.
    let untouched = em.stored_plan(&kept).await;
    assert_eq!(
        (
            text(&untouched, "Name"),
            text(&untouched, "Description"),
            size(&untouched)
        ),
        (
            Some("Kept".to_string()),
            Some("kept".to_string()),
            1
        ),
        "case \"a neighbour of the updated plan\": expected the stored plan untouched, \
         got {untouched}"
    );

    let (status, answered) = em
        .call("POST", DELETE_PATH, &json!({ "Arn": doomed.clone() }))
        .await;
    assert_eq!(
        status, 200,
        "case \"delete\": expected DeletePricingPlan to be served, got {status} with {answered}"
    );
    let remaining = arns(&em.list_all(None).await);
    assert!(
        !remaining.contains(&doomed),
        "case \"delete\": a deleted plan must be reported by nobody, got {remaining:?}"
    );
    let mut expected = vec![kept, moved];
    expected.sort();
    assert_eq!(
        remaining, expected,
        "case \"delete\": expected the surviving plans in ARN order"
    );
    assert!(
        em.list_all(Some(std::slice::from_ref(&doomed)))
            .await
            .is_empty(),
        "case \"delete\": a filter naming the deleted plan must select nothing"
    );
}

/// RULE: the listing is a READ — it changes nothing a later caller sees, whatever it
/// is asked, and asking twice answers the same.
#[tokio::test]
async fn reading_the_plans_changes_nothing_a_later_caller_sees() {
    let em = Emulator::new();
    let first = em
        .create_plan(&json!({ "Name": "Read-First", "PricingRuleArns": ["AAAAAAAAA1"] }))
        .await;
    let second = em
        .create_plan(&json!({ "Name": "Read-Second", "Description": "second" }))
        .await;

    // Rows differ along the request: unfiltered, filtered, paged, and with a period.
    let requests: Vec<ReadRow> = vec![
        ReadRow {
            case: "the whole set",
            filter: None,
            max_results: None,
            billing_period: None,
        },
        ReadRow {
            case: "one plan by ARN",
            filter: Some(vec![first.clone()]),
            max_results: None,
            billing_period: None,
        },
        ReadRow {
            case: "one plan by bare id",
            filter: Some(vec![bare_id(&second).to_string()]),
            max_results: None,
            billing_period: None,
        },
        ReadRow {
            case: "a page of one",
            filter: None,
            max_results: Some(1),
            billing_period: None,
        },
        ReadRow {
            case: "a named billing period",
            filter: None,
            max_results: None,
            billing_period: Some("2026-07"),
        },
    ];

    let baseline = em.list_all(None).await;
    for row in requests {
        let (status, first_answer) = em
            .list(row.filter.as_deref(), row.max_results, None, row.billing_period)
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {first_answer}",
            row.case
        );
        let (_, second_answer) = em
            .list(row.filter.as_deref(), row.max_results, None, row.billing_period)
            .await;
        assert_eq!(
            first_answer, second_answer,
            "case {:?}: two identical reads must answer identically",
            row.case
        );
        assert_eq!(
            em.list_all(None).await,
            baseline,
            "case {:?}: the read must leave the stored plans exactly as they were",
            row.case
        );
    }
}

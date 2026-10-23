// SPDX-License-Identifier: CC0-1.0

//! The Savings Plan purchase lifecycle, end to end over the wire: search the
//! offering catalog, purchase an offering, describe the plan, tag it, list its
//! tags and return it.
//!
//! Every assertion here goes through the same `MultiServiceRouter` the emulator
//! boots, so what it pins is the HTTP behaviour (status, error code, body), not
//! an internal function's return value. The two exceptions are the clock tests
//! at the bottom, which drive `crate::state` directly so the *span* of a due
//! transition can be pinned without sleeping through it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// One answer from the emulator: the HTTP status, the wire error code (empty on
/// success) and the parsed body.
///
/// `pub(crate)` so a sibling suite in this test crate drives the emulator
/// through the same harness rather than standing up a second one.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) code: String,
    pub(crate) body: Value,
}

impl Answer {
    /// The body member `name`, or `Value::Null`.
    pub(crate) fn get(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    /// The body member `name` as an array.
    pub(crate) fn list(&self, name: &str) -> &[Value] {
        self.get(name).as_array().map_or(&[], Vec::as_slice)
    }
}

/// The emulator, with this one service mounted on the production router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing, so take the authenticator out: this suite is
        // about the service's behaviour, not about SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// `POST /<operation>` with a JSON body, routed by the SigV4 credential
    /// scope the way an SDK request routes.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/savingsplans/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer { status, code, body }
    }

    /// Every offering in the catalog (one unpaged call).
    async fn all_offerings(&self) -> Vec<Value> {
        let answer = self
            .call("DescribeSavingsPlansOfferings", json!({ "maxResults": 0 }))
            .await;
        assert_eq!(
            answer.status, 200,
            "unpaged offering search: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        answer.list("searchResults").to_vec()
    }

    /// An offering with this plan type, payment option and term. Plan types
    /// that are scoped to a region have one per region; the first is as good as
    /// any for a purchase.
    pub(crate) async fn offering(&self, plan_type: &str, payment_option: &str, duration: i64) -> Value {
        let answer = self
            .call(
                "DescribeSavingsPlansOfferings",
                json!({
                    "planTypes": [plan_type],
                    "paymentOptions": [payment_option],
                    "durations": [duration],
                    "maxResults": 0,
                }),
            )
            .await;
        let results = answer.list("searchResults");
        assert!(
            !results.is_empty(),
            "case {plan_type}/{payment_option}/{duration}: expected an offering, got {:?}",
            answer.body
        );
        results[0].clone()
    }

    /// Purchase `offering_id`, asserting the purchase succeeded.
    pub(crate) async fn purchase(&self, request: Value) -> String {
        let answer = self.call("CreateSavingsPlan", request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "purchase {request}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        answer
            .get("savingsPlanId")
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Describe the one plan with this id.
    pub(crate) async fn plan(&self, savings_plan_id: &str) -> Value {
        let answer = self
            .call(
                "DescribeSavingsPlans",
                json!({ "savingsPlanIds": [savings_plan_id] }),
            )
            .await;
        let plans = answer.list("savingsPlans");
        assert_eq!(
            plans.len(),
            1,
            "case {savings_plan_id}: expected exactly one plan, got {}",
            plans.len()
        );
        plans[0].clone()
    }
}

/// A string member of a JSON object, or `""`.
pub(crate) fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A decimal `Amount` member parsed back to a number, so a test can assert the
/// arithmetic rule rather than one spelling of its result.
fn amount(value: &Value, name: &str) -> f64 {
    text(value, name).parse().unwrap_or(f64::NAN)
}

/// The terms the catalog is seeded with, in seconds.
pub(crate) const ONE_YEAR: i64 = 31_536_000;
/// See [`ONE_YEAR`].
pub(crate) const THREE_YEARS: i64 = 94_608_000;

// ---------------------------------------------------------------------------
// The offering catalog
// ---------------------------------------------------------------------------

/// Every supplied filter narrows the catalog, and nothing else does.
///
/// The table's rows differ along what the rule generalises over — which filter,
/// how many values it names, and whether it reads a scalar member, a list
/// member or one of the offering's own properties. The expected result of each
/// row is computed from the unfiltered catalog by the same predicate, so the row
/// pins "the filter admits exactly these offerings" rather than a count someone
/// wrote down.
#[tokio::test]
async fn offering_search_applies_every_filter() {
    let emulator = Emulator::new();
    let catalog = emulator.all_offerings().await;
    assert!(
        catalog.len() > 20,
        "the seeded catalog must be a catalog: got {} offerings",
        catalog.len()
    );

    type Predicate = Box<dyn Fn(&Value) -> bool>;
    let has_property = |name: &'static str, wanted: &'static str| -> Predicate {
        Box::new(move |offering: &Value| {
            offering
                .get("properties")
                .and_then(Value::as_array)
                .is_some_and(|properties| {
                    properties.iter().any(|property| {
                        text(property, "name") == name && text(property, "value") == wanted
                    })
                })
        })
    };
    let cases: Vec<(&str, Value, Predicate)> = vec![
        (
            "planTypes with one value",
            json!({ "planTypes": ["Compute"], "maxResults": 0 }),
            Box::new(|o: &Value| text(o, "planType") == "Compute"),
        ),
        (
            "planTypes with two values",
            json!({ "planTypes": ["EC2Instance", "SageMaker"], "maxResults": 0 }),
            Box::new(|o: &Value| {
                matches!(text(o, "planType").as_str(), "EC2Instance" | "SageMaker")
            }),
        ),
        (
            "paymentOptions",
            json!({ "paymentOptions": ["All Upfront"], "maxResults": 0 }),
            Box::new(|o: &Value| text(o, "paymentOption") == "All Upfront"),
        ),
        (
            "durations",
            json!({ "durations": [THREE_YEARS], "maxResults": 0 }),
            Box::new(|o: &Value| o.get("durationSeconds").and_then(Value::as_i64) == Some(THREE_YEARS)),
        ),
        (
            "currencies",
            json!({ "currencies": ["EUR"], "maxResults": 0 }),
            Box::new(|o: &Value| text(o, "currency") == "EUR"),
        ),
        (
            "currencies with two values",
            json!({ "currencies": ["EUR", "CNY"], "maxResults": 0 }),
            Box::new(|o: &Value| matches!(text(o, "currency").as_str(), "EUR" | "CNY")),
        ),
        (
            "productType covered by one plan type",
            json!({ "productType": "Lambda", "maxResults": 0 }),
            Box::new(|o: &Value| {
                o.get("productTypes")
                    .and_then(Value::as_array)
                    .is_some_and(|types| types.iter().any(|t| t.as_str() == Some("Lambda")))
            }),
        ),
        (
            "productType covered by several plan types",
            json!({ "productType": "EC2", "maxResults": 0 }),
            Box::new(|o: &Value| {
                o.get("productTypes")
                    .and_then(Value::as_array)
                    .is_some_and(|types| types.iter().any(|t| t.as_str() == Some("EC2")))
            }),
        ),
        (
            "serviceCodes",
            json!({ "serviceCodes": ["AmazonSageMaker"], "maxResults": 0 }),
            Box::new(|o: &Value| text(o, "serviceCode") == "AmazonSageMaker"),
        ),
        (
            "operations",
            json!({ "operations": ["RunInstances"], "maxResults": 0 }),
            Box::new(|o: &Value| text(o, "operation") == "RunInstances"),
        ),
        (
            "usageTypes",
            json!({ "usageTypes": ["us-east-1-EC2SP:m5"], "maxResults": 0 }),
            Box::new(|o: &Value| text(o, "usageType") == "us-east-1-EC2SP:m5"),
        ),
        (
            "property filter on region",
            json!({
                "filters": [{ "name": "region", "values": ["cn-north-1"] }],
                "maxResults": 0,
            }),
            has_property("region", "cn-north-1"),
        ),
        (
            "property filter on instanceFamily",
            json!({
                "filters": [{ "name": "instanceFamily", "values": ["c5"] }],
                "maxResults": 0,
            }),
            has_property("instanceFamily", "c5"),
        ),
        (
            "several filters at once",
            json!({
                "planTypes": ["Compute"],
                "paymentOptions": ["No Upfront"],
                "durations": [ONE_YEAR],
                "maxResults": 0,
            }),
            Box::new(|o: &Value| {
                text(o, "planType") == "Compute"
                    && text(o, "paymentOption") == "No Upfront"
                    && o.get("durationSeconds").and_then(Value::as_i64) == Some(ONE_YEAR)
            }),
        ),
        (
            "a filter nothing satisfies",
            json!({
                "planTypes": ["Compute"],
                "filters": [{ "name": "instanceFamily", "values": ["m5"] }],
                "maxResults": 0,
            }),
            Box::new(|_: &Value| false),
        ),
    ];

    for (case, request, predicate) in cases {
        let answer = emulator
            .call("DescribeSavingsPlansOfferings", request.clone())
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let got = answer.list("searchResults");
        let expected: Vec<&Value> = catalog.iter().filter(|o| predicate(o)).collect();
        assert_eq!(
            got.len(),
            expected.len(),
            "case {case:?}: expected {} offerings, got {} ({request})",
            expected.len(),
            got.len()
        );
        for offering in got {
            assert!(
                predicate(offering),
                "case {case:?}: filter admitted an offering it should not: {offering:?}"
            );
        }
    }

    // One offering id addresses exactly one offering.
    let wanted = catalog
        .last()
        .expect("the catalog is not empty")
        .get("offeringId")
        .cloned()
        .expect("every offering carries an id");
    let answer = emulator
        .call(
            "DescribeSavingsPlansOfferings",
            json!({ "offeringIds": [wanted], "maxResults": 0 }),
        )
        .await;
    assert_eq!(
        answer.list("searchResults").len(),
        1,
        "case offeringIds: expected one offering for {wanted:?}, got {:?}",
        answer.body
    );
}

/// Every offering is well formed: the members the model declares are present
/// and carry values the model's own value sets and patterns admit.
#[tokio::test]
async fn every_offering_is_well_formed() {
    let emulator = Emulator::new();
    for offering in emulator.all_offerings().await {
        let id = text(&offering, "offeringId");
        assert!(
            id.len() == 36
                && id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase() || b == b'-'),
            "case {id:?}: offeringId must satisfy the model's UUID pattern"
        );
        // Value sets from the model's enums (SavingsPlanType, CurrencyCode,
        // SavingsPlanPaymentOption).
        assert!(
            matches!(
                text(&offering, "planType").as_str(),
                "Compute" | "EC2Instance" | "SageMaker" | "Database"
            ),
            "case {id:?}: unknown planType {:?}",
            text(&offering, "planType")
        );
        assert!(
            matches!(text(&offering, "currency").as_str(), "USD" | "EUR" | "CNY"),
            "case {id:?}: unknown currency {:?}",
            text(&offering, "currency")
        );
        assert!(
            matches!(
                text(&offering, "paymentOption").as_str(),
                "All Upfront" | "Partial Upfront" | "No Upfront"
            ),
            "case {id:?}: unknown paymentOption {:?}",
            text(&offering, "paymentOption")
        );
        let duration = offering.get("durationSeconds").and_then(Value::as_i64);
        assert!(
            duration == Some(ONE_YEAR) || duration == Some(THREE_YEARS),
            "case {id:?}: unexpected durationSeconds {duration:?}"
        );
        let description = text(&offering, "description");
        assert!(
            !description.is_empty()
                && description
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-' || b == b'_'),
            "case {id:?}: description {description:?} must satisfy the model's pattern"
        );
        assert!(
            !offering
                .get("productTypes")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .is_empty(),
            "case {id:?}: an offering must cover at least one product type"
        );
    }
}

/// `maxResults`/`nextToken` page the search without losing, duplicating or
/// reordering an offering, and a token this paginator did not issue is refused.
#[tokio::test]
async fn offering_search_pages_the_catalog() {
    let emulator = Emulator::new();
    let catalog: Vec<String> = emulator
        .all_offerings()
        .await
        .iter()
        .map(|o| text(o, "offeringId"))
        .collect();

    for page_size in [1_usize, 7, 50, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = &next {
                request["nextToken"] = json!(token);
            }
            let answer = emulator
                .call("DescribeSavingsPlansOfferings", request)
                .await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let results = answer.list("searchResults");
            next = answer
                .get("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if next.is_some() {
                assert_eq!(
                    results.len(),
                    page_size,
                    "case page size {page_size}: a page before the last must be full, got {}",
                    results.len()
                );
            }
            seen.extend(results.iter().map(|o| text(o, "offeringId")));
            pages += 1;
            assert!(
                pages <= catalog.len() + 1,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, catalog,
            "case page size {page_size}: paging must yield the catalog in catalog order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            catalog.len(),
            "case page size {page_size}: paging must not repeat an offering"
        );
    }

    for bad in ["zzzz", "AAAA", "ffff", "0102030405"] {
        let answer = emulator
            .call(
                "DescribeSavingsPlansOfferings",
                json!({ "maxResults": 1, "nextToken": bad }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case nextToken {bad:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code, "ValidationException",
            "case nextToken {bad:?}: expected ValidationException, got {:?}",
            answer.code
        );
    }
}

// ---------------------------------------------------------------------------
// Purchase
// ---------------------------------------------------------------------------

/// A purchase is visible to a later describe, with every member computed from
/// the offering it was bought from and the request that bought it.
#[tokio::test]
async fn a_purchase_round_trips_through_describe() {
    let emulator = Emulator::new();
    let offering = emulator.offering("Compute", "No Upfront", ONE_YEAR).await;
    let offering_id = text(&offering, "offeringId");

    let savings_plan_id = emulator
        .purchase(json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.100",
            "clientToken": "token-round-trip",
            "tags": { "env": "test" },
        }))
        .await;

    let plan = emulator.plan(&savings_plan_id).await;
    assert_eq!(
        text(&plan, "offeringId"),
        offering_id,
        "the plan must name the offering it was bought from, got {plan:?}"
    );
    assert_eq!(
        text(&plan, "savingsPlanArn"),
        format!("arn:aws:savingsplans::000000000000:savingsplan/{savings_plan_id}"),
        "the plan's ARN must be the canonical savingsplan ARN, got {plan:?}"
    );
    for (member, expected) in [
        ("description", text(&offering, "description")),
        ("savingsPlanType", "Compute".to_string()),
        ("paymentOption", "No Upfront".to_string()),
        ("currency", "USD".to_string()),
        // 0.1 an hour, nothing up front: the whole commitment recurs. Every
        // `Amount` is rendered at the service's one fixed scale.
        ("commitment", "0.10000000".to_string()),
        ("upfrontPaymentAmount", "0.00000000".to_string()),
        ("recurringPaymentAmount", "0.10000000".to_string()),
    ] {
        assert_eq!(
            text(&plan, member),
            expected,
            "case {member:?}: expected {expected:?}, got {:?}",
            text(&plan, member)
        );
    }
    assert_eq!(
        plan.get("termDurationInSeconds").and_then(Value::as_i64),
        Some(ONE_YEAR),
        "the plan's term must be the offering's, got {plan:?}"
    );
    assert_eq!(
        plan.get("productTypes"),
        offering.get("productTypes"),
        "the plan's product types must be the offering's, got {plan:?}"
    );
    assert_eq!(
        plan.get("tags"),
        Some(&json!({ "env": "test" })),
        "the tags the purchase carried must be readable back, got {plan:?}"
    );
    assert!(
        matches!(text(&plan, "state").as_str(), "payment-pending" | "active"),
        "a fresh purchase must be payment-pending or active, got {:?}",
        text(&plan, "state")
    );
    // A Compute Savings Plan is scoped to no region or instance family, so
    // neither member is reported.
    assert_eq!(
        plan.get("region"),
        None,
        "a Compute plan carries no region, got {plan:?}"
    );
    assert_eq!(
        plan.get("ec2InstanceFamily"),
        None,
        "a Compute plan carries no instance family, got {plan:?}"
    );
    // start, end and returnableUntil are ISO-8601 instants one term and one
    // hour apart.
    let start = text(&plan, "start");
    let end = text(&plan, "end");
    let returnable_until = text(&plan, "returnableUntil");
    for (member, value) in [("start", &start), ("end", &end), ("returnableUntil", &returnable_until)] {
        assert!(
            value.len() >= 20 && value.ends_with('Z') && value.contains('T'),
            "case {member:?}: expected an ISO-8601 UTC instant, got {value:?}"
        );
    }
    assert!(
        end > start,
        "the term must end after it starts, got start {start:?} end {end:?}"
    );
    assert!(
        returnable_until > start && returnable_until < end,
        "returnableUntil must be inside the term, got {returnable_until:?} for start {start:?}"
    );

    // A region-scoped plan type reports both of the members the Compute plan
    // leaves out.
    let ec2 = emulator
        .call(
            "DescribeSavingsPlansOfferings",
            json!({
                "planTypes": ["EC2Instance"],
                "paymentOptions": ["No Upfront"],
                "durations": [ONE_YEAR],
                "filters": [
                    { "name": "region", "values": ["eu-west-1"] },
                    { "name": "instanceFamily", "values": ["c5"] },
                ],
                "maxResults": 0,
            }),
        )
        .await;
    let ec2_offering = ec2.list("searchResults");
    assert_eq!(
        ec2_offering.len(),
        1,
        "expected one eu-west-1 c5 offering, got {:?}",
        ec2.body
    );
    let ec2_plan_id = emulator
        .purchase(json!({
            "savingsPlanOfferingId": text(&ec2_offering[0], "offeringId"),
            "commitment": "1",
        }))
        .await;
    let ec2_plan = emulator.plan(&ec2_plan_id).await;
    assert_eq!(
        (text(&ec2_plan, "region"), text(&ec2_plan, "ec2InstanceFamily"), text(&ec2_plan, "currency")),
        ("eu-west-1".to_string(), "c5".to_string(), "EUR".to_string()),
        "an EC2 Instance plan must report its region, family and currency, got {ec2_plan:?}"
    );
}

/// A purchase of an offering that is not in the catalog is a 404, and the
/// purchase does not happen.
#[tokio::test]
async fn purchasing_an_unknown_offering_is_not_found() {
    let emulator = Emulator::new();
    for case in [
        "00000000-0000-4000-8000-000000000000",
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
        "deadbeef",
    ] {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({ "savingsPlanOfferingId": case, "commitment": "0.1" }),
            )
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.code
        );
    }
    let answer = emulator.call("DescribeSavingsPlans", json!({})).await;
    assert!(
        answer.list("savingsPlans").is_empty(),
        "a refused purchase must not leave a plan behind, got {:?}",
        answer.body
    );
}

/// The same `clientToken` buys one plan, however often it is replayed; a
/// different one buys another.
#[tokio::test]
async fn a_replayed_client_token_buys_nothing_more() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "All Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let request = json!({
        "savingsPlanOfferingId": offering_id,
        "commitment": "0.5",
        "clientToken": "token-idempotent",
    });

    let first = emulator.purchase(request.clone()).await;
    for attempt in 1..=3 {
        // Deliberately separated in time: a replay is idempotent because the
        // REQUEST is the same, not because it arrived in the same millisecond.
        std::thread::sleep(Duration::from_millis(30));
        let again = emulator.purchase(request.clone()).await;
        assert_eq!(
            again, first,
            "case replay {attempt}: the same clientToken must answer with the same plan"
        );
    }
    let mut other = request.clone();
    other["clientToken"] = json!("token-idempotent-2");
    let second = emulator.purchase(other).await;
    assert_ne!(
        second, first,
        "a different clientToken must buy a second plan"
    );

    // The same token carrying a DIFFERENT purchase is refused, rather than
    // answered with the first plan — that would report success for a purchase
    // that never happened.
    let mut mismatched = request.clone();
    mismatched["commitment"] = json!("0.6");
    let answer = emulator.call("CreateSavingsPlan", mismatched).await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (400, "ValidationException"),
        "case replayed token with a different commitment: expected a 400 \
         ValidationException, got {} {:?}",
        answer.status,
        answer.body
    );
    let mut mismatched = request.clone();
    mismatched["tags"] = json!({ "env": "test" });
    let answer = emulator.call("CreateSavingsPlan", mismatched).await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (400, "ValidationException"),
        "case replayed token with different tags: expected a 400 \
         ValidationException, got {} {:?}",
        answer.status,
        answer.body
    );

    let answer = emulator.call("DescribeSavingsPlans", json!({})).await;
    assert_eq!(
        answer.list("savingsPlans").len(),
        2,
        "three replays, one fresh token and two refused mismatches must leave two plans, got {:?}",
        answer.body
    );
}

/// How a term is paid for follows the offering's payment option: everything up
/// front, nothing up front, or a whole-number share up front with the balance
/// spread over the term's hours. The rows vary the payment option, the term and
/// the commitment's magnitude and scale.
#[tokio::test]
async fn payment_amounts_follow_the_offerings_payment_option() {
    let emulator = Emulator::new();
    // (payment option, term, commitment, supplied up-front amount)
    let cases: [(&str, i64, &str, Option<&str>); 9] = [
        ("All Upfront", ONE_YEAR, "0.1", None),
        ("All Upfront", THREE_YEARS, "0.1", None),
        ("All Upfront", ONE_YEAR, "0.00123", None),
        ("No Upfront", ONE_YEAR, "0.1", None),
        ("No Upfront", THREE_YEARS, "1000000", None),
        ("Partial Upfront", ONE_YEAR, "0.1", None),
        ("Partial Upfront", THREE_YEARS, "0.1", None),
        ("Partial Upfront", ONE_YEAR, "0.001", None),
        ("Partial Upfront", ONE_YEAR, "0.1", Some("438")),
    ];

    for (payment_option, duration, commitment, supplied) in cases {
        let case = format!("{payment_option}/{duration}/{commitment}/{supplied:?}");
        let offering_id = text(
            &emulator.offering("Compute", payment_option, duration).await,
            "offeringId",
        );
        let mut request = json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": commitment,
        });
        if let Some(upfront) = supplied {
            request["upfrontPaymentAmount"] = json!(upfront);
        }
        let plan = emulator.plan(&emulator.purchase(request).await).await;

        let hourly: f64 = commitment.parse().expect("the case commitment parses");
        let hours = duration as f64 / 3600.0;
        let total = hourly * hours;
        let (expected_upfront, expected_recurring) = match payment_option {
            "All Upfront" => (total, 0.0),
            "No Upfront" => (0.0, hourly),
            _ => {
                let upfront = match supplied {
                    Some(value) => value.parse().expect("the case up-front amount parses"),
                    None => (total * 0.5).ceil(),
                };
                (upfront, (total - upfront) / hours)
            }
        };
        assert!(
            (amount(&plan, "commitment") - hourly).abs() < 1e-9,
            "case {case:?}: expected commitment {hourly}, got {:?}",
            text(&plan, "commitment")
        );
        assert!(
            (amount(&plan, "upfrontPaymentAmount") - expected_upfront).abs() < 1e-5,
            "case {case:?}: expected upfrontPaymentAmount {expected_upfront}, got {:?}",
            text(&plan, "upfrontPaymentAmount")
        );
        assert!(
            (amount(&plan, "recurringPaymentAmount") - expected_recurring).abs() < 1e-5,
            "case {case:?}: expected recurringPaymentAmount {expected_recurring}, got {:?}",
            text(&plan, "recurringPaymentAmount")
        );
    }
}

/// `commitment` is "a value between 0.001 and 1 million" with "no more than
/// five digits after the decimal point": each published bound is accepted and
/// one step past it is refused, as is anything that is not a decimal number.
#[tokio::test]
async fn commitment_is_validated_against_its_documented_range() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    // (commitment, accepted)
    let cases: [(&str, bool); 14] = [
        ("0.001", true),
        ("1000000", true),
        ("0.5", true),
        ("0.00001", false),
        ("0.00100", true),
        ("0.12345", true),
        ("0.0009", false),
        ("1000000.00001", false),
        ("0.123456", false),
        ("", false),
        ("abc", false),
        ("-1", false),
        ("1e5", false),
        ("1.2.3", false),
    ];

    for (commitment, accepted) in cases {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({ "savingsPlanOfferingId": offering_id, "commitment": commitment }),
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case commitment {commitment:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case commitment {commitment:?}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code, "ValidationException",
                "case commitment {commitment:?}: expected ValidationException, got {:?}",
                answer.code
            );
        }
    }
}

/// `upfrontPaymentAmount` is "a whole number between 50 and 99 percent of the
/// total value of the Savings Plan" and "is only supported if the payment option
/// is Partial Upfront".
#[tokio::test]
async fn upfront_payment_amount_is_validated_against_its_documented_rule() {
    let emulator = Emulator::new();
    // A one-year term at 0.1 an hour is a total value of 876, so the window is
    // 438 (50%) to 867.24 (99%).
    // (payment option, up-front amount, accepted)
    let cases: [(&str, &str, bool); 10] = [
        ("Partial Upfront", "438", true),
        ("Partial Upfront", "867", true),
        ("Partial Upfront", "600", true),
        ("Partial Upfront", "437", false),
        ("Partial Upfront", "868", false),
        ("Partial Upfront", "500.5", false),
        ("Partial Upfront", "abc", false),
        ("No Upfront", "438", false),
        ("All Upfront", "438", false),
        ("All Upfront", "0", false),
    ];

    for (payment_option, upfront, accepted) in cases {
        let case = format!("{payment_option}/{upfront}");
        let offering_id = text(
            &emulator.offering("Compute", payment_option, ONE_YEAR).await,
            "offeringId",
        );
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({
                    "savingsPlanOfferingId": offering_id,
                    "commitment": "0.1",
                    "upfrontPaymentAmount": upfront,
                }),
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected a 400 ValidationException, got {} {:?}",
                answer.status,
                answer.body
            );
        }
    }
}

/// A caller-supplied `purchaseTime` is bounded before it reaches the term and
/// return-window arithmetic: inside the window it is honoured, outside it the
/// purchase is refused and nothing is stored.
#[tokio::test]
async fn purchase_time_is_bounded() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let now_seconds = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_secs(),
    )
    .expect("an epoch that fits in i64");
    let decade = 10 * 365 * 24 * 60 * 60;

    // (case, purchaseTime in epoch seconds, accepted)
    let cases: [(&str, i64, bool); 9] = [
        ("now", now_seconds, true),
        ("tomorrow", now_seconds + 86_400, true),
        ("two hours ago", now_seconds - 7_200, true),
        ("just inside the window", now_seconds + decade - 60, true),
        ("just outside the window", now_seconds + decade + 60, false),
        ("a decade and more ago", now_seconds - decade - 60, false),
        ("the epoch", 0, false),
        ("near i64::MAX seconds", 9_000_000_000_000_000_000, false),
        ("near i64::MIN seconds", -9_000_000_000_000_000_000, false),
    ];

    let mut accepted = 0_usize;
    for (case, purchase_time, is_accepted) in cases {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({
                    "savingsPlanOfferingId": offering_id,
                    "commitment": "0.1",
                    "purchaseTime": purchase_time,
                }),
            )
            .await;
        if is_accepted {
            accepted += 1;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected a 400 ValidationException, got {} {:?}",
                answer.status,
                answer.body
            );
        }
        let described = emulator.call("DescribeSavingsPlans", json!({})).await;
        assert_eq!(
            described.list("savingsPlans").len(),
            accepted,
            "case {case:?}: expected {accepted} stored plans, got {:?}",
            described.body
        );
    }
}

/// The account's plan quota is the ceiling on the store: the purchase past it is
/// refused with the service's own `ServiceQuotaExceededException`, and the plans
/// already bought are untouched.
#[tokio::test]
async fn purchases_stop_at_the_plan_quota() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let request = json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.001" });

    for plan in 1..=state::MAX_PLANS_PER_ACCOUNT {
        let answer = emulator.call("CreateSavingsPlan", request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case plan {plan} of {}: expected 200, got {} {:?}",
            state::MAX_PLANS_PER_ACCOUNT,
            answer.status,
            answer.body
        );
    }
    for attempt in 1..=2 {
        let answer = emulator.call("CreateSavingsPlan", request.clone()).await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (402, "ServiceQuotaExceededException"),
            "case attempt {attempt} past the quota: expected a 402 \
             ServiceQuotaExceededException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
    let described = emulator
        .call(
            "DescribeSavingsPlans",
            json!({ "maxResults": state::MAX_PLANS_PER_ACCOUNT }),
        )
        .await;
    assert_eq!(
        described.list("savingsPlans").len(),
        state::MAX_PLANS_PER_ACCOUNT,
        "the quota must not disturb the plans already bought, got {}",
        described.list("savingsPlans").len()
    );
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// A tag written by `TagResource` is readable through `ListTagsForResource` and
/// through the plan's own `tags`, and an ARN that names no plan of ours is
/// refused.
#[tokio::test]
async fn tags_written_are_readable_back() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Database", "Partial Upfront", THREE_YEARS).await,
        "offeringId",
    );
    let savings_plan_id = emulator
        .purchase(json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.2",
            "tags": { "env": "test" },
        }))
        .await;
    let arn = text(&emulator.plan(&savings_plan_id).await, "savingsPlanArn");

    // Each row writes tags and states the whole tag set expected afterwards:
    // TagResource adds keys and overwrites the ones it names, and leaves the
    // rest alone.
    let cases: [(Value, Value); 4] = [
        (json!({ "owner": "qa" }), json!({ "env": "test", "owner": "qa" })),
        (json!({ "env": "prod" }), json!({ "env": "prod", "owner": "qa" })),
        (
            json!({ "a": "1", "b": "2" }),
            json!({ "env": "prod", "owner": "qa", "a": "1", "b": "2" }),
        ),
        (
            json!({}),
            json!({ "env": "prod", "owner": "qa", "a": "1", "b": "2" }),
        ),
    ];
    for (write, expected) in cases {
        let answer = emulator
            .call("TagResource", json!({ "resourceArn": arn, "tags": write }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {write:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let listed = emulator
            .call("ListTagsForResource", json!({ "resourceArn": arn }))
            .await;
        assert_eq!(
            listed.get("tags"),
            &expected,
            "case {write:?}: ListTagsForResource must report {expected:?}, got {:?}",
            listed.body
        );
        assert_eq!(
            emulator.plan(&savings_plan_id).await.get("tags"),
            Some(&expected),
            "case {write:?}: DescribeSavingsPlans must report {expected:?}"
        );
    }
}

/// An ARN that is not a Savings Plan ARN is a 400, and one that is but names no
/// plan is a 404 — for both tag operations.
#[tokio::test]
async fn tagging_an_arn_that_names_no_plan_is_refused() {
    let emulator = Emulator::new();
    // (resourceArn, expected status)
    let cases: [(&str, u16); 7] = [
        // Well formed, but no such plan.
        ("arn:aws:savingsplans::000000000000:savingsplan/00000000-0000-4000-8000-000000000000", 404),
        ("arn:aws:savingsplans:us-east-1:000000000000:savingsplan/11111111-2222-4333-8444-555555555555", 404),
        // Well formed under the model's pattern, but another service's ARN.
        ("arn:aws:ec2::000000000000:savingsplan/00000000-0000-4000-8000-000000000000", 400),
        // Refused by the model's own pattern.
        ("not-an-arn", 400),
        ("arn:aws:savingsplans::000000000000:savingsplan/not-a-uuid", 400),
        ("arn:aws:savingsplans::0:savingsplan/00000000-0000-4000-8000-000000000000", 400),
        ("", 400),
    ];

    for (arn, expected) in cases {
        for operation in ["TagResource", "ListTagsForResource"] {
            let mut request = json!({ "resourceArn": arn });
            if operation == "TagResource" {
                request["tags"] = json!({ "k": "v" });
            }
            let answer = emulator.call(operation, request).await;
            assert_eq!(
                answer.status, expected,
                "case {operation} {arn:?}: expected {expected}, got {} {:?}",
                answer.status, answer.body
            );
            let expected_code = if expected == 404 {
                "ResourceNotFoundException"
            } else {
                "ValidationException"
            };
            assert_eq!(
                answer.code, expected_code,
                "case {operation} {arn:?}: expected {expected_code}, got {:?}",
                answer.code
            );
        }
    }
}

/// The per-plan tag store is bounded the way the plan store is: a write that
/// would take the plan past the service's tag limit, or that carries an
/// over-long key or value, is refused whole and leaves the stored tags alone.
#[tokio::test]
async fn tag_writes_stop_at_the_tag_limit() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let savings_plan_id = emulator
        .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.1" }))
        .await;
    let arn = text(&emulator.plan(&savings_plan_id).await, "savingsPlanArn");

    // Fill the plan to the limit one tag at a time.
    for tag in 0..state::MAX_TAGS_PER_PLAN {
        let answer = emulator
            .call(
                "TagResource",
                json!({ "resourceArn": arn, "tags": { format!("k{tag}"): "v" } }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case tag {tag} of {}: expected 200, got {} {:?}",
            state::MAX_TAGS_PER_PLAN,
            answer.status,
            answer.body
        );
    }
    let at_limit = emulator
        .call("ListTagsForResource", json!({ "resourceArn": arn }))
        .await
        .get("tags")
        .clone();
    assert_eq!(
        at_limit.as_object().map(serde_json::Map::len),
        Some(state::MAX_TAGS_PER_PLAN),
        "expected {} stored tags, got {at_limit:?}",
        state::MAX_TAGS_PER_PLAN
    );

    // (case, tags) — each refused, and each leaving the stored set untouched.
    let long_key = "k".repeat(state::MAX_TAG_KEY_CHARS + 1);
    let long_value = "v".repeat(state::MAX_TAG_VALUE_CHARS + 1);
    let mut one_too_many = serde_json::Map::new();
    for tag in 0..=state::MAX_TAGS_PER_PLAN {
        one_too_many.insert(format!("fresh{tag}"), json!("v"));
    }
    let cases: [(&str, Value); 4] = [
        ("one more key than the limit allows", json!({ "one-more": "v" })),
        (
            "a whole request over the limit at once",
            Value::Object(one_too_many),
        ),
        ("an over-long key", json!({ long_key.clone(): "v" })),
        ("an over-long value", json!({ "short": long_value.clone() })),
    ];
    for (case, tags) in cases {
        let answer = emulator
            .call("TagResource", json!({ "resourceArn": arn, "tags": tags }))
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
        let after = emulator
            .call("ListTagsForResource", json!({ "resourceArn": arn }))
            .await
            .get("tags")
            .clone();
        assert_eq!(
            after, at_limit,
            "case {case:?}: a refused tag write must leave the stored tags unchanged"
        );
    }

    // Overwriting a key that is already there is not growth, so it is accepted
    // even at the limit.
    let answer = emulator
        .call("TagResource", json!({ "resourceArn": arn, "tags": { "k0": "replaced" } }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case overwrite at the limit: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        emulator
            .call("ListTagsForResource", json!({ "resourceArn": arn }))
            .await
            .get("tags")
            .get("k0")
            .and_then(Value::as_str),
        Some("replaced"),
        "case overwrite at the limit: the value must be the one just written"
    );

    // And a purchase is held to the same rule.
    for (case, tags) in [
        ("over-long key at purchase", json!({ long_key: "v" })),
        ("over-long value at purchase", json!({ "short": long_value })),
    ] {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({
                    "savingsPlanOfferingId": offering_id,
                    "commitment": "0.1",
                    "tags": tags,
                }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Return
// ---------------------------------------------------------------------------

/// A return moves the plan out of the state it was returnable from, and that is
/// what a later describe reports. The rows vary how the plan was purchased, so
/// they cover each state a return is accepted from and each one it is refused
/// from.
#[tokio::test]
async fn returning_a_plan_moves_it_out_of_its_returnable_state() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let now_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs() as i64;

    // (case, purchaseTime offset in seconds, expected status, the states the
    // plan may be in afterwards — a refused return leaves the plan where it
    // was, which is payment-pending until its own activation span elapses —
    // and whether `returnableUntil` falls back to the plan's start time)
    type ReturnCase = (
        &'static str,
        Option<i64>,
        u16,
        &'static [&'static str],
        bool,
    );
    let cases: [ReturnCase; 5] = [
        ("bought now", None, 200, &["returned"], true),
        ("bought a minute ago", Some(-60), 200, &["returned"], true),
        // Outside the 7-day (168-hour) window AWS documents for a return.
        (
            "bought eight days ago",
            Some(-8 * 86_400),
            400,
            &["payment-pending", "active"],
            true,
        ),
        // Past the end of its one-year term, so the clock already retired it.
        (
            "bought two years ago",
            Some(-2 * 365 * 86_400),
            400,
            &["retired"],
            true,
        ),
        // Not started yet: a queued purchase is cancelled through
        // DeleteQueuedSavingsPlan, not returned, so the return is refused and
        // the plan keeps the window its start will open.
        ("queued for tomorrow", Some(86_400), 400, &["queued"], false),
    ];

    for (case, offset, expected_status, expected_states, returnable_until_is_start) in cases {
        let mut request = json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.1",
        });
        if let Some(offset) = offset {
            request["purchaseTime"] = json!(now_seconds + offset);
        }
        let savings_plan_id = emulator.purchase(request).await;

        let answer = emulator
            .call(
                "ReturnSavingsPlan",
                json!({ "savingsPlanId": savings_plan_id }),
            )
            .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status}, got {} {:?}",
            answer.status, answer.body
        );
        if expected_status == 200 {
            assert_eq!(
                answer.get("savingsPlanId").as_str(),
                Some(savings_plan_id.as_str()),
                "case {case:?}: the return must answer with the plan it returned, got {:?}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.code, "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.code
            );
        }
        let plan = emulator.plan(&savings_plan_id).await;
        let state = text(&plan, "state");
        assert!(
            expected_states.contains(&state.as_str()),
            "case {case:?}: expected the plan to be one of {expected_states:?}, got {state:?}"
        );
        // A plan that cannot be returned any more — two have been and two are
        // outside the states a return is accepted from — reports its start
        // time; one whose window is still ahead of it reports when that window
        // closes.
        if returnable_until_is_start {
            assert_eq!(
                text(&plan, "returnableUntil"),
                text(&plan, "start"),
                "case {case:?}: a plan that cannot be returned reports its start time, got {plan:?}"
            );
        } else {
            assert!(
                text(&plan, "returnableUntil") > text(&plan, "start"),
                "case {case:?}: a plan whose return window is still open reports when it \
                 closes, got {plan:?}"
            );
        }
    }
}

/// Returning a plan twice is refused, and a replayed `clientToken` answers with
/// the first return rather than returning again.
#[tokio::test]
async fn a_plan_cannot_be_returned_twice() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("SageMaker", "All Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // Without a token, the second return is refused.
    let plain = emulator
        .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.1" }))
        .await;
    let first = emulator
        .call("ReturnSavingsPlan", json!({ "savingsPlanId": plain }))
        .await;
    assert_eq!(
        first.status, 200,
        "case first return: expected 200, got {} {:?}",
        first.status, first.body
    );
    let second = emulator
        .call("ReturnSavingsPlan", json!({ "savingsPlanId": plain }))
        .await;
    assert_eq!(
        (second.status, second.code.as_str()),
        (400, "ValidationException"),
        "case second return: expected a 400 ValidationException, got {} {:?}",
        second.status,
        second.body
    );

    // With one, the replay is idempotent.
    let tokened = emulator
        .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.1" }))
        .await;
    let request = json!({ "savingsPlanId": tokened, "clientToken": "token-return" });
    for attempt in 1..=3 {
        let answer = emulator.call("ReturnSavingsPlan", request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case replay {attempt}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("savingsPlanId").as_str(),
            Some(tokened.as_str()),
            "case replay {attempt}: expected the same plan back, got {:?}",
            answer.body
        );
    }
    assert_eq!(
        text(&emulator.plan(&tokened).await, "state"),
        "returned",
        "a replayed return must leave the plan returned"
    );
}

/// Returning a plan that was never purchased is a 404.
#[tokio::test]
async fn returning_an_unknown_plan_is_not_found() {
    let emulator = Emulator::new();
    for case in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-plan",
        "a",
    ] {
        let answer = emulator
            .call("ReturnSavingsPlan", json!({ "savingsPlanId": case }))
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Describe: filters and paging over the plans actually purchased
// ---------------------------------------------------------------------------

/// `DescribeSavingsPlans` reports the plans actually purchased, narrowed by its
/// id, ARN, state and attribute filters and paged by `maxResults`/`nextToken`.
#[tokio::test]
async fn describe_plans_filters_and_pages() {
    let emulator = Emulator::new();
    let mut ids = Vec::new();
    // Purchase across plan types, payment options and terms so the filters have
    // something to tell apart.
    for (plan_type, payment_option, duration) in [
        ("Compute", "All Upfront", ONE_YEAR),
        ("Compute", "No Upfront", THREE_YEARS),
        ("EC2Instance", "Partial Upfront", ONE_YEAR),
        ("SageMaker", "No Upfront", ONE_YEAR),
        ("Database", "All Upfront", THREE_YEARS),
    ] {
        let offering = if plan_type == "EC2Instance" {
            let answer = emulator
                .call(
                    "DescribeSavingsPlansOfferings",
                    json!({
                        "planTypes": [plan_type],
                        "paymentOptions": [payment_option],
                        "durations": [duration],
                        "filters": [
                            { "name": "region", "values": ["us-east-1"] },
                            { "name": "instanceFamily", "values": ["m5"] },
                        ],
                        "maxResults": 0,
                    }),
                )
                .await;
            answer.list("searchResults")[0].clone()
        } else {
            emulator
                .offering(plan_type, payment_option, duration)
                .await
        };
        ids.push(
            emulator
                .purchase(json!({
                    "savingsPlanOfferingId": text(&offering, "offeringId"),
                    "commitment": "0.1",
                }))
                .await,
        );
    }

    let all = emulator.call("DescribeSavingsPlans", json!({})).await;
    let plans = all.list("savingsPlans").to_vec();
    assert_eq!(
        plans.len(),
        ids.len(),
        "every purchase must be described, got {:?}",
        all.body
    );
    assert_eq!(
        plans
            .iter()
            .map(|p| text(p, "savingsPlanId"))
            .collect::<Vec<_>>(),
        ids,
        "plans must be described in purchase order"
    );

    type Predicate = Box<dyn Fn(&Value) -> bool>;
    let cases: Vec<(&str, Value, Predicate)> = vec![
        (
            "savingsPlanIds",
            json!({ "savingsPlanIds": [ids[0], ids[3]] }),
            Box::new({
                let wanted = [ids[0].clone(), ids[3].clone()];
                move |p: &Value| wanted.contains(&text(p, "savingsPlanId"))
            }),
        ),
        (
            "savingsPlanArns",
            json!({ "savingsPlanArns": [text(&plans[1], "savingsPlanArn")] }),
            Box::new({
                let wanted = text(&plans[1], "savingsPlanArn");
                move |p: &Value| text(p, "savingsPlanArn") == wanted
            }),
        ),
        (
            "filter on savings-plan-type",
            json!({ "filters": [{ "name": "savings-plan-type", "values": ["Compute"] }] }),
            Box::new(|p: &Value| text(p, "savingsPlanType") == "Compute"),
        ),
        (
            "filter on payment-option with two values",
            json!({ "filters": [{
                "name": "payment-option",
                "values": ["All Upfront", "Partial Upfront"],
            }] }),
            Box::new(|p: &Value| {
                matches!(
                    text(p, "paymentOption").as_str(),
                    "All Upfront" | "Partial Upfront"
                )
            }),
        ),
        (
            "filter on term",
            json!({ "filters": [{ "name": "term", "values": [THREE_YEARS.to_string()] }] }),
            Box::new(|p: &Value| {
                p.get("termDurationInSeconds").and_then(Value::as_i64) == Some(THREE_YEARS)
            }),
        ),
        (
            "filter on region",
            json!({ "filters": [{ "name": "region", "values": ["us-east-1"] }] }),
            Box::new(|p: &Value| text(p, "region") == "us-east-1"),
        ),
        (
            "filter on ec2-instance-family",
            json!({ "filters": [{ "name": "ec2-instance-family", "values": ["m5"] }] }),
            Box::new(|p: &Value| text(p, "ec2InstanceFamily") == "m5"),
        ),
        (
            "filter on commitment",
            json!({ "filters": [{ "name": "commitment", "values": ["0.1"] }] }),
            Box::new(|p: &Value| text(p, "commitment") == "0.10000000"),
        ),
        (
            "filter on commitment spelled differently",
            json!({ "filters": [{ "name": "commitment", "values": ["0.100"] }] }),
            Box::new(|p: &Value| text(p, "commitment") == "0.10000000"),
        ),
        (
            "filter on commitment spelled at the scale the service renders",
            json!({ "filters": [{ "name": "commitment", "values": ["0.10000000"] }] }),
            Box::new(|p: &Value| text(p, "commitment") == "0.10000000"),
        ),
        (
            "filter on start",
            json!({ "filters": [{
                "name": "start",
                "values": [text(&plans[0], "start")],
            }] }),
            Box::new({
                let wanted = text(&plans[0], "start");
                move |p: &Value| text(p, "start") == wanted
            }),
        ),
        (
            "filter on start spelled with an offset",
            json!({ "filters": [{
                "name": "start",
                // The same instant, written in +00:00 rather than Z.
                "values": [text(&plans[0], "start").replace('Z', "+00:00")],
            }] }),
            Box::new({
                let wanted = text(&plans[0], "start");
                move |p: &Value| text(p, "start") == wanted
            }),
        ),
        (
            "filter nothing satisfies",
            json!({ "filters": [{ "name": "region", "values": ["ap-south-1"] }] }),
            Box::new(|_: &Value| false),
        ),
        (
            "states",
            json!({ "states": ["payment-pending", "active"] }),
            Box::new(|p: &Value| {
                matches!(text(p, "state").as_str(), "payment-pending" | "active")
            }),
        ),
        (
            "states nothing is in",
            json!({ "states": ["retired", "payment-failed"] }),
            Box::new(|_: &Value| false),
        ),
    ];

    for (case, request, predicate) in cases {
        let answer = emulator.call("DescribeSavingsPlans", request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let got = answer.list("savingsPlans");
        let expected = plans.iter().filter(|p| predicate(p)).count();
        assert_eq!(
            got.len(),
            expected,
            "case {case:?}: expected {expected} plans, got {} ({request})",
            got.len()
        );
        for plan in got {
            assert!(
                predicate(plan),
                "case {case:?}: filter admitted a plan it should not: {plan:?}"
            );
        }
    }

    for page_size in [1_usize, 2, 5] {
        let mut seen = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = &next {
                request["nextToken"] = json!(token);
            }
            let answer = emulator.call("DescribeSavingsPlans", request).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            seen.extend(
                answer
                    .list("savingsPlans")
                    .iter()
                    .map(|p| text(p, "savingsPlanId")),
            );
            next = answer
                .get("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if next.is_none() {
                break;
            }
            assert!(
                seen.len() <= ids.len(),
                "case page size {page_size}: paging did not terminate, saw {}",
                seen.len()
            );
        }
        assert_eq!(
            seen, ids,
            "case page size {page_size}: paging must yield every plan once, in purchase order"
        );
    }

    // A token this paginator did not issue is refused — including one the
    // offering paginator issued.
    let offerings_token = emulator
        .call(
            "DescribeSavingsPlansOfferings",
            json!({ "maxResults": 1 }),
        )
        .await
        .get("nextToken")
        .as_str()
        .expect("the offering search pages")
        .to_string();
    for bad in [offerings_token.as_str(), "zzzz", "abcd"] {
        let answer = emulator
            .call(
                "DescribeSavingsPlans",
                json!({ "maxResults": 1, "nextToken": bad }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case nextToken {bad:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// A purchase is never *instantly* active: the move to `active` is owed a span
/// after the call, so a caller that describes the plan straight away still sees
/// `payment-pending`, and a queued purchase waits for its purchase time.
///
/// Driven through the store rather than the wire, so the span itself is pinned
/// without a test sleeping through it.
#[test]
fn a_purchase_becomes_active_only_once_the_clock_owes_it() {
    let mut store = state::ComAmazonawsSavingsplansAwssavingsplanState::default();
    let offering = store.offerings()[0].clone();
    let now = 1_777_000_000_000_i64;

    // (case, purchase time, the state right away, when it first becomes active)
    let cases: [(&str, i64, &str, i64); 3] = [
        (
            "bought now",
            now,
            state::STATE_PAYMENT_PENDING,
            now + state::ACTIVATION_DELAY_MILLIS,
        ),
        (
            "bought an hour ago",
            now - 3_600_000,
            state::STATE_PAYMENT_PENDING,
            now + state::ACTIVATION_DELAY_MILLIS,
        ),
        (
            "queued for ten seconds' time",
            now + 10_000,
            state::STATE_QUEUED,
            now + 10_000 + state::ACTIVATION_DELAY_MILLIS,
        ),
    ];

    for (case, purchase_millis, expected_now, active_at) in cases {
        let savings_plan_id = format!("plan-{case}");
        let record = state::purchase(
            &offering,
            savings_plan_id.clone(),
            // 0.1 an hour, nothing up front, 0.1 an hour recurring, in
            // AMOUNT_SCALE units.
            state::AMOUNT_SCALE / 10,
            0,
            state::AMOUNT_SCALE / 10,
            purchase_millis,
            now,
            BTreeMap::new(),
        );
        assert_eq!(
            record.state, expected_now,
            "case {case:?}: expected {expected_now:?} right after the purchase, got {:?}",
            record.state
        );
        store.insert_plan(record);

        store.materialize_due(active_at - 1);
        let state_before = store
            .plan(&savings_plan_id)
            .map(|plan| plan.state.clone())
            .unwrap_or_default();
        assert_ne!(
            state_before,
            state::STATE_ACTIVE,
            "case {case:?}: the plan must not be active before {active_at}, got {state_before:?}"
        );

        store.materialize_due(active_at);
        let state_after = store
            .plan(&savings_plan_id)
            .map(|plan| plan.state.clone())
            .unwrap_or_default();
        assert_eq!(
            state_after,
            state::STATE_ACTIVE,
            "case {case:?}: the plan must be active at {active_at}, got {state_after:?}"
        );

        // And the clock owes it one more transition: the term ends, so the plan
        // retires — the instant before, it is still active.
        let end = store
            .plan(&savings_plan_id)
            .map(|plan| plan.end_millis)
            .expect("the plan is stored");
        store.materialize_due(end - 1);
        let before_end = store
            .plan(&savings_plan_id)
            .map(|plan| plan.state.clone())
            .unwrap_or_default();
        assert_eq!(
            before_end,
            state::STATE_ACTIVE,
            "case {case:?}: the plan must still be active one millisecond before {end}, \
             got {before_end:?}"
        );
        store.materialize_due(end);
        let at_end = store
            .plan(&savings_plan_id)
            .map(|plan| plan.state.clone())
            .unwrap_or_default();
        assert_eq!(
            at_end,
            state::STATE_RETIRED,
            "case {case:?}: the plan must be retired once its term ends at {end}, got {at_end:?}"
        );
    }
}

/// And the same transition is observable over the wire: a plan described after
/// the activation span has elapsed is `active`, without anybody updating it.
#[tokio::test]
async fn an_activated_plan_is_reported_active() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let savings_plan_id = emulator
        .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.1" }))
        .await;

    // `tokio`'s timer is not enabled in this workspace, and a test thread may
    // block: the wait is the activation span plus margin.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(state::ACTIVATION_DELAY_MILLIS).unwrap_or(500) + 250,
    ));

    let state_after = text(&emulator.plan(&savings_plan_id).await, "state");
    assert_eq!(
        state_after, "active",
        "a plan past its activation span must be reported active, got {state_after:?}"
    );
}

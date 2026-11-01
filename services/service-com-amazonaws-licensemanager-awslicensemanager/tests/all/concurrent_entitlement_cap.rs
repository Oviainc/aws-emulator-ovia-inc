// SPDX-License-Identifier: CC0-1.0

//! The entitlement cap `CheckoutLicense` holds when callers RACE, driven as
//! awsJson1_1 requests (`POST /`, `X-Amz-Target: AWSLicenseManager.<Operation>`)
//! through the same `MultiServiceRouter` the production emulator mounts, with every
//! caller of a burst in flight together on a multi-threaded runtime.
//!
//! The rules under test:
//!
//! * a checkout's decision and the consumption it writes are ONE step over the
//!   entitlement's units, so however many callers ask at once, the units admitted
//!   never exceed the maximum count the entitlement caps: exactly as many callers
//!   win as the cap has room for, each with its own consumption token, and the rest
//!   are refused `NoEntitlementsAllowedException` (HTTP 400) — an entitlement that
//!   allows overage or caps no maximum count admits every caller;
//! * the counter the race is decided on is the one `GetLicenseUsage` reports and
//!   `CheckInLicense` releases, so after a burst the usage reports the units the
//!   winners hold, after the winners check in it reports none, and a second burst is
//!   admitted exactly as the first was;
//! * one `ClientToken` is one grant however many callers present it at once: the
//!   repeat is answered with the grant the first call minted and consumes nothing
//!   more.
//!
//! Each row of the tables differs in what those rules generalise over: how many
//! callers race, the entitlement's unit, whether it caps a maximum count and allows
//! overage, and how many units one caller asks for — so the cap is exercised where
//! it divides the requests evenly and where it does not.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.licensemanager#AWSLicenseManager`.
const TARGET_PREFIX: &str = "AWSLicenseManager";

/// The region the harness signs for.
const SIGNING_REGION: &str = "us-east-1";

/// The provisional time to live every license this suite creates carries, in
/// minutes, which is what bounds a `PROVISIONAL` checkout of it.
const PROVISIONAL_MINUTES: i64 = 60;

/// The validity end every license this suite creates carries.
const VALIDITY_END: &str = "2099-12-31T23:59:59Z";

/// The beneficiary every license and checkout of this suite names.
const BENEFICIARY: &str = "arn:aws:iam::000000000000:root";

/// The one entitlement every license of this suite declares and every checkout of it
/// asks for.
const ENTITLEMENT: &str = "sim2real-seats";

/// The wire error code a checkout past the cap is refused with: the modelled
/// `NoEntitlementsAllowedException`
/// (`smithy_query(query="operation",operation="CheckoutLicense")`).
const NO_ENTITLEMENTS_ALLOWED_CODE: &str = "NoEntitlementsAllowedException";

/// The HTTP status that error carries.
const NO_ENTITLEMENTS_ALLOWED_STATUS: u16 = 400;

/// The message AWS words a `NoEntitlementsAllowedException` with: the fixed text of
/// the class, carrying nothing from the request.
const NO_ENTITLEMENTS_ALLOWED: &str = "No Entitlements Allowed.";

/// One emulator: a single shared engine behind the router, exactly as `core/router`
/// assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_licensemanager_awslicensemanager::EmulatedService::new(engine),
    );
    router
}

/// Send one awsJson1_1 request and return its status and body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{SIGNING_REGION}/license-manager/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of a failure body: the last segment of awsJson's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.split('#').next_back().unwrap_or(t).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// The message a refusal body carries, under either casing the protocols use.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The entitlement a row declares on its license.
fn declared_entitlement(unit: &str, max_count: Option<i64>, overage: bool) -> Value {
    let mut entitlement = Map::new();
    entitlement.insert("Name".to_string(), json!(ENTITLEMENT));
    entitlement.insert("Unit".to_string(), json!(unit));
    if let Some(max_count) = max_count {
        entitlement.insert("MaxCount".to_string(), json!(max_count));
    }
    entitlement.insert("Overage".to_string(), json!(overage));
    entitlement.insert("AllowCheckIn".to_string(), json!(true));
    Value::Object(entitlement)
}

/// The license of the product `sku` a row's burst is taken on: its ARN and the issuer
/// key fingerprint a checkout of it has to present, read back through `GetLicense`
/// rather than assumed.
async fn create_license(
    router: &MultiServiceRouter,
    case: &str,
    sku: &str,
    entitlement: Value,
) -> (String, String) {
    let body = json!({
        "LicenseName": format!("license-{sku}"),
        "ProductName": "Concurrent Checkout Suite",
        "ProductSKU": sku,
        "Issuer": {"Name": format!("issuer-{sku}")},
        "HomeRegion": SIGNING_REGION,
        "Validity": {"Begin": "2026-01-01T00:00:00Z", "End": VALIDITY_END},
        "Entitlements": [entitlement],
        "Beneficiary": BENEFICIARY,
        "ConsumptionConfiguration": {
            "ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": PROVISIONAL_MINUTES},
        },
        "ClientToken": format!("create-{sku}"),
    });
    let (status, response) = call(router, "CreateLicense", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLicense to answer 200, got {status} {response}"
    );
    let arn = response["LicenseArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateLicense answered no LicenseArn: {response}"))
        .to_string();

    let (status, read) = call(router, "GetLicense", &json!({"LicenseArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetLicense to answer 200, got {status} {read}"
    );
    let fingerprint = read["License"]["Issuer"]["KeyFingerprint"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetLicense reported no KeyFingerprint: {read}"))
        .to_string();
    (arn, fingerprint)
}

/// One checkout request body, asking for `units` of this suite's entitlement.
fn checkout_body(
    sku: &str,
    fingerprint: &str,
    unit: &str,
    units: i64,
    client_token: &str,
) -> Value {
    json!({
        "ProductSKU": sku,
        "KeyFingerprint": fingerprint,
        "CheckoutType": "PROVISIONAL",
        "ClientToken": client_token,
        "Beneficiary": BENEFICIARY,
        "Entitlements": [{
            "Name": ENTITLEMENT,
            "Unit": unit,
            "Value": units.to_string(),
        }],
    })
}

/// What one caller of a burst was answered: a grant carrying its consumption token,
/// or a refusal reduced to the three things the rule fixes — the HTTP status, the
/// wire error code and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Granted(String),
    Refused(u16, String, String),
}

/// One checkout, as one of several in flight together.
async fn checkout(router: Arc<MultiServiceRouter>, body: Value) -> Outcome {
    let (status, response) = call(&router, "CheckoutLicense", &body).await;
    if status == 200 {
        let token = response["LicenseConsumptionToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a granted checkout answered no token: {response}"))
            .to_string();
        Outcome::Granted(token)
    } else {
        Outcome::Refused(status, error_code(&response), error_message(&response))
    }
}

/// Fire `callers` checkouts of `units` units at once, each under its own
/// `ClientToken`, and answer what each was answered.
async fn burst(
    router: &Arc<MultiServiceRouter>,
    sku: &str,
    fingerprint: &str,
    unit: &str,
    units: i64,
    callers: usize,
    client_token_prefix: &str,
) -> Vec<Outcome> {
    let tasks: Vec<_> = (0..callers)
        .map(|caller| {
            let body = checkout_body(
                sku,
                fingerprint,
                unit,
                units,
                &format!("{client_token_prefix}-{caller}"),
            );
            tokio::spawn(checkout(Arc::clone(router), body))
        })
        .collect();

    let mut outcomes = Vec::with_capacity(callers);
    for task in tasks {
        outcomes.push(task.await.expect("the checkout ran to completion"));
    }
    outcomes
}

/// The consumption tokens of the grants in `outcomes`.
fn granted_tokens(outcomes: &[Outcome]) -> Vec<String> {
    outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::Granted(token) => Some(token.clone()),
            Outcome::Refused(..) => None,
        })
        .collect()
}

/// The `(ConsumedValue, MaxCount)` pair `GetLicenseUsage` reports for this suite's
/// entitlement of `arn`.
async fn reported_usage(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> (String, Option<String>) {
    let (status, response) = call(router, "GetLicenseUsage", &json!({"LicenseArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetLicenseUsage to answer 200, got {status} {response}"
    );
    let usages = response["LicenseUsage"]["EntitlementUsages"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetLicenseUsage answered no EntitlementUsages: {response}")
        })
        .clone();
    let usage = usages
        .iter()
        .find(|usage| usage["Name"].as_str() == Some(ENTITLEMENT))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no usage reported for the entitlement {ENTITLEMENT:?}: {usages:?}")
        });
    let consumed = usage["ConsumedValue"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: usage {usage:?} carries no ConsumedValue"))
        .to_string();
    let max_count = usage["MaxCount"].as_str().map(str::to_string);
    (consumed, max_count)
}

/// Check in every token of `tokens` at once, and assert each release was accepted.
async fn check_in_all(router: &Arc<MultiServiceRouter>, case: &str, tokens: &[String]) {
    let tasks: Vec<_> = tokens
        .iter()
        .map(|token| {
            let router = Arc::clone(router);
            let body = json!({"LicenseConsumptionToken": token, "Beneficiary": BENEFICIARY});
            tokio::spawn(async move { call(&router, "CheckInLicense", &body).await })
        })
        .collect();
    for task in tasks {
        let (status, response) = task.await.expect("the check-in ran to completion");
        assert_eq!(
            status, 200,
            "case {case:?}: expected CheckInLicense to answer 200, got {status} {response}"
        );
    }
}

/// One row of the concurrent-cap table.
struct Row {
    case: &'static str,
    /// The entitlement's unit, one of the modelled `EntitlementUnit` values
    /// (`smithy_query(query="shape",shape="EntitlementUnit")`).
    unit: &'static str,
    /// The maximum count the entitlement caps, when it caps one.
    max_count: Option<i64>,
    /// Whether the entitlement allows consumption past that cap.
    overage: bool,
    /// How many callers ask at once.
    callers: usize,
    /// How many units each of them asks for.
    units_each: i64,
    /// How many of them the cap has room for.
    expected_grants: usize,
}

/// The table: one row per shape the race takes.
const ROWS: &[Row] = &[
    // The recorded instance: six callers of one unit each against a cap of two.
    Row {
        case: "six callers of one unit against a cap of two",
        unit: "Count",
        max_count: Some(2),
        overage: false,
        callers: 6,
        units_each: 1,
        expected_grants: 2,
    },
    // A wider burst against a wider cap.
    Row {
        case: "nine callers of one unit against a cap of four",
        unit: "Count",
        max_count: Some(4),
        overage: false,
        callers: 9,
        units_each: 1,
        expected_grants: 4,
    },
    // More than one unit per caller, dividing the cap exactly.
    Row {
        case: "five callers of two units against a cap of six",
        unit: "Count",
        max_count: Some(6),
        overage: false,
        callers: 5,
        units_each: 2,
        expected_grants: 3,
    },
    // A cap no whole number of requests reaches: the last unit stays unsold.
    Row {
        case: "four callers of three units against a cap of ten",
        unit: "Seconds",
        max_count: Some(10),
        overage: false,
        callers: 4,
        units_each: 3,
        expected_grants: 3,
    },
    // One caller's request is the whole cap, so the rest find no room at all.
    Row {
        case: "three callers each asking for the whole cap",
        unit: "Gigabits/Second",
        max_count: Some(40),
        overage: false,
        callers: 3,
        units_each: 40,
        expected_grants: 1,
    },
    // Overage lifts the cap, so the race has no loser.
    Row {
        case: "five callers against a cap of two that allows overage",
        unit: "Count",
        max_count: Some(2),
        overage: true,
        callers: 5,
        units_each: 1,
        expected_grants: 5,
    },
    // An entitlement capping no maximum count caps nothing.
    Row {
        case: "six callers against an entitlement capping no maximum count",
        unit: "Count",
        max_count: None,
        overage: false,
        callers: 6,
        units_each: 1,
        expected_grants: 6,
    },
];

/// However many callers race, the units admitted never exceed the cap: exactly as
/// many win as the entitlement has room for, each with its own token, and the rest
/// read the one modelled refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_of_concurrent_checkouts_admits_exactly_the_capped_units() {
    let router = Arc::new(emulator());

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let sku = format!("concurrent-cap-{index}");
        let (arn, fingerprint) = create_license(
            &router,
            case,
            &sku,
            declared_entitlement(row.unit, row.max_count, row.overage),
        )
        .await;

        let fresh = reported_usage(&router, case, &arn).await;
        let expected_max = row.max_count.map(|count| count.to_string());
        assert_eq!(
            fresh,
            ("0".to_string(), expected_max.clone()),
            "case {case:?}: expected a fresh license to report no consumption and the \
             declared maximum count, got {fresh:?}"
        );

        let outcomes = burst(
            &router,
            &sku,
            &fingerprint,
            row.unit,
            row.units_each,
            row.callers,
            "first-burst",
        )
        .await;

        let tokens = granted_tokens(&outcomes);
        assert_eq!(
            tokens.len(),
            row.expected_grants,
            "case {case:?}: expected exactly {} of {} concurrent callers to be granted, \
             got {}; outcomes: {outcomes:?}",
            row.expected_grants,
            row.callers,
            tokens.len()
        );
        let distinct: BTreeSet<&String> = tokens.iter().collect();
        assert_eq!(
            distinct.len(),
            tokens.len(),
            "case {case:?}: expected every grant to carry its own consumption token, \
             got {tokens:?}"
        );

        let expected_refusal = Outcome::Refused(
            NO_ENTITLEMENTS_ALLOWED_STATUS,
            NO_ENTITLEMENTS_ALLOWED_CODE.to_string(),
            NO_ENTITLEMENTS_ALLOWED.to_string(),
        );
        for (caller, outcome) in outcomes.iter().enumerate() {
            if let Outcome::Refused(..) = outcome {
                assert_eq!(
                    outcome, &expected_refusal,
                    "case {case:?}, caller {caller}: expected the refusal a checkout past \
                     the cap is owed, got {outcome:?}"
                );
            }
        }

        let held = reported_usage(&router, case, &arn).await;
        let expected_held = (row.expected_grants as i64 * row.units_each).to_string();
        assert_eq!(
            held,
            (expected_held, expected_max.clone()),
            "case {case:?}: expected the usage to report the units the winners hold, got \
             {held:?} after outcomes {outcomes:?}"
        );
        if !row.overage {
            if let Some(max_count) = row.max_count {
                let consumed: i64 = held
                    .0
                    .parse()
                    .unwrap_or_else(|e| panic!("case {case:?}: ConsumedValue {:?} is not a count ({e})", held.0));
                assert!(
                    consumed <= max_count,
                    "case {case:?}: expected consumption to stay at or below the cap \
                     {max_count}, got {consumed}"
                );
            }
        }

        check_in_all(&router, case, &tokens).await;
        let released = reported_usage(&router, case, &arn).await;
        assert_eq!(
            released,
            ("0".to_string(), expected_max.clone()),
            "case {case:?}: expected the usage to report no consumption once every winner \
             checked in, got {released:?}"
        );

        // The counter the race was decided on is the one the check-ins released, so a
        // second burst is admitted exactly as the first was.
        let again = burst(
            &router,
            &sku,
            &fingerprint,
            row.unit,
            row.units_each,
            row.callers,
            "second-burst",
        )
        .await;
        let again_tokens = granted_tokens(&again);
        assert_eq!(
            again_tokens.len(),
            row.expected_grants,
            "case {case:?}: expected the released units to be admitted again, so {} of {} \
             callers win the second burst, got {}; outcomes: {again:?}",
            row.expected_grants,
            row.callers,
            again_tokens.len()
        );
        let after_second = reported_usage(&router, case, &arn).await;
        assert_eq!(
            after_second,
            (
                (row.expected_grants as i64 * row.units_each).to_string(),
                expected_max
            ),
            "case {case:?}: expected the usage after the second burst to report the units \
             its winners hold, got {after_second:?}"
        );
    }
}

/// One row of the concurrent-repeat table: how many callers present the one
/// `ClientToken` at once, how many units it asks for, and the cap it asks under.
struct RepeatRow {
    case: &'static str,
    max_count: Option<i64>,
    callers: usize,
    units_each: i64,
}

/// The table: one row per shape the repeated token takes.
const REPEAT_ROWS: &[RepeatRow] = &[
    RepeatRow {
        case: "two callers of one unit under a cap of two",
        max_count: Some(2),
        callers: 2,
        units_each: 1,
    },
    RepeatRow {
        case: "six callers of one unit under a cap of two",
        max_count: Some(2),
        callers: 6,
        units_each: 1,
    },
    RepeatRow {
        case: "four callers of the whole cap at once",
        max_count: Some(3),
        callers: 4,
        units_each: 3,
    },
    RepeatRow {
        case: "five callers under an entitlement capping no maximum count",
        max_count: None,
        callers: 5,
        units_each: 2,
    },
];

/// One `ClientToken` is one grant however many callers present it at once: every
/// caller reads the grant the first of them minted, and the usage reports that one
/// grant's units rather than one per caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_repeats_of_one_client_token_consume_one_grant() {
    let router = Arc::new(emulator());

    for (index, row) in REPEAT_ROWS.iter().enumerate() {
        let case = row.case;
        let sku = format!("concurrent-repeat-{index}");
        let (arn, fingerprint) =
            create_license(&router, case, &sku, declared_entitlement("Count", row.max_count, false))
                .await;

        let client_token = format!("one-token-{index}");
        let tasks: Vec<_> = (0..row.callers)
            .map(|_| {
                let body = checkout_body(
                    &sku,
                    &fingerprint,
                    "Count",
                    row.units_each,
                    &client_token,
                );
                tokio::spawn(checkout(Arc::clone(&router), body))
            })
            .collect();
        let mut outcomes = Vec::with_capacity(row.callers);
        for task in tasks {
            outcomes.push(task.await.expect("the checkout ran to completion"));
        }

        let tokens = granted_tokens(&outcomes);
        assert_eq!(
            tokens.len(),
            row.callers,
            "case {case:?}: expected every caller of the one ClientToken to be answered \
             with a grant, got {}; outcomes: {outcomes:?}",
            tokens.len()
        );
        let distinct: BTreeSet<&String> = tokens.iter().collect();
        assert_eq!(
            distinct.len(),
            1,
            "case {case:?}: expected ONE consumption token across every concurrent repeat, \
             got {distinct:?}"
        );

        let held = reported_usage(&router, case, &arn).await;
        assert_eq!(
            held,
            (
                row.units_each.to_string(),
                row.max_count.map(|count| count.to_string())
            ),
            "case {case:?}: expected the usage to report the one grant's units, got {held:?}"
        );

        let token = tokens
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: no grant to check in"))
            .clone();
        check_in_all(&router, case, &[token]).await;
        let released = reported_usage(&router, case, &arn).await;
        assert_eq!(
            released.0,
            "0".to_string(),
            "case {case:?}: expected the one grant's check-in to release every unit it \
             held, got {released:?}"
        );
    }
}

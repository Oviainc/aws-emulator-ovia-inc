// SPDX-License-Identifier: CC0-1.0

//! `GetLicenseUsage` over the wire, driven as awsJson1_1 requests (`POST /`,
//! `X-Amz-Target: AWSLicenseManager.<Operation>`) through the same
//! `MultiServiceRouter` the production emulator mounts.
//!
//! The rule under test: the usage of a license is one entry per entitlement that
//! license PERSISTS, in the order the license carries them — the entitlement's own
//! name and unit, its maximum count rendered as text when it caps one and absent
//! when it caps none — and each entry's consumed value is the total units the
//! service's outstanding consumption records hold against that entitlement of that
//! license, which is zero while none is outstanding. An ARN that names no stored
//! license is refused with a modelled 400 rather than reported as an empty usage.
//!
//! Each table row differs in what the rule generalises over: how many entitlements
//! the license declares, their units, whether they cap a count, and how much
//! consumption is outstanding against them.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_licensemanager_awslicensemanager::state::{
    service_state, StoredLicenseConsumption,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.licensemanager#AWSLicenseManager`.
const TARGET_PREFIX: &str = "AWSLicenseManager";

/// The region the harness signs for.
const SIGNING_REGION: &str = "us-east-1";

/// Entitlement units this suite builds licenses from, taken from the modelled
/// `EntitlementUnit` enum, whose values the usage's `EntitlementDataUnit` admits too
/// (`smithy_query(query="shape",shape="EntitlementUnit,EntitlementDataUnit")`; the
/// crate carries the same catalog as `ENTITLEMENT_UNIT_VALUES`).
const UNITS: &[&str] = &[
    "Count",
    "None",
    "Seconds",
    "Terabytes",
    "Gigabits/Second",
    "Percent",
];

/// One emulator: a single shared engine behind the router, exactly as
/// `core/router` assembles it, plus the handle on that engine this suite seeds
/// consumption records through — nothing mints them over the wire yet, because the
/// checkout operations are still scaffolded.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_licensemanager_awslicensemanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
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
        .body(Bytes::from(
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

/// One entitlement, with only the members the row sets.
fn entitlement(name: &str, unit: &str, extra: &[(&str, Value)]) -> Value {
    let mut entitlement = Map::new();
    entitlement.insert("Name".to_string(), json!(name));
    entitlement.insert("Unit".to_string(), json!(unit));
    for (member, value) in extra {
        entitlement.insert((*member).to_string(), value.clone());
    }
    Value::Object(entitlement)
}

/// A license body carrying `entitlements`, keyed by `client_token`.
fn license_body(client_token: &str, entitlements: Value) -> Value {
    json!({
        "LicenseName": format!("license-{client_token}"),
        "ProductName": "Usage Suite",
        "ProductSKU": format!("sku-{client_token}"),
        "Issuer": {"Name": "Usage Issuer"},
        "HomeRegion": "us-east-1",
        "Validity": {"Begin": "2026-01-01T00:00:00Z"},
        "Entitlements": entitlements,
        "Beneficiary": "arn:aws:iam::000000000000:root",
        "ConsumptionConfiguration": {"ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": 60}},
        "ClientToken": client_token,
    })
}

/// Create one license and return the ARN the service minted.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call(router, "CreateLicense", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLicense to answer 200, got {status} {response}"
    );
    response["LicenseArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateLicense answered no LicenseArn: {response}"))
        .to_string()
}

/// Read the usage of `arn`, failing the case when the read did not succeed.
async fn usage_ok(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<Value> {
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
    usages
}

/// The usage entry the rule predicts for `entitlement` when `consumed` units of it
/// are outstanding.
fn expected_usage(entitlement: &Value, consumed: i64) -> Value {
    let mut expected = Map::new();
    expected.insert("Name".to_string(), entitlement["Name"].clone());
    expected.insert("ConsumedValue".to_string(), json!(consumed.to_string()));
    if let Some(max_count) = entitlement.get("MaxCount").and_then(Value::as_i64) {
        expected.insert("MaxCount".to_string(), json!(max_count.to_string()));
    }
    expected.insert("Unit".to_string(), entitlement["Unit"].clone());
    Value::Object(expected)
}

/// Assert the reported usages are exactly what the rule predicts for the
/// entitlements `entitlements`, with `consumed` units outstanding against each.
fn assert_usages(case: &str, reported: &[Value], entitlements: &[Value], consumed: &[i64]) {
    assert_eq!(
        reported.len(),
        entitlements.len(),
        "case {case:?}: expected one usage per declared entitlement ({}), got {} ({reported:?})",
        entitlements.len(),
        reported.len()
    );
    for (index, (entitlement, units)) in entitlements.iter().zip(consumed).enumerate() {
        let expected = expected_usage(entitlement, *units);
        assert_eq!(
            reported[index], expected,
            "case {case:?}: usage {index} is {:?}, expected {expected:?}",
            reported[index]
        );
    }
}

/// Seed one outstanding consumption of `license_arn`: `units` of each named
/// entitlement, held under the token `token`.
fn seed_consumption(
    engine: &engine::InMemoryEngine,
    case: &str,
    token: &str,
    license_arn: &str,
    units: &[(&str, i64)],
) {
    let state = service_state(engine).expect("the license store is reachable");
    let stored = state.lock().insert_consumption(StoredLicenseConsumption {
        token: token.to_string(),
        license_arn: license_arn.to_string(),
        consumed_units: units
            .iter()
            .map(|(name, units)| ((*name).to_string(), *units))
            .collect::<BTreeMap<String, i64>>(),
    });
    assert!(
        stored,
        "case {case:?}: the store refused the consumption record {token:?}"
    );
}

/// The table: one row per form a license's entitlement list takes.
fn usage_rows() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        (
            "one counted entitlement that caps a maximum",
            vec![entitlement(
                "seats",
                "Count",
                &[("MaxCount", json!(10)), ("AllowCheckIn", json!(true))],
            )],
        ),
        (
            "one entitlement that caps nothing: unit None carries a Value",
            vec![entitlement("site", "None", &[("Value", json!("unlimited"))])],
        ),
        (
            "a maximum of zero, which caps consumption at none rather than not capping it",
            vec![entitlement("storage", "Terabytes", &[("MaxCount", json!(0))])],
        ),
        (
            "three entitlements across three units, reported in the license's order",
            vec![
                entitlement(
                    "bandwidth",
                    "Gigabits/Second",
                    &[("MaxCount", json!(40)), ("Overage", json!(true))],
                ),
                entitlement("runtime", "Seconds", &[("MaxCount", json!(86_400))]),
                entitlement("headroom", "Percent", &[]),
            ],
        ),
        (
            "an entitlement capped at the largest count the member holds",
            vec![entitlement(
                "bits",
                "Bits",
                &[("MaxCount", json!(i64::MAX))],
            )],
        ),
        ("no entitlements at all", Vec::new()),
        (
            "the largest entitlement list this store accepts, cycling the unit catalog",
            (0..50)
                .map(|n| {
                    entitlement(
                        &format!("entitlement-{n:02}"),
                        UNITS[n % UNITS.len()],
                        &[("MaxCount", json!(n as i64))],
                    )
                })
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn usage_reports_one_entry_per_declared_entitlement() {
    let (router, _engine) = emulator();

    for (index, (case, entitlements)) in usage_rows().into_iter().enumerate() {
        let body = license_body(
            &format!("token-usage-{index}"),
            Value::Array(entitlements.clone()),
        );
        let arn = create_ok(&router, case, &body).await;

        // Nothing has been checked out, so every declared entitlement is reported
        // with no units consumed.
        let reported = usage_ok(&router, case, &arn).await;
        let consumed = vec![0_i64; entitlements.len()];
        assert_usages(case, &reported, &entitlements, &consumed);
    }
}

#[tokio::test]
async fn consumed_value_totals_the_outstanding_consumption_of_that_entitlement() {
    let (router, engine) = emulator();

    let entitlements = vec![
        entitlement("seats", "Count", &[("MaxCount", json!(10))]),
        entitlement("runtime", "Seconds", &[("MaxCount", json!(3_600))]),
    ];
    let case = "two licenses declaring the same entitlement names";
    let first = create_ok(
        &router,
        case,
        &license_body("token-usage-first", Value::Array(entitlements.clone())),
    )
    .await;
    let second = create_ok(
        &router,
        case,
        &license_body("token-usage-second", Value::Array(entitlements.clone())),
    )
    .await;

    // Two grants on the first license sum per entitlement name; a grant on the
    // second license counts only against the second; and units held against a name
    // the license does not declare are reported by neither.
    seed_consumption(&engine, case, "tok-1", &first, &[("seats", 1)]);
    seed_consumption(
        &engine,
        case,
        "tok-2",
        &first,
        &[("seats", 2), ("runtime", 600)],
    );
    seed_consumption(&engine, case, "tok-3", &second, &[("seats", 7)]);
    seed_consumption(&engine, case, "tok-4", &first, &[("undeclared", 5)]);

    let reported = usage_ok(&router, "the license two grants are outstanding on", &first).await;
    assert_usages(
        "the license two grants are outstanding on",
        &reported,
        &entitlements,
        &[3, 600],
    );

    let reported = usage_ok(&router, "the license one grant is outstanding on", &second).await;
    assert_usages(
        "the license one grant is outstanding on",
        &reported,
        &entitlements,
        &[7, 0],
    );

    // Reporting the usage changes nothing a later caller sees: the same read
    // repeated answers the same totals.
    let again = usage_ok(&router, "the same read repeated", &first).await;
    assert_usages("the same read repeated", &again, &entitlements, &[3, 600]);
}

#[tokio::test]
async fn an_arn_that_names_no_license_is_refused() {
    let (router, _engine) = emulator();

    let stored = create_ok(
        &router,
        "a stored license",
        &license_body(
            "token-usage-refusal",
            json!([entitlement("seats", "Count", &[("MaxCount", json!(1))])]),
        ),
    )
    .await;

    // Each row is a well-formed ARN the store holds no license under: another
    // license id, the same id in another Region, another resource kind of this
    // service, and another service's resource.
    let elsewhere = stored.replace("us-east-1", "eu-west-1");
    let other_kind = stored.replace(":license:l-", ":license-configuration:lic-");
    let rows: Vec<(&str, &str)> = vec![
        (
            "a license ARN no create minted",
            "arn:aws:license-manager:us-east-1:000000000000:license:l-00000000000000000000000000000000",
        ),
        ("the stored license id in another Region", &elsewhere),
        ("a license-configuration ARN", &other_kind),
        (
            "another service's resource",
            "arn:aws:s3:::bucket-that-is-not-a-license",
        ),
    ];

    for (case, arn) in rows {
        let (status, response) =
            call(&router, "GetLicenseUsage", &json!({"LicenseArn": arn})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetLicenseUsage to refuse {arn:?} with 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );
        assert!(
            response["message"]
                .as_str()
                .or_else(|| response["Message"].as_str())
                .is_some_and(|message| message.contains(arn)),
            "case {case:?}: expected the refusal to name the ARN it could not resolve, got {response}"
        );
    }

    // The refusals left the stored license reportable.
    let reported = usage_ok(&router, "the stored license after the refusals", &stored).await;
    assert_usages(
        "the stored license after the refusals",
        &reported,
        &[entitlement("seats", "Count", &[("MaxCount", json!(1))])],
        &[0],
    );
}

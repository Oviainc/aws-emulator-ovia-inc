// SPDX-License-Identifier: CC0-1.0

//! The granted-license slice over the wire: `CreateLicense` and `GetLicense`, with
//! the tagging operations that share this service's one ARN-keyed tag map, driven as
//! awsJson1_1 requests (`POST /`, `X-Amz-Target: AWSLicenseManager.<Operation>`)
//! through the same `MultiServiceRouter` the production emulator mounts.
//!
//! The rules under test:
//!
//! * `CreateLicense` persists ONE license record and `GetLicense` answers from it,
//!   so every member of the license body a create carried is what a later read
//!   reports — whatever its value, size, unit or form.
//! * the minted identity is a license ARN in the Region the request named as the
//!   license's home, the license is `AVAILABLE` at once, and its version is the
//!   first one.
//! * `ClientToken` is an idempotency key: the repeat of an accepted create reports
//!   the license that create minted and mints nothing new.
//! * a license body this store will not hold is refused with a modelled 400 and
//!   nothing is stored.
//!
//! Every assertion is about STORED state: a response is checked against what an
//! earlier, different call persisted, never against a value this file hands the
//! service and reads straight back out of the same response.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.licensemanager#AWSLicenseManager`.
const TARGET_PREFIX: &str = "AWSLicenseManager";

/// The region the harness signs for.
const SIGNING_REGION: &str = "us-east-1";

/// The canonical account id every emulated ARN carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// Every value the modelled `EntitlementUnit` enum admits, in model order
/// (`smithy_query(query="shape",shape="EntitlementUnit")`, which the crate also
/// carries as `ENTITLEMENT_UNIT_VALUES`).
const ENTITLEMENT_UNITS: &[&str] = &[
    "Bits",
    "Bits/Second",
    "Bytes",
    "Bytes/Second",
    "Count",
    "Count/Second",
    "Gigabits",
    "Gigabits/Second",
    "Gigabytes",
    "Gigabytes/Second",
    "Kilobits",
    "Kilobits/Second",
    "Kilobytes",
    "Kilobytes/Second",
    "Megabits",
    "Megabits/Second",
    "Megabytes",
    "Megabytes/Second",
    "Microseconds",
    "Milliseconds",
    "None",
    "Percent",
    "Seconds",
    "Terabits",
    "Terabits/Second",
    "Terabytes",
    "Terabytes/Second",
];

/// Every value the modelled `RenewType` enum admits
/// (`smithy_query(query="shape",shape="RenewType")`).
const RENEW_TYPES: &[&str] = &["Monthly", "None", "Weekly"];

/// The seed of the generated round-trip cases, printed by every assertion of
/// [`round_trip_holds_for_generated_license_bodies`] so a failure is reproducible.
const GENERATOR_SEED: u64 = 0x5EED_1CE5_1CE5_5EED;

/// One emulator: a single shared engine behind the router, exactly as
/// `core/router` assembles it.
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

/// Whether `arn` is the ARN of a license whose home Region is `region`:
/// `arn:aws:license-manager:<region>:<account>:license:l-<32 hex>`.
fn is_license_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:license-manager:{region}:{ACCOUNT_ID}:license:");
    arn.strip_prefix(&prefix)
        .and_then(|id| id.strip_prefix("l-"))
        .is_some_and(|hex| hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Create one license and return the ARN the service minted, failing the case when
/// the call did not succeed or did not answer the minted identity.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call(router, "CreateLicense", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLicense to answer 200, got {status} {response}"
    );
    let arn = response["LicenseArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateLicense answered no LicenseArn: {response}"))
        .to_string();
    let home_region = body["HomeRegion"].as_str().expect("the case names a home Region");
    assert!(
        is_license_arn(&arn, home_region),
        "case {case:?}: expected a license ARN in {home_region}, got {arn:?}"
    );
    // A license AWS hands back can be checked out at once, and the create is the
    // first version of it.
    assert_eq!(
        response["Status"].as_str(),
        Some("AVAILABLE"),
        "case {case:?}: expected Status AVAILABLE, got {response}"
    );
    assert_eq!(
        response["Version"].as_str(),
        Some("1"),
        "case {case:?}: expected Version 1, got {response}"
    );
    arn
}

/// Read one license back, failing the case when the read did not succeed.
async fn get_ok(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) = call(router, "GetLicense", &json!({"LicenseArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetLicense to answer 200, got {status} {response}"
    );
    response
        .get("License")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetLicense answered no License: {response}"))
}

/// Assert that `member` of the license the store reports is exactly what the create
/// carried — including ABSENT when the create carried none.
fn assert_stored_member(case: &str, stored: &Value, body: &Value, member: &str) {
    assert_eq!(
        stored.get(member),
        body.get(member),
        "case {case:?}: GetLicense reported {member} = {:?}, expected the created {:?}",
        stored.get(member),
        body.get(member)
    );
}

/// Assert the whole license body `body` created is what the store reports in
/// `stored`, member by member.
fn assert_round_trip(case: &str, stored: &Value, body: &Value, arn: &str) {
    for member in [
        "LicenseName",
        "ProductName",
        "ProductSKU",
        "HomeRegion",
        "Beneficiary",
        "Validity",
        "Entitlements",
        "ConsumptionConfiguration",
        "LicenseMetadata",
    ] {
        assert_stored_member(case, stored, body, member);
    }
    assert_eq!(
        stored["LicenseArn"].as_str(),
        Some(arn),
        "case {case:?}: GetLicense reported another license's ARN: {stored}"
    );
    assert_eq!(
        stored["Status"].as_str(),
        Some("AVAILABLE"),
        "case {case:?}: expected the stored Status AVAILABLE, got {stored}"
    );
    assert_eq!(
        stored["Version"].as_str(),
        Some("1"),
        "case {case:?}: expected the stored Version 1, got {stored}"
    );
    // The issuer is reported as `IssuerDetails`: the name and sign key the create
    // carried. The emulator holds no KMS key material and signs no license, so it
    // reports no key fingerprint rather than inventing one.
    assert_eq!(
        stored["Issuer"]["Name"], body["Issuer"]["Name"],
        "case {case:?}: GetLicense reported another Issuer.Name: {stored}"
    );
    assert_eq!(
        stored["Issuer"].get("SignKey"),
        body["Issuer"].get("SignKey"),
        "case {case:?}: GetLicense reported another Issuer.SignKey: {stored}"
    );
    assert!(
        stored["Issuer"].get("KeyFingerprint").is_none(),
        "case {case:?}: no license is signed here, so no KeyFingerprint may be reported: {stored}"
    );
    let create_time = stored["CreateTime"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the stored license carries no CreateTime: {stored}"));
    assert!(
        DateTime::from_str(create_time, Format::DateTime).is_ok(),
        "case {case:?}: CreateTime {create_time:?} is not an ISO8601-UTC instant"
    );
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

/// The smallest license body that is accepted, with `client_token` as its
/// idempotency key: a row overwrites whichever members it is about.
fn base_body(client_token: &str) -> Value {
    json!({
        "LicenseName": "base-license",
        "ProductName": "base-product",
        "ProductSKU": "sku-base",
        "Issuer": {"Name": "base-issuer"},
        "HomeRegion": "us-east-1",
        "Validity": {"Begin": "2026-01-01T00:00:00Z"},
        "Entitlements": [entitlement("base", "Count", &[("MaxCount", json!(1))])],
        "Beneficiary": "arn:aws:iam::000000000000:root",
        "ConsumptionConfiguration": {"ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": 60}},
        "ClientToken": client_token,
    })
}

/// The round-trip table: one row per form a license body takes, differing in the
/// entitlement units and counts, the consumption configuration's form, the presence
/// of the optional members, the validity's form and the home Region.
fn round_trip_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a counted entitlement, provisional consumption, metadata and tags",
            json!({
                "LicenseName": "analytics-seat",
                "ProductName": "Analytics Suite",
                "ProductSKU": "sku-analytics-9000",
                "Issuer": {"Name": "Example Corp", "SignKey": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555"},
                "HomeRegion": "us-east-1",
                "Validity": {"Begin": "2026-02-01T00:00:00Z", "End": "2027-02-01T00:00:00Z"},
                "Entitlements": [entitlement(
                    "seats",
                    "Count",
                    &[("MaxCount", json!(10)), ("Overage", json!(false)), ("AllowCheckIn", json!(true))],
                )],
                "Beneficiary": "arn:aws:iam::000000000000:role/analytics",
                "ConsumptionConfiguration": {
                    "RenewType": "Monthly",
                    "ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": 240},
                },
                "LicenseMetadata": [
                    {"Name": "contract", "Value": "C-4711"},
                    {"Name": "channel", "Value": "direct"},
                ],
                "Tags": [{"Key": "team", "Value": "data"}, {"Key": "env", "Value": "prod"}],
                "ClientToken": "token-analytics-seat",
            }),
        ),
        (
            "an unmetered entitlement (unit None carries a Value), borrow consumption, no optional members",
            json!({
                "LicenseName": "site-wide",
                "ProductName": "Build Server",
                "ProductSKU": "sku-build-1",
                "Issuer": {"Name": "Example Corp"},
                "HomeRegion": "eu-west-1",
                "Validity": {"Begin": "2026-03-15T12:30:45Z"},
                "Entitlements": [entitlement(
                    "site",
                    "None",
                    &[("Value", json!("unlimited")), ("AllowCheckIn", json!(false))],
                )],
                "Beneficiary": "arn:aws:iam::000000000000:role/builders",
                "ConsumptionConfiguration": {
                    "BorrowConfiguration": {"AllowEarlyCheckIn": true, "MaxTimeToLiveInMinutes": 1440},
                },
                "ClientToken": "token-site-wide",
            }),
        ),
        (
            "three entitlements across three units, overage allowed, fractional validity",
            json!({
                "LicenseName": "throughput-bundle",
                "ProductName": "Edge Transfer",
                "ProductSKU": "sku-edge-42",
                "Issuer": {"Name": "Edge Issuer"},
                "HomeRegion": "ap-southeast-2",
                "Validity": {"Begin": "2026-04-01T00:00:00.500Z", "End": "2026-04-30T23:59:59.999Z"},
                "Entitlements": [
                    entitlement("bandwidth", "Gigabits/Second", &[("MaxCount", json!(40)), ("Overage", json!(true))]),
                    entitlement("storage", "Terabytes", &[("MaxCount", json!(0))]),
                    entitlement("runtime", "Seconds", &[("MaxCount", json!(86_400)), ("AllowCheckIn", json!(true))]),
                ],
                "Beneficiary": "arn:aws:iam::000000000000:root",
                "ConsumptionConfiguration": {"RenewType": "Weekly"},
                "LicenseMetadata": [{"Name": "tier", "Value": "gold"}],
                "ClientToken": "token-throughput-bundle",
            }),
        ),
        (
            "no entitlements and an empty consumption configuration",
            json!({
                "LicenseName": "placeholder",
                "ProductName": "Nothing Yet",
                "ProductSKU": "sku-none",
                "Issuer": {"Name": "Example Corp"},
                "HomeRegion": "us-west-2",
                "Validity": {"Begin": "2026-05-01T00:00:00Z"},
                "Entitlements": [],
                "Beneficiary": "arn:aws:iam::000000000000:root",
                "ConsumptionConfiguration": {},
                "LicenseMetadata": [],
                "ClientToken": "token-placeholder",
            }),
        ),
        (
            "the largest license body this store accepts: 50 entitlements and 50 metadata entries",
            {
                let mut body = base_body("token-largest");
                body["LicenseName"] = json!("bulk");
                body["HomeRegion"] = json!("eu-central-1");
                body["Entitlements"] = Value::Array(
                    (0..50)
                        .map(|n| {
                            entitlement(
                                &format!("entitlement-{n}"),
                                ENTITLEMENT_UNITS[n % ENTITLEMENT_UNITS.len()],
                                &[("MaxCount", json!(n as i64))],
                            )
                        })
                        .collect(),
                );
                body["LicenseMetadata"] = Value::Array(
                    (0..50)
                        .map(|n| json!({"Name": format!("key-{n}"), "Value": format!("value-{n}")}))
                        .collect(),
                );
                body
            },
        ),
    ]
}

#[tokio::test]
async fn a_created_license_is_what_get_license_reports() {
    let router = emulator();
    let mut arns = BTreeSet::new();
    for (case, body) in round_trip_rows() {
        let arn = create_ok(&router, case, &body).await;
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: the service minted the ARN {arn:?} twice"
        );
        let stored = get_ok(&router, case, &arn).await;
        assert_round_trip(case, &stored, &body, &arn);

        // The read is a report, not a mutation: a second read reports the same
        // record, untouched.
        let again = get_ok(&router, case, &arn).await;
        assert_eq!(
            again, stored,
            "case {case:?}: reading a license changed what a later read reports"
        );
    }
}

#[tokio::test]
async fn every_modelled_entitlement_unit_and_renew_type_round_trips() {
    // The catalogs are the modelled `EntitlementUnit` and `RenewType` enums, so a
    // license may carry any entry of either and a read reports exactly that entry.
    let router = emulator();
    for (n, unit) in ENTITLEMENT_UNITS.iter().enumerate() {
        let case = format!("unit {unit}");
        let mut body = base_body(&format!("token-unit-{n}"));
        body["LicenseName"] = json!(format!("unit-{n}"));
        body["ConsumptionConfiguration"] =
            json!({"RenewType": RENEW_TYPES[n % RENEW_TYPES.len()], "ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": 30}});
        // `Value` is for the `None` unit and `MaxCount` for every other one.
        body["Entitlements"] = json!([if *unit == "None" {
            entitlement("per-unit", unit, &[("Value", json!("unlimited"))])
        } else {
            entitlement("per-unit", unit, &[("MaxCount", json!(7))])
        }]);

        let arn = create_ok(&router, &case, &body).await;
        let stored = get_ok(&router, &case, &arn).await;
        assert_round_trip(&case, &stored, &body, &arn);
    }
}

#[tokio::test]
async fn round_trip_holds_for_generated_license_bodies() {
    // A bounded, seeded sweep of the same rule: whatever entitlement set, metadata
    // and consumption form a body carries, the read reports exactly it. The seed is
    // in every message, so a failure replays.
    let router = emulator();
    let mut state = GENERATOR_SEED;
    for n in 0..32u64 {
        let case = format!("generated case {n} (seed {GENERATOR_SEED:#x})");
        let mut body = base_body(&format!("token-generated-{n}"));
        body["LicenseName"] = json!(format!("generated-{n}"));
        body["HomeRegion"] = json!(["us-east-1", "us-west-2", "eu-west-3", "sa-east-1"]
            [next(&mut state) as usize % 4]);
        let entitlements = 1 + next(&mut state) % 4;
        body["Entitlements"] = Value::Array(
            (0..entitlements)
                .map(|e| {
                    let unit = ENTITLEMENT_UNITS[next(&mut state) as usize % ENTITLEMENT_UNITS.len()];
                    let max_count = (next(&mut state) % 1_000) as i64;
                    entitlement(
                        &format!("generated-{n}-{e}"),
                        unit,
                        &[
                            ("MaxCount", json!(max_count)),
                            ("Overage", json!(next(&mut state).is_multiple_of(2))),
                            ("AllowCheckIn", json!(next(&mut state).is_multiple_of(2))),
                        ],
                    )
                })
                .collect(),
        );
        if next(&mut state).is_multiple_of(2) {
            body["Validity"] = json!({"Begin": "2026-01-01T00:00:00Z", "End": "2026-12-31T23:59:59Z"});
        }
        if next(&mut state).is_multiple_of(2) {
            body["LicenseMetadata"] = json!([{"Name": format!("generated-{n}"), "Value": "set"}]);
        }

        let arn = create_ok(&router, &case, &body).await;
        let stored = get_ok(&router, &case, &arn).await;
        assert_round_trip(&case, &stored, &body, &arn);
    }
}

/// One step of the generator: SplitMix64, so the sweep above is reproducible from
/// [`GENERATOR_SEED`] alone and needs no entropy.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[tokio::test]
async fn a_repeat_of_a_client_token_mints_nothing_new() {
    let router = emulator();
    for (case, body) in round_trip_rows() {
        let arn = create_ok(&router, case, &body).await;
        let stored = get_ok(&router, case, &arn).await;

        // The idempotent retry: the same request again names the license it already
        // minted, down to the version and the creation time, so nothing was minted
        // a second time.
        let (status, repeat) = call(&router, "CreateLicense", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated create to answer 200, got {status} {repeat}"
        );
        assert_eq!(
            repeat["LicenseArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the repeated create minted another license: {repeat}"
        );
        assert_eq!(
            repeat["Status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: expected the repeat to report Status AVAILABLE, got {repeat}"
        );
        assert_eq!(
            repeat["Version"].as_str(),
            Some("1"),
            "case {case:?}: expected the repeat to report Version 1, got {repeat}"
        );
        assert_eq!(
            get_ok(&router, case, &arn).await,
            stored,
            "case {case:?}: the repeated create changed the stored license"
        );

        // A DIFFERENT token is a different request, so it mints a second license.
        let mut other = body.clone();
        other["ClientToken"] = json!(format!("{}-again", body["ClientToken"].as_str().unwrap()));
        let second = create_ok(&router, case, &other).await;
        assert_ne!(
            second, arn,
            "case {case:?}: a create under a fresh ClientToken reused a license ARN"
        );
        let second_stored = get_ok(&router, case, &second).await;
        assert_round_trip(case, &second_stored, &other, &second);
    }
}

#[tokio::test]
async fn a_license_arn_the_store_does_not_hold_is_a_modelled_400() {
    let router = emulator();
    let arn = create_ok(&router, "stored", &base_body("token-arn-rules")).await;

    // A configuration lives in the same service but is not a license, and vice
    // versa: each read answers only from its own store.
    let (_, configuration) = call(
        &router,
        "CreateLicenseConfiguration",
        &json!({"Name": "not-a-license", "LicenseCountingType": "vCPU"}),
    )
    .await;
    let configuration_arn = configuration["LicenseConfigurationArn"]
        .as_str()
        .expect("the configuration create answered an ARN")
        .to_string();

    for (case, body) in [
        ("an ARN that was never minted", json!({"LicenseArn": format!("{arn}-nope")})),
        (
            "a license ARN with another id",
            json!({"LicenseArn": format!("arn:aws:license-manager:us-east-1:{ACCOUNT_ID}:license:l-{}", "0".repeat(32))}),
        ),
        (
            "a license configuration's ARN",
            json!({"LicenseArn": configuration_arn.clone()}),
        ),
        (
            "a stored license at a version it does not have",
            json!({"LicenseArn": arn.clone(), "Version": "2"}),
        ),
    ] {
        let (status, response) = call(&router, "GetLicense", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetLicense to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );
    }

    // The version the create minted is still readable.
    let stored = call(
        &router,
        "GetLicense",
        &json!({"LicenseArn": arn.clone(), "Version": "1"}),
    )
    .await;
    assert_eq!(
        stored.0, 200,
        "GetLicense at the stored version must answer 200, got {} {}",
        stored.0, stored.1
    );
    assert_eq!(
        stored.1["License"]["LicenseArn"].as_str(),
        Some(arn.as_str()),
        "GetLicense at the stored version reported another license: {}",
        stored.1
    );
}

#[tokio::test]
async fn a_license_body_the_store_will_not_hold_is_refused_and_stores_nothing() {
    let router = emulator();
    let oversized = "x".repeat(2049);

    // Each row breaks one rule the store carries: a bound the model publishes no
    // length for, a Region token, a range that runs backwards, or a count with no
    // meaning below zero.
    let rows: Vec<(&str, Value)> = vec![
        ("a LicenseName past the stored length", {
            let mut body = base_body("token-refused-name");
            body["LicenseName"] = json!(oversized.clone());
            body
        }),
        ("a Beneficiary past the stored length", {
            let mut body = base_body("token-refused-beneficiary");
            body["Beneficiary"] = json!(oversized.clone());
            body
        }),
        ("a HomeRegion that is not a Region token", {
            let mut body = base_body("token-refused-region");
            body["HomeRegion"] = json!("us-east-1/../../etc");
            body
        }),
        ("an empty HomeRegion", {
            let mut body = base_body("token-refused-empty-region");
            body["HomeRegion"] = json!("");
            body
        }),
        ("a Validity that ends before it begins", {
            let mut body = base_body("token-refused-validity");
            body["Validity"] = json!({"Begin": "2026-06-01T00:00:00Z", "End": "2026-05-31T23:59:59Z"});
            body
        }),
        ("a negative entitlement MaxCount", {
            let mut body = base_body("token-refused-max-count");
            body["Entitlements"] = json!([entitlement("seats", "Count", &[("MaxCount", json!(-1))])]);
            body
        }),
        ("a negative provisional MaxTimeToLiveInMinutes", {
            let mut body = base_body("token-refused-ttl");
            body["ConsumptionConfiguration"] =
                json!({"ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": -5}});
            body
        }),
        ("a negative borrow MaxTimeToLiveInMinutes", {
            let mut body = base_body("token-refused-borrow-ttl");
            body["ConsumptionConfiguration"] = json!({
                "BorrowConfiguration": {"AllowEarlyCheckIn": false, "MaxTimeToLiveInMinutes": -1}
            });
            body
        }),
        ("one entitlement past the entitlement cap", {
            let mut body = base_body("token-refused-entitlements");
            body["Entitlements"] = Value::Array(
                (0..51)
                    .map(|n| entitlement(&format!("e-{n}"), "Count", &[("MaxCount", json!(1))]))
                    .collect(),
            );
            body
        }),
        ("one metadata entry past the metadata cap", {
            let mut body = base_body("token-refused-metadata");
            body["LicenseMetadata"] = Value::Array(
                (0..51)
                    .map(|n| json!({"Name": format!("k-{n}"), "Value": "v"}))
                    .collect(),
            );
            body
        }),
        ("one tag past the tag cap", {
            let mut body = base_body("token-refused-tags");
            body["Tags"] = Value::Array(
                (0..51)
                    .map(|n| json!({"Key": format!("k-{n}"), "Value": "v"}))
                    .collect(),
            );
            body
        }),
    ];

    for (case, body) in rows {
        let (status, response) = call(&router, "CreateLicense", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateLicense to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );

        // A refused create stored NOTHING — not even its ClientToken, so the same
        // token is still free to mint a license from an acceptable body.
        let token = body["ClientToken"].as_str().expect("the row carries a token");
        let mut acceptable = base_body(token);
        acceptable["LicenseName"] = json!(format!("after-{token}"));
        let arn = create_ok(&router, case, &acceptable).await;
        let stored = get_ok(&router, case, &arn).await;
        assert_round_trip(case, &stored, &acceptable, &arn);
    }
}

#[tokio::test]
async fn an_entitlement_unit_outside_the_modelled_catalog_is_refused() {
    let router = emulator();
    // `Unit` targets the modelled `EntitlementUnit` enum, so a value outside its
    // catalog never reaches the store: the centralized constraint walk refuses it
    // as a 400 `ValidationException`.
    for (case, unit) in [
        ("an invented unit", "Furlongs"),
        ("a unit in the wrong case", "count"),
        ("an empty unit", ""),
    ] {
        let mut body = base_body(&format!("token-bad-unit-{case}"));
        body["Entitlements"] = json!([entitlement("seats", unit, &[("MaxCount", json!(1))])]);
        let (status, response) = call(&router, "CreateLicense", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateLicense to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}

#[tokio::test]
async fn a_license_is_tagged_through_the_same_map_as_every_other_resource() {
    let router = emulator();
    let mut body = base_body("token-tagged");
    body["Tags"] = json!([{"Key": "team", "Value": "data"}, {"Key": "env", "Value": "dev"}]);
    let arn = create_ok(&router, "tagged at create", &body).await;

    let tags = |response: &Value| -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = response["Tags"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        pairs.sort();
        pairs
    };

    let (status, listed) = call(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource to answer 200 for a license, got {status} {listed}"
    );
    assert_eq!(
        tags(&listed),
        vec![
            ("env".to_string(), "dev".to_string()),
            ("team".to_string(), "data".to_string())
        ],
        "a license's create-time tags must be what ListTagsForResource reports: {listed}"
    );

    // `TagResource` and `UntagResource` reach a license through the same map.
    let (status, response) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": [{"Key": "env", "Value": "prod"}, {"Key": "owner", "Value": "platform"}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected TagResource to answer 200 for a license, got {status} {response}"
    );
    let (_, listed) = call(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        tags(&listed),
        vec![
            ("env".to_string(), "prod".to_string()),
            ("owner".to_string(), "platform".to_string()),
            ("team".to_string(), "data".to_string())
        ],
        "TagResource must replace a license's tag value and add the new key: {listed}"
    );

    let (status, response) = call(
        &router,
        "UntagResource",
        &json!({"ResourceArn": arn, "TagKeys": ["team", "never-set"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UntagResource to answer 200 for a license, got {status} {response}"
    );
    let (_, listed) = call(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        tags(&listed),
        vec![
            ("env".to_string(), "prod".to_string()),
            ("owner".to_string(), "platform".to_string())
        ],
        "UntagResource must drop only the keys it named: {listed}"
    );

    // Tagging does not touch the license itself.
    let stored = get_ok(&router, "tagged at create", &arn).await;
    assert_round_trip("tagged at create", &stored, &body, &arn);
}

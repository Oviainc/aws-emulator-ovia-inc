// SPDX-License-Identifier: CC0-1.0

//! `Service$effectivePolicyValues`, end to end over HTTP: the values a service has in
//! force, derived from the resilience policy its `policyArn` names.
//!
//! The rule under test: a response member whose value is DERIVED from another stored
//! resource is resolved from that resource every time the body is projected — so a
//! `Service` reports one `EffectivePolicyValues` entry per target the attached policy
//! sets (that target's stored value, the policy's `name`, and whether the policy sits in
//! the service's own account), no entry for a target the policy leaves unset, and the
//! member itself is always present: a service with no policy attached, or one whose
//! `policyArn` no longer resolves to a stored policy, reports the empty object while the
//! pointer itself survives.
//!
//! Every suite drives the real wire path (`MultiServiceRouter::dispatch`), so the
//! modelled constraints, the restJson1 (de)serializers and the HTTP status codes are
//! exercised exactly as a client would exercise them.
//!
//! The suites are TABLES: each row is one input of the class the rule covers (which
//! target blocks the policy sets, target values at their modelled bounds, each disaster
//! recovery approach, no policy at all, a policy ARN that resolves to nothing, a policy
//! swapped for another one), not one recorded request.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes these suites drive.
const CREATE_POLICY: &str = "/v2/create-policy";
const CREATE_SERVICE: &str = "/v2/create-service";
const GET_SERVICE: &str = "/v2/get-service";
const UPDATE_SERVICE: &str = "/v2/update-service";

/// The Region these suites sign for.
const REGION: &str = "us-east-1";

/// The account every emulated ARN carries, and so the account every stored policy is
/// owned by.
const ACCOUNT: &str = "000000000000";

/// `PolicyValueSource`'s value for a policy in the reading service's own account.
///
/// SOURCE: `smithy_query(query="shape", shape="PolicyValueSource")` — the enum's two
/// values are `SELF` and `CROSS_ACCOUNT`.
const SOURCE_SELF: &str = "SELF";

/// Every `EffectivePolicyValues` member, paired with the policy block and member its
/// value is derived from.
///
/// SOURCE: `smithy_query(query="shape",
/// shape="EffectivePolicyValues,AvailabilitySlo,MultiAzTargets,MultiRegionTargets,DataRecoveryTargets")`
/// — the eight per-target members of `EffectivePolicyValues` and the policy targets that
/// carry the same quantity.
const DERIVATIONS: [(&str, &str, &str); 8] = [
    ("availabilitySlo", "availabilitySlo", "target"),
    ("multiAzRto", "multiAz", "rtoInMinutes"),
    ("multiAzRpo", "multiAz", "rpoInMinutes"),
    ("multiAzDrApproach", "multiAz", "disasterRecoveryApproach"),
    ("multiRegionRto", "multiRegion", "rtoInMinutes"),
    ("multiRegionRpo", "multiRegion", "rpoInMinutes"),
    (
        "multiRegionDrApproach",
        "multiRegion",
        "disasterRecoveryApproach",
    ),
    (
        "dataRecoveryTimeBetweenBackups",
        "dataRecovery",
        "timeBetweenBackupsInMinutes",
    ),
];

/// The values `MultiAzDisasterRecoveryApproach` and `MultiRegionDisasterRecoveryApproach`
/// accept on the wire.
///
/// SOURCE: `smithy_query(query="shape",
/// shape="MultiAzDisasterRecoveryApproach,MultiRegionDisasterRecoveryApproach")` — both
/// enums carry exactly these five values.
const DISASTER_RECOVERY_APPROACHES: [&str; 5] = [
    "ACTIVE_ACTIVE",
    "HOT_STANDBY",
    "WARM_STANDBY",
    "PILOT_LIGHT",
    "BACKUP_AND_RESTORE",
];

/// `AvailabilitySlo$target`'s modelled range.
///
/// SOURCE: `smithy_query(query="shape", shape="AvailabilitySlo")` — `range {min: 0,
/// max: 100}`.
const AVAILABILITY_SLO_MIN: f64 = 0.0;
const AVAILABILITY_SLO_MAX: f64 = 100.0;

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`resiliencehub`) and the Region ARNs are minted in. Signature verification is
/// off for these tests, so the value only has to parse.
fn signed() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY00000000/20260101/{REGION}\
         /resiliencehub/aws4_request, SignedHeaders=host;x-amz-date, \
         Signature=0123456789abcdef"
    )
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", signed())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create the policy `body` describes and answer its minted ARN.
async fn create_policy(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, answered) = call(router, "POST", CREATE_POLICY, Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating the policy expected HTTP 200, got {status} {answered:?}"
    );
    answered["policy"]["policyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no policy.policyArn in {answered:?}"))
        .to_string()
}

/// `CreateService` with the minimal required members, optionally attaching `policy_arn`.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    policy_arn: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({
        "name": name,
        "regions": [REGION],
        "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
    });
    if let Some(policy_arn) = policy_arn {
        body["policyArn"] = json!(policy_arn);
    }
    call(router, "POST", CREATE_SERVICE, Some(body)).await
}

async fn get_service(router: &MultiServiceRouter, service_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{GET_SERVICE}?serviceArn={service_arn}"),
        None,
    )
    .await
}

/// The `service` member of a successful create, read or update.
fn service_of(case: &str, status: u16, body: &Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} {body:?}"
    );
    let service = &body["service"];
    assert!(
        service.is_object(),
        "case {case:?}: the @required service member is absent from {body:?}"
    );
    service.clone()
}

fn service_arn_of(case: &str, service: &Value) -> String {
    service["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no serviceArn in {service:?}"))
        .to_string()
}

/// The rule, written once: every `EffectivePolicyValues` entry the policy `body`
/// describes, each the policy's stored target value together with the policy's `name` and
/// the account the value came from. A target the policy does not set derives nothing, so
/// it contributes no entry — and a policy that sets none at all derives the empty object.
fn derived_from(policy_body: &Value, policy_name: &str, source: &str) -> Value {
    let mut expected = Map::new();
    for (member, block, target) in DERIVATIONS {
        let value = policy_body
            .get(block)
            .and_then(|block| block.get(target))
            .filter(|value| !value.is_null());
        if let Some(value) = value {
            expected.insert(
                member.to_string(),
                json!({
                    "value": value.clone(),
                    "policyName": policy_name,
                    "source": source,
                }),
            );
        }
    }
    Value::Object(expected)
}

/// Whether `actual` and `expected` are the same JSON value, comparing numbers by their
/// numeric value so an integral double matches whichever way a serializer punctuated it.
fn same_json(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) => {
            match (actual.as_f64(), expected.as_f64()) {
                (Some(actual), Some(expected)) => actual == expected,
                _ => false,
            }
        }
        (Value::Object(actual), Value::Object(expected)) => {
            actual.len() == expected.len()
                && actual.iter().all(|(key, value)| {
                    expected
                        .get(key)
                        .is_some_and(|expected| same_json(value, expected))
                })
        }
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| same_json(actual, expected))
        }
        _ => actual == expected,
    }
}

/// Assert that `service` reports exactly `expected` as its `effectivePolicyValues`, the
/// member being PRESENT either way.
fn assert_effective(case: &str, service: &Value, expected: &Value) {
    let actual = &service["effectivePolicyValues"];
    assert!(
        actual.is_object(),
        "case {case:?}: effectivePolicyValues is absent or not an object in {service:?}"
    );
    assert!(
        same_json(actual, expected),
        "case {case:?}: expected effectivePolicyValues {expected}, got {actual}"
    );
}

/// One row of the derivation table: the target blocks a policy sets.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    name: &'static str,
    /// The `CreatePolicy` target blocks, merged into the request beside `name`.
    targets: Value,
}

fn derivation_rows() -> Vec<Row> {
    vec![
        Row {
            label: "a policy with no targets at all derives nothing",
            name: "NoTargets",
            targets: json!({}),
        },
        Row {
            label: "availability SLO at the modelled minimum",
            name: "SloAtMinimum",
            targets: json!({"availabilitySlo": {"target": AVAILABILITY_SLO_MIN}}),
        },
        Row {
            label: "availability SLO at the modelled maximum",
            name: "SloAtMaximum",
            targets: json!({"availabilitySlo": {"target": AVAILABILITY_SLO_MAX}}),
        },
        Row {
            label: "a fractional availability SLO",
            name: "SloFractional",
            targets: json!({"availabilitySlo": {"target": 99.95}}),
        },
        Row {
            label: "multi-AZ RTO and RPO, RTO at the modelled minimum",
            name: "MultiAzRtoRpo",
            targets: json!({"multiAz": {"rtoInMinutes": 0, "rpoInMinutes": 15}}),
        },
        Row {
            label: "a multi-AZ disaster recovery approach alone",
            name: "MultiAzApproachOnly",
            targets: json!({"multiAz": {"disasterRecoveryApproach": "WARM_STANDBY"}}),
        },
        Row {
            label: "multi-Region RTO, RPO and approach",
            name: "MultiRegionAll",
            targets: json!({
                "multiRegion": {
                    "rtoInMinutes": 240,
                    "rpoInMinutes": 60,
                    "disasterRecoveryApproach": "PILOT_LIGHT",
                }
            }),
        },
        Row {
            label: "data recovery alone",
            name: "DataRecoveryOnly",
            targets: json!({"dataRecovery": {"timeBetweenBackupsInMinutes": 1440}}),
        },
        Row {
            label: "every target block set at once",
            name: "EveryTargetBlock",
            targets: json!({
                "availabilitySlo": {"target": 99.9},
                "multiAz": {
                    "rtoInMinutes": 30,
                    "rpoInMinutes": 5,
                    "disasterRecoveryApproach": "ACTIVE_ACTIVE",
                },
                "multiRegion": {
                    "rtoInMinutes": 1440,
                    "rpoInMinutes": 720,
                    "disasterRecoveryApproach": "BACKUP_AND_RESTORE",
                },
                "dataRecovery": {"timeBetweenBackupsInMinutes": 0},
            }),
        },
        Row {
            label: "a policy whose only block carries no target member",
            name: "EmptyBlock",
            targets: json!({"multiAz": {}}),
        },
    ]
}

/// A `CreatePolicy` body for `row`.
fn policy_body(row: &Row) -> Value {
    let mut body = json!({"name": row.name});
    for (key, value) in row.targets.as_object().expect("the row's targets are an object") {
        body[key] = value.clone();
    }
    body
}

#[tokio::test]
async fn create_and_get_derive_effective_policy_values_from_the_attached_policy() {
    let router = emulator();
    for row in derivation_rows() {
        let case = row.label;
        let body = policy_body(&row);
        let policy_arn = create_policy(&router, case, body.clone()).await;
        let expected = derived_from(&body, row.name, SOURCE_SELF);

        let (status, answered) =
            create_service(&router, &format!("Svc{}", row.name), Some(&policy_arn)).await;
        let created = service_of(case, status, &answered);
        assert_eq!(
            created["policyArn"].as_str(),
            Some(policy_arn.as_str()),
            "case {case:?}: expected the attached policyArn {policy_arn}, got {:?}",
            created["policyArn"]
        );
        assert_effective(case, &created, &expected);

        // The same projection answers a later read, so the member cannot drift from what
        // the create reported.
        let service_arn = service_arn_of(case, &created);
        let (status, answered) = get_service(&router, &service_arn).await;
        let read_back = service_of(case, status, &answered);
        assert_effective(case, &read_back, &expected);
    }
}

#[tokio::test]
async fn every_disaster_recovery_approach_is_derived_for_both_targets() {
    let router = emulator();
    for (index, approach) in DISASTER_RECOVERY_APPROACHES.into_iter().enumerate() {
        for block in ["multiAz", "multiRegion"] {
            let case = format!("{block} disaster recovery approach {approach}");
            let name = format!("Approach{index}{}", &block[..7]);
            let body = json!({
                "name": name,
                block: {"disasterRecoveryApproach": approach},
            });
            let policy_arn = create_policy(&router, &case, body.clone()).await;
            let expected = derived_from(&body, &name, SOURCE_SELF);

            let (status, answered) =
                create_service(&router, &format!("Svc{name}"), Some(&policy_arn)).await;
            let created = service_of(&case, status, &answered);
            assert_effective(&case, &created, &expected);
        }
    }
}

#[tokio::test]
async fn a_service_with_nothing_to_derive_reports_the_empty_object() {
    let router = emulator();
    // A `policyArn` of the modelled form that names no stored policy: what a service's
    // pointer becomes once the policy it names is gone.
    let dangling = format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:policy/00000000-0000-8000-8000-000000000000");
    let cases: [(&str, &str, Option<&str>); 2] = [
        ("no policy attached at all", "NoPolicyAttached", None),
        (
            "a policyArn that resolves to no stored policy",
            "DanglingPolicyArn",
            Some(dangling.as_str()),
        ),
    ];
    for (case, name, policy_arn) in cases {
        let (status, answered) = create_service(&router, name, policy_arn).await;
        let created = service_of(case, status, &answered);
        assert_effective(case, &created, &json!({}));
        assert_eq!(
            created["policyArn"].as_str(),
            policy_arn,
            "case {case:?}: expected the policyArn pointer {policy_arn:?} to survive, got {:?}",
            created["policyArn"]
        );

        let service_arn = service_arn_of(case, &created);
        let (status, answered) = get_service(&router, &service_arn).await;
        let read_back = service_of(case, status, &answered);
        assert_effective(case, &read_back, &json!({}));
        assert_eq!(
            read_back["policyArn"].as_str(),
            policy_arn,
            "case {case:?}: expected the policyArn pointer {policy_arn:?} to survive a read, \
             got {:?}",
            read_back["policyArn"]
        );
    }
}

#[tokio::test]
async fn reattaching_a_policy_rederives_the_values_the_service_reports() {
    let router = emulator();
    let case = "a service whose attached policy is swapped for another";
    let first_body = json!({
        "name": "FirstAttached",
        "availabilitySlo": {"target": 95.5},
        "multiAz": {"rtoInMinutes": 10},
    });
    let second_body = json!({
        "name": "SecondAttached",
        "multiRegion": {"rpoInMinutes": 90, "disasterRecoveryApproach": "HOT_STANDBY"},
        "dataRecovery": {"timeBetweenBackupsInMinutes": 60},
    });
    let first = create_policy(&router, case, first_body.clone()).await;
    let second = create_policy(&router, case, second_body.clone()).await;

    let (status, answered) = create_service(&router, "SwappedPolicy", Some(&first)).await;
    let created = service_of(case, status, &answered);
    assert_effective(
        case,
        &created,
        &derived_from(&first_body, "FirstAttached", SOURCE_SELF),
    );
    let service_arn = service_arn_of(case, &created);

    let (status, answered) = call(
        &router,
        "POST",
        UPDATE_SERVICE,
        Some(json!({"serviceArn": service_arn, "policyArn": second})),
    )
    .await;
    let updated = service_of(case, status, &answered);
    let expected = derived_from(&second_body, "SecondAttached", SOURCE_SELF);
    assert_effective(case, &updated, &expected);

    let (status, answered) = get_service(&router, &service_arn).await;
    let read_back = service_of(case, status, &answered);
    assert_effective(case, &read_back, &expected);
}

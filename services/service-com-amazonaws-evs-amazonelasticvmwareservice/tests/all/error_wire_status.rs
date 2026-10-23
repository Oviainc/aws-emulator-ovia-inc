// SPDX-License-Identifier: CC0-1.0

//! How Amazon EVS puts a failure on the wire: the HTTP status, and the not-found
//! message.
//!
//! The rules under test:
//!
//! * The status of a modeled error is its FAULT, not the exact code its `@httpError`
//!   trait names. Amazon EVS speaks `awsJson1_0`
//!   (`smithy_query(query="service_info")`), and an awsJson front end answers every
//!   modeled client error with HTTP 400 — the body's `__type` is what tells one
//!   client error from another — while a server error keeps its 5xx so a client's
//!   retry classifier still sees a server fault.
//! * An environment this service holds no record of is a `ResourceNotFoundException`
//!   whose message is built from the request: `Environment <requested id> was not
//!   found`, with `resourceId` echoing the requested id and `resourceType` naming the
//!   resource.
//!
//! The harness (`emulator`, `call`, `call_in_region`, `create`) is the one
//! `environment_lifecycle` already owns, so both suites drive the service the same
//! way a client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Op>`.

use serde_json::{json, Value};

use crate::environment_lifecycle::{
    call, call_in_region, create, emulator, error_type, ACCOUNT, REGION,
};
use service_com_amazonaws_evs_amazonelasticvmwareservice::scaffolded::wire_http_status;

/// The resource type the not-found error names for an environment.
const RESOURCE_TYPE: &str = "environment";

/// Well-formed environment ids this service holds no record of, spanning the
/// modelled `^(env-[a-zA-Z0-9]{10})$`: digits, lower case, upper case and mixed.
const ABSENT_IDS: [&str; 5] = [
    "env-0000000000",
    "env-aaaaaaaaaa",
    "env-ZZZZZZZZZZ",
    "env-0A1b2C3d4E",
    "env-9999999999",
];

/// The implemented operations that resolve an environment by id, with the request
/// that asks each one about `id`.
fn lookup_requests(id: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetEnvironment", json!({ "environmentId": id })),
        ("ListEnvironmentVlans", json!({ "environmentId": id })),
        (
            "CreateEntitlement",
            json!({
                "environmentId": id,
                "connectorId": "cnctr-0123456789",
                "entitlementType": "WINDOWS_SERVER",
                "vmIds": ["vm-1"],
            }),
        ),
        (
            "ListVmEntitlements",
            json!({
                "environmentId": id,
                "connectorId": "cnctr-0123456789",
                "entitlementType": "WINDOWS_SERVER",
            }),
        ),
        ("DeleteEnvironment", json!({ "environmentId": id })),
    ]
}

/// A member of an error body, as a string, for an assertion.
fn member(body: &Value, name: &str) -> String {
    body[name].as_str().unwrap_or("<absent>").to_string()
}

#[tokio::test]
async fn an_absent_environment_is_http_400_with_a_request_derived_not_found_message() {
    let router = emulator();
    // One real environment, so a miss is a miss on the lookup and not an empty store.
    let present = create(&router, Some("present")).await;

    for absent in ABSENT_IDS {
        for (operation, request) in lookup_requests(absent) {
            let case = format!("{operation} {absent}");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {}",
                error_type(&body)
            );
            assert_eq!(
                member(&body, "message"),
                format!("Environment {absent} was not found"),
                "case {case:?}: the message must name the requested id, got {body}"
            );
            assert_eq!(
                member(&body, "resourceId"),
                absent,
                "case {case:?}: resourceId must echo the requested id, got {body}"
            );
            assert_eq!(
                member(&body, "resourceType"),
                RESOURCE_TYPE,
                "case {case:?}: unexpected resourceType, got {body}"
            );
        }
    }

    // The environment that does exist is still served, so the 400 above is the
    // lookup's answer and not the operation's answer to everything.
    let (status, body) = call(
        &router,
        "GetEnvironment",
        &json!({ "environmentId": present }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the present environment: expected 200, got {status} {body}"
    );
}

#[tokio::test]
async fn an_environment_asked_for_outside_its_region_misses_with_the_same_400() {
    let router = emulator();
    let home = create(&router, Some("home-region-env")).await;

    for region in ["eu-central-1", "ap-southeast-2", "us-east-1"] {
        for (operation, request) in lookup_requests(&home) {
            let case = format!("{operation} from {region}");
            let (status, body) = call_in_region(&router, region, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                member(&body, "message"),
                format!("Environment {home} was not found"),
                "case {case:?}: the message must name the requested id, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_tagging_surface_reports_a_missing_resource_with_the_same_400() {
    // The tagging operations take a resourceArn rather than an environmentId, and the
    // same status rule holds for them. Their not-found BODY is a different shape (a
    // fixed message plus a `resourceArn` member) and is pinned by
    // `tests/all/tagging_not_found.rs`, so this asserts the status and the error shape
    // only.
    let router = emulator();
    let absent_arn = format!("arn:aws:evs:{REGION}:{ACCOUNT}:environment/env-0000000000");

    let rows: Vec<(&str, Value)> = vec![
        (
            "ListTagsForResource",
            json!({ "resourceArn": absent_arn.clone() }),
        ),
        (
            "TagResource",
            json!({ "resourceArn": absent_arn.clone(), "tags": { "k": "v" } }),
        ),
        (
            "UntagResource",
            json!({ "resourceArn": absent_arn.clone(), "tagKeys": ["k"] }),
        ),
    ];
    for (operation, request) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {operation:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
    }
}

#[tokio::test]
async fn two_different_client_errors_share_the_400_and_differ_by_type() {
    // A client reads the fault from the status and the specific error from `__type`,
    // so one status must carry both of this surface's client errors.
    let router = emulator();

    // Each row: the case, the request, and the error `__type` the rule gives it.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a well-formed id no environment holds",
            json!({ "environmentId": "env-0000000000" }),
            "ResourceNotFoundException",
        ),
        (
            "an id that is not an environment id at all",
            json!({ "environmentId": "not-an-env-id" }),
            "ValidationException",
        ),
        (
            "no environmentId at all",
            json!({}),
            "ValidationException",
        ),
    ];
    for (case, request, expected_type) in rows {
        let (status, body) = call(&router, "GetEnvironment", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_type(&body).contains(expected_type),
            "case {case:?}: expected a {expected_type}, got {}",
            error_type(&body)
        );
    }

    // And the scaffold channel is untouched by the status rule: an operation nobody
    // has implemented yet is still a 501, not a client error.
    let (status, body) = call(&router, "GetVersions", &json!({})).await;
    assert_eq!(
        status, 501,
        "case a scaffolded operation: expected HTTP 501, got {status} {body}"
    );
}

#[test]
fn a_modeled_client_status_renders_as_400_and_a_server_status_is_kept() {
    // Each row: the case, the status the model declares, and the status this service
    // puts on the wire. The declared statuses are this service's own error shapes,
    // from `smithy_query(query="shape", shape="ResourceNotFoundException,
    // ValidationException,ThrottlingException,TagPolicyException,
    // ServiceQuotaExceededException,InternalServerException")`, plus the class bounds
    // (the lowest and highest 4xx, the lowest 5xx, and one past the 4xx range).
    let rows: [(&str, u16, u16); 10] = [
        ("ValidationException @httpError(400)", 400, 400),
        ("ServiceQuotaExceededException @httpError(402)", 402, 400),
        ("TagPolicyException @httpError(400)", 400, 400),
        ("ResourceNotFoundException @httpError(404)", 404, 400),
        ("ThrottlingException @httpError(429)", 429, 400),
        ("InternalServerException @httpError(500)", 500, 500),
        ("the lowest client status", 400, 400),
        ("the highest client status", 499, 400),
        ("the lowest server status", 500, 500),
        ("a server status past the lowest", 503, 503),
    ];
    for (case, modeled, expected) in rows {
        let got = wire_http_status(modeled);
        assert_eq!(
            got, expected,
            "case {case:?}: modeled {modeled} must render as {expected}, got {got}"
        );
    }
}

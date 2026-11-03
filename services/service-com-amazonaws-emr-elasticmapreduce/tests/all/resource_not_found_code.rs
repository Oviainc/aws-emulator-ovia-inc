// SPDX-License-Identifier: CC0-1.0

//! What an Amazon EMR refusal carries BESIDE its message, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so
//! these are the same bytes an SDK client sends.
//!
//! THE RULE: Amazon EMR funnels every bad request through `InvalidRequestException`, so
//! its `ErrorCode` member ("the error code associated with the exception") is the only
//! discriminator a client has. A refusal that reports the resource the request names as
//! ABSENT carries `ResourceNotFound` there — whichever resource and whichever operation
//! was asked — and a refusal of any other class carries no code at all, which is what
//! lets an idempotent teardown swallow "gone" and re-raise everything else.
//!
//! The last table asserts that ABSENCE for the classes Amazon EMR words as rejecting the
//! value rather than reporting an absent resource ("… is not valid."), because the
//! boundary is what gives `ResourceNotFound` its meaning. A class AWS IS known to publish
//! its own code for is asserted in the suite that owns that code: an unresolvable cluster
//! id in `tests/all/cluster_id_refusal_code.rs`.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{assert_refused, at, kept_alive_request, Emr};
use crate::refusal_messages::{message, studio_request, UNKNOWN_STUDIO_IDS};

/// The discriminator Amazon EMR answers for a request naming a resource it holds none
/// of.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

/// A security configuration document every create below is made with: the smallest one
/// the service accepts.
const DOCUMENT: &str =
    r#"{"EncryptionConfiguration":{"EnableInTransitEncryption":false,"EnableAtRestEncryption":false}}"#;

/// `CreateSecurityConfiguration$Name` targets `XmlString`, whose documented length bound
/// is `len 0..10280`
/// (`smithy_query(query="operation", operation="DescribeSecurityConfiguration")`), so a
/// name of the maximum length reaches the handler rather than the boundary validator.
const NAME_MAX_LEN: usize = 10280;

/// The `ErrorCode` member of a refusal, or `None` when the refusal carries none.
fn error_code(payload: &Value) -> Option<String> {
    at(payload, &["ErrorCode"]).as_str().map(str::to_string)
}

/// Assert one absent-resource refusal: HTTP 400, the modelled client error, and the
/// absent-resource discriminator in `ErrorCode`.
fn assert_not_found(case: &str, status: u16, payload: &Value) {
    assert_refused(case, status, payload, "InvalidRequestException");
    assert_eq!(
        error_code(payload).as_deref(),
        Some(RESOURCE_NOT_FOUND),
        "case {case:?}: expected ErrorCode {RESOURCE_NOT_FOUND}, got {:?} in {payload}",
        error_code(payload)
    );
}

/// Names no security configuration is held under. Rows differ along the form of the
/// name, which is what the rule generalises over: a plain one, one carrying spaces,
/// punctuation and digits, a non-ASCII one, a one-character one, the empty string (the
/// member's length bound starts at 0, so the handler sees it) and one of the maximum
/// documented length.
fn absent_names() -> Vec<String> {
    vec![
        "never-created".to_string(),
        "a name with spaces".to_string(),
        "dots.and_dashes-99".to_string(),
        "конфигурация".to_string(),
        "x".to_string(),
        String::new(),
        "n".repeat(NAME_MAX_LEN),
    ]
}

/// The operations that resolve a security configuration by name, with a body naming
/// `name`.
fn security_configuration_requests(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "DescribeSecurityConfiguration",
            json!({ "Name": name }),
        ),
        ("DeleteSecurityConfiguration", json!({ "Name": name })),
    ]
}

/// THE RULE on the security-configuration surface: a name the account holds no
/// configuration under is refused with the absent-resource discriminator and the
/// service's sentence for it, which quotes the `Name` the request sent — whether the
/// name was never created or has just been deleted, and whether the caller described or
/// deleted it.
#[tokio::test]
async fn an_absent_security_configuration_is_refused_with_the_not_found_code() {
    let emr = Emr::new();
    // A configuration EXISTS throughout, so every refusal below is a lookup rather
    // than a blanket refusal of the surface.
    emr.ok(
        "CreateSecurityConfiguration",
        json!({ "Name": "held", "SecurityConfiguration": DOCUMENT }),
    )
    .await;

    for name in absent_names() {
        // Leg one: a name that was never created.
        for (operation, body) in security_configuration_requests(&name) {
            let case = format!("{operation} on the never-created name {name:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_not_found(&case, status, &payload);
            assert_eq!(
                message(&case, &payload),
                format!("Security configuration with name \"{name}\" does not exist."),
                "case {case:?}: unexpected message in {payload}"
            );
        }

        // Leg two: the same name after a create and a delete — the resource is gone, so
        // the refusal is the same one, which is what makes the teardown idempotent.
        emr.ok(
            "CreateSecurityConfiguration",
            json!({ "Name": &name, "SecurityConfiguration": DOCUMENT }),
        )
        .await;
        emr.ok("DeleteSecurityConfiguration", json!({ "Name": &name }))
            .await;
        for (operation, body) in security_configuration_requests(&name) {
            let case = format!("{operation} on the deleted name {name:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_not_found(&case, status, &payload);
            assert_eq!(
                message(&case, &payload),
                format!("Security configuration with name \"{name}\" does not exist."),
                "case {case:?}: unexpected message in {payload}"
            );
        }
    }

    // The held configuration still reads back, so the refusals above resolved names.
    let held = emr
        .ok("DescribeSecurityConfiguration", json!({ "Name": "held" }))
        .await;
    assert_eq!(
        at(&held, &["Name"]).as_str(),
        Some("held"),
        "the held configuration must still read back, got {held}"
    );
}

/// Every operation that resolves a `StudioId`, with a body naming `studio_id`.
///
/// `ListStudioSessionMappings` is here only for a non-empty id: its `StudioId` is an
/// optional FILTER, so the empty string is a listing of every mapping rather than a
/// Studio lookup.
fn studio_id_requests(studio_id: &str) -> Vec<(&'static str, Value)> {
    let identity = json!({
        "StudioId": studio_id,
        "IdentityName": "analyst",
        "IdentityType": "USER",
    });
    let mut mapped = identity.clone();
    mapped["SessionPolicyArn"] = json!("arn:aws:iam::aws:policy/EMRStudioBasic");
    let mut requests = vec![
        ("DescribeStudio", json!({ "StudioId": studio_id })),
        (
            "UpdateStudio",
            json!({ "StudioId": studio_id, "Name": "renamed" }),
        ),
        ("DeleteStudio", json!({ "StudioId": studio_id })),
        ("CreateStudioSessionMapping", mapped.clone()),
        ("GetStudioSessionMapping", identity.clone()),
        ("UpdateStudioSessionMapping", mapped),
        ("DeleteStudioSessionMapping", identity),
    ];
    if !studio_id.is_empty() {
        requests.push((
            "ListStudioSessionMappings",
            json!({ "StudioId": studio_id }),
        ));
    }
    requests
}

/// THE RULE, on the Studio surface: the same discriminator, for the other resources this
/// service holds — a Studio id it has no Studio for, and an identity an existing Studio
/// has no session mapping for. Rows span every Studio-scoped operation, the id forms a
/// caller can send, both `IdentityType` values and both ways an identity is named.
#[tokio::test]
async fn an_absent_studio_or_session_mapping_is_refused_with_the_not_found_code() {
    let emr = Emr::new();
    let live = emr.ok("CreateStudio", studio_request("live")).await["StudioId"]
        .as_str()
        .expect("CreateStudio answers a StudioId")
        .to_string();

    for unknown in UNKNOWN_STUDIO_IDS {
        for (operation, body) in studio_id_requests(unknown) {
            let case = format!("{operation} with the studio id {unknown:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_not_found(&case, status, &payload);
        }
    }

    // An EXISTING Studio with no mapping for the identity asked about: the mapping is
    // the absent resource this time, so the same discriminator identifies it.
    for (identity_member, identity, identity_type) in [
        ("IdentityName", "unmapped-analyst", "USER"),
        ("IdentityId", "9f0e8d7c-1234-4321-abcd-00000000beef", "USER"),
        ("IdentityName", "unmapped-analysts", "GROUP"),
        ("IdentityId", "1a2b3c4d-5678-8765-dcba-00000000feeb", "GROUP"),
    ] {
        let mut body = json!({ "StudioId": &live, "IdentityType": identity_type });
        body[identity_member] = json!(identity);
        let mut mapped = body.clone();
        mapped["SessionPolicyArn"] = json!("arn:aws:iam::aws:policy/EMRStudioBasic");
        for (operation, request) in [
            ("GetStudioSessionMapping", body.clone()),
            ("DeleteStudioSessionMapping", body.clone()),
            ("UpdateStudioSessionMapping", mapped),
        ] {
            let case = format!("{operation} for the unmapped {identity_type} {identity:?}");
            let (status, payload) = emr.call(operation, request).await;
            assert_not_found(&case, status, &payload);
            let said = message(&case, &payload);
            assert!(
                said.contains(&live) && said.contains(identity),
                "case {case:?}: the refusal names the Studio and the identity, got {said:?}"
            );
        }
    }

    // The live Studio still resolves, so every refusal above was a lookup.
    let studio = emr.ok("DescribeStudio", json!({ "StudioId": &live })).await;
    assert_eq!(
        at(&studio, &["Studio", "StudioId"]).as_str(),
        Some(live.as_str()),
        "the live studio must still read back, got {studio}"
    );
}

/// THE OTHER HALF of the rule: a refusal that is NOT about an absent resource and that
/// Amazon EMR publishes no code of its own for carries no discriminator, so
/// `ResourceNotFound` means exactly one thing. Rows span those classes — a malformed
/// document, a name already taken, a mutually exclusive pair of members and a step id
/// Amazon EMR rejects as not valid — across the modelled `InvalidRequestException`
/// channel and the `ValidationException` one.
#[tokio::test]
async fn a_refusal_of_another_class_carries_no_discriminator() {
    let emr = Emr::new();
    emr.ok(
        "CreateSecurityConfiguration",
        json!({ "Name": "taken", "SecurityConfiguration": DOCUMENT }),
    )
    .await;
    let live_studio = emr.ok("CreateStudio", studio_request("live")).await["StudioId"]
        .as_str()
        .expect("CreateStudio answers a StudioId")
        .to_string();
    let live_cluster = emr.run_job_flow(kept_alive_request("live")).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a name already taken by a different document",
            "CreateSecurityConfiguration",
            json!({
                "Name": "taken",
                "SecurityConfiguration":
                    r#"{"EncryptionConfiguration":{"EnableInTransitEncryption":false,"EnableAtRestEncryption":true,"AtRestEncryptionConfiguration":{"S3EncryptionConfiguration":{"EncryptionMode":"SSE-S3"}}}}"#,
            }),
        ),
        (
            "a document that is not JSON",
            "CreateSecurityConfiguration",
            json!({ "Name": "malformed", "SecurityConfiguration": "{not json" }),
        ),
        (
            "both ways of naming one identity",
            "GetStudioSessionMapping",
            json!({
                "StudioId": &live_studio,
                "IdentityId": "9f0e8d7c-1234-4321-abcd-00000000beef",
                "IdentityName": "analyst",
                "IdentityType": "USER",
            }),
        ),
        (
            "a step id Amazon EMR rejects as not valid",
            "DescribeStep",
            json!({ "ClusterId": &live_cluster, "StepId": "s-ZZZZZZZZZZZZZ" }),
        ),
    ];

    for (case, operation, body) in cases {
        let (status, payload) = emr.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            None,
            "case {case:?}: {operation} must carry no ErrorCode, got {:?} in {payload}",
            error_code(&payload)
        );
    }
}

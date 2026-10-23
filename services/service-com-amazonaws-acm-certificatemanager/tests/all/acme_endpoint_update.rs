// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAcmeEndpoint` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.UpdateAcmeEndpoint`, awsJson1_1).
//!
//! RULE: `UpdateAcmeEndpoint` writes the configuration members the request NAMES
//! (`AuthorizationBehavior`, `Contact`, `CertificateAuthority`) onto the stored ACME
//! endpoint its `AcmeEndpointArn` identifies, leaves every member it omits — and
//! everything the operation models no member for — as it stands, dates the record with
//! `UpdatedAt` when a value actually changes, and answers an empty body; an ARN with no
//! record behind it is refused `ResourceNotFoundException`, and an endpoint whose
//! configuration ACM is still acting on (any status other than `ACTIVE`) is refused
//! `ConflictException`.
//!
//! Every write goes through `UpdateAcmeEndpoint` and every read through
//! `DescribeAcmeEndpoint` or `ListTagsForResource`, so a response that is not computed
//! from the persisted store cannot pass. The wire harness, the model's value catalogs
//! and the create/describe helpers are shared with [`crate::acme_endpoint_lifecycle`]
//! and [`crate::certificate_lifecycle`].
//!
//! The table spans what the rule generalises over: WHICH members a request names (each
//! one alone, all of them, and none), and the FORM the certificate authority takes (no
//! algorithm list, one algorithm, every published algorithm, an empty list, the same
//! algorithms in another order) — not the single configuration the stub issue quoted.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_endpoint_lifecycle::{
    create_endpoint, describe_endpoint, hold_activations, restore_activations, wait_until,
    AUTHORIZATION_BEHAVIORS, CONTACTS, PUBLIC_KEY_ALGORITHMS,
};
use crate::certificate_lifecycle::{
    call, error_code, error_message, router, router_with_engine, ACCOUNT, REGION,
};

/// One `UpdateAcmeEndpoint` case: what it exercises, the `CreateAcmeEndpoint` body the
/// endpoint starts from, the update to send (without the ARN, which the test fills in),
/// the configuration the endpoint must report afterwards, and whether the update asks
/// for a change at all.
struct UpdateCase {
    case: &'static str,
    create: Value,
    update: Value,
    expect_behavior: &'static str,
    expect_contact: &'static str,
    /// The `AllowedKeyAlgorithms` the describe must report — `Value::Null` for an
    /// endpoint whose certificate authority names no algorithm list.
    expect_algorithms: Value,
    changed: bool,
}

/// A `CreateAcmeEndpoint` body: the authorization behavior, the contact requirement and
/// the certificate authority to start an endpoint from.
fn create_body(contact: &str, authority: Value) -> Value {
    json!({
        "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
        "Contact": contact,
        "CertificateAuthority": { "PublicCertificateAuthority": authority },
    })
}

/// A `PublicCertificateAuthority` naming `algorithms` as its allowed key algorithms.
fn authority(algorithms: &[&str]) -> Value {
    json!({ "AllowedKeyAlgorithms": algorithms })
}

/// A `CertificateAuthority` union value naming `algorithms`, for an update body.
fn update_authority(algorithms: &[&str]) -> Value {
    json!({ "PublicCertificateAuthority": authority(algorithms) })
}

/// Send `UpdateAcmeEndpoint` for `arn` with `update`'s members and return
/// `(status, body)`.
async fn update_endpoint(
    router: &MultiServiceRouter,
    arn: &str,
    update: &Value,
) -> (u16, Value) {
    let mut body = update.clone();
    body["AcmeEndpointArn"] = json!(arn);
    call(router, "UpdateAcmeEndpoint", &body).await
}

/// The resource tags `ListTagsForResource` reports for `arn`, as it reports them (a
/// `BTreeMap` behind it, so the order is deterministic and two reads compare equal).
async fn resource_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) =
        call(router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}) must succeed, got {status} {response}"
    );
    response["Tags"].clone()
}

/// Every member of `AcmeEndpoint` this operation models no input member for, so every
/// one an update must leave exactly as it found it
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeEndpoint")` beside
/// `smithy_query(query="operation", operation="UpdateAcmeEndpoint")`).
const UNTOUCHED_MEMBERS: &[&str] = &[
    "AcmeEndpointArn",
    "EndpointUrl",
    "Status",
    "FailureReason",
    "CertificateTags",
    "CreatedAt",
];

/// One row per class of update the rule covers.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "Contact alone",
            create: create_body(CONTACTS[0], json!({})),
            update: json!({ "Contact": CONTACTS[1] }),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[1],
            expect_algorithms: Value::Null,
            changed: true,
        },
        UpdateCase {
            // `AcmeAuthorizationBehavior` publishes exactly one value, so naming it is
            // the one single-member update that asks for no change.
            case: "AuthorizationBehavior alone, the value the endpoint already carries",
            create: create_body(CONTACTS[1], authority(&[PUBLIC_KEY_ALGORITHMS[0]])),
            update: json!({ "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0] }),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[1],
            expect_algorithms: json!([PUBLIC_KEY_ALGORITHMS[0]]),
            changed: false,
        },
        UpdateCase {
            case: "CertificateAuthority alone, from no algorithm list to every published algorithm",
            create: create_body(CONTACTS[0], json!({})),
            update: json!({ "CertificateAuthority": update_authority(PUBLIC_KEY_ALGORITHMS) }),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[0],
            expect_algorithms: json!(PUBLIC_KEY_ALGORITHMS),
            changed: true,
        },
        UpdateCase {
            case: "CertificateAuthority narrowed to one algorithm",
            create: create_body(CONTACTS[1], authority(PUBLIC_KEY_ALGORITHMS)),
            update: json!({
                "CertificateAuthority": update_authority(&[PUBLIC_KEY_ALGORITHMS[1]]),
            }),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[1],
            expect_algorithms: json!([PUBLIC_KEY_ALGORITHMS[1]]),
            changed: true,
        },
        UpdateCase {
            // `PublicKeyAlgorithmList` carries no length constraint, so the empty list
            // is a value the member can take and is a different configuration.
            case: "CertificateAuthority emptied to an empty algorithm list",
            create: create_body(CONTACTS[0], authority(&[PUBLIC_KEY_ALGORITHMS[2]])),
            update: json!({ "CertificateAuthority": update_authority(&[]) }),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[0],
            expect_algorithms: json!([]),
            changed: true,
        },
        UpdateCase {
            // The member is a LIST, so its order is the caller's: the same algorithms
            // listed the other way round is a changed configuration.
            case: "the same algorithms in another order",
            create: create_body(
                CONTACTS[0],
                authority(&[PUBLIC_KEY_ALGORITHMS[0], PUBLIC_KEY_ALGORITHMS[1]]),
            ),
            update: json!({
                "CertificateAuthority": update_authority(&[
                    PUBLIC_KEY_ALGORITHMS[1],
                    PUBLIC_KEY_ALGORITHMS[0],
                ]),
            }),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[0],
            expect_algorithms: json!([PUBLIC_KEY_ALGORITHMS[1], PUBLIC_KEY_ALGORITHMS[0]]),
            changed: true,
        },
        UpdateCase {
            case: "every member at once, over a tagged endpoint",
            create: json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": CONTACTS[0],
                "CertificateAuthority": {
                    "PublicCertificateAuthority": authority(&[PUBLIC_KEY_ALGORITHMS[0]]),
                },
                "Tags": [{ "Key": "owner", "Value": "équipe-sécurité" }],
                "CertificateTags": [{ "Key": "環境", "Value": "本番" }],
            }),
            update: json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": CONTACTS[1],
                "CertificateAuthority": update_authority(&[
                    PUBLIC_KEY_ALGORITHMS[2],
                    PUBLIC_KEY_ALGORITHMS[0],
                ]),
            }),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[1],
            expect_algorithms: json!([PUBLIC_KEY_ALGORITHMS[2], PUBLIC_KEY_ALGORITHMS[0]]),
            changed: true,
        },
        UpdateCase {
            case: "no member named at all",
            create: create_body(CONTACTS[0], json!({})),
            update: json!({}),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[0],
            expect_algorithms: Value::Null,
            changed: false,
        },
        UpdateCase {
            case: "the whole configuration the endpoint already carries",
            create: create_body(CONTACTS[1], authority(&[PUBLIC_KEY_ALGORITHMS[1]])),
            update: json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": CONTACTS[1],
                "CertificateAuthority": update_authority(&[PUBLIC_KEY_ALGORITHMS[1]]),
            }),
            expect_behavior: AUTHORIZATION_BEHAVIORS[0],
            expect_contact: CONTACTS[1],
            expect_algorithms: json!([PUBLIC_KEY_ALGORITHMS[1]]),
            changed: false,
        },
    ]
}

/// RULE: for every row of the table, `UpdateAcmeEndpoint` persists the members the
/// request named, leaves the rest of the record alone, and `DescribeAcmeEndpoint`
/// reports the result; re-sending the same update (the operation is `@idempotent`)
/// changes nothing further.
#[tokio::test]
async fn update_persists_the_members_the_request_names() {
    let router = router();
    let cases = update_cases();

    // One endpoint per row, all created first so the activation the creates owe them
    // can be waited out once on the real clock: this operation applies to a settled
    // (`ACTIVE`) endpoint, which is the state these rows are about.
    let mut arns = Vec::new();
    for case in &cases {
        arns.push(create_endpoint(&router, case.case, &case.create).await);
    }
    std::thread::sleep(state::ACME_ENDPOINT_ACTIVATION_DELAY + Duration::from_millis(100));

    for (case, arn) in cases.iter().zip(&arns) {
        let label = case.case;
        let before = describe_endpoint(&router, label, arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(state::ACME_ENDPOINT_STATUS_ACTIVE),
            "case {label:?}: the endpoint must have settled before the update, got {before}"
        );
        let tags_before = resource_tags(&router, label, arn).await;

        let (status, response) = update_endpoint(&router, arn, &case.update).await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateAcmeEndpoint must succeed, got {status} {response}"
        );
        // The operation's output shape is `Unit`: a success carries no member.
        assert!(
            response
                .as_object()
                .is_none_or(|members| members.is_empty()),
            "case {label:?}: UpdateAcmeEndpoint models no output member, got {response}"
        );

        let after = describe_endpoint(&router, label, arn).await;
        assert_eq!(
            after["AuthorizationBehavior"].as_str(),
            Some(case.expect_behavior),
            "case {label:?}: expected AuthorizationBehavior {:?}, got {after}",
            case.expect_behavior
        );
        assert_eq!(
            after["Contact"].as_str(),
            Some(case.expect_contact),
            "case {label:?}: expected Contact {:?}, got {after}",
            case.expect_contact
        );
        assert_eq!(
            after["CertificateAuthority"]["PublicCertificateAuthority"]["AllowedKeyAlgorithms"],
            case.expect_algorithms,
            "case {label:?}: expected AllowedKeyAlgorithms {}, got {after}",
            case.expect_algorithms
        );

        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                after[member], before[member],
                "case {label:?}: {member} is not this operation's to change, expected {}, got {}",
                before[member], after[member]
            );
        }
        assert_eq!(
            resource_tags(&router, label, arn).await,
            tags_before,
            "case {label:?}: the update must leave the endpoint's resource tags alone"
        );

        // `UpdatedAt` is "the time at which the ACME endpoint was last updated": it
        // moves exactly when the update changed something.
        let updated_before = before["UpdatedAt"].as_f64().unwrap_or_default();
        let updated_after = after["UpdatedAt"].as_f64().unwrap_or_default();
        if case.changed {
            assert!(
                updated_after > updated_before,
                "case {label:?}: a configuration change must move UpdatedAt past {updated_before}, got {updated_after}"
            );
            assert!(
                updated_after >= after["CreatedAt"].as_f64().unwrap_or_default(),
                "case {label:?}: UpdatedAt must not precede CreatedAt, got {after}"
            );
        } else {
            assert_eq!(
                updated_after, updated_before,
                "case {label:?}: an update that asks for no change must leave UpdatedAt where it was, got {after}"
            );
        }

        // `@idempotent`: the same update sent again writes the same configuration, so
        // the whole record — `UpdatedAt` included — stands exactly as it is.
        let (status, response) = update_endpoint(&router, arn, &case.update).await;
        assert_eq!(
            status, 200,
            "case {label:?}: re-sending the update must succeed, got {status} {response}"
        );
        let again = describe_endpoint(&router, label, arn).await;
        assert_eq!(
            again, after,
            "case {label:?}: re-sending the same update must change nothing, got {again}"
        );
    }

    // Each row's endpoint holds its OWN configuration: one update reached one record.
    for (case, arn) in cases.iter().zip(&arns) {
        let endpoint = describe_endpoint(&router, case.case, arn).await;
        assert_eq!(
            endpoint["Contact"].as_str(),
            Some(case.expect_contact),
            "case {:?}: every other endpoint's update must have left this one alone, got {endpoint}",
            case.case
        );
        assert_eq!(
            endpoint["CertificateAuthority"]["PublicCertificateAuthority"]
                ["AllowedKeyAlgorithms"],
            case.expect_algorithms,
            "case {:?}: every other endpoint's update must have left this one alone, got {endpoint}",
            case.case
        );
    }
}

/// RULE: an `AcmeEndpointArn` with no record behind it is refused
/// `ResourceNotFoundException` (HTTP 400) and nothing is created for it.
#[tokio::test]
async fn update_of_an_unknown_endpoint_is_not_found() {
    let router = router();
    // One endpoint really does exist, so the refusal below is about the ARN the request
    // named rather than about an empty store.
    let existing = create_endpoint(
        &router,
        "a stored endpoint",
        &create_body(CONTACTS[0], json!({})),
    )
    .await;

    // Well-formed `AcmeEndpointArn` values (the modelled `@pattern` is
    // `^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$`) that name
    // no stored endpoint: an id nothing minted, the SAME id in another Region and under
    // another account, and the longest id the member's 200-character length allows.
    let endpoint_id = existing
        .rsplit_once("acme-endpoint/")
        .expect("the created ARN is an ACME endpoint ARN")
        .1
        .to_string();
    let long_id = "a".repeat(
        200 - format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/").len(),
    );
    let missing = [
        (
            "an id nothing minted",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/0000-no-such-endpoint"),
        ),
        (
            "the stored id in another Region",
            format!("arn:aws:acm:eu-west-1:{ACCOUNT}:acme-endpoint/{endpoint_id}"),
        ),
        (
            "the stored id under another account",
            format!("arn:aws:acm:{REGION}:123456789012:acme-endpoint/{endpoint_id}"),
        ),
        (
            "an id at AcmeEndpointArn's maximum length",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/{long_id}"),
        ),
    ];

    for (case, arn) in missing {
        let (status, response) =
            update_endpoint(&router, &arn, &json!({ "Contact": CONTACTS[1] })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: updating {arn} must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            state::acme_endpoint_not_found_message(&arn),
            "case {case:?}: expected the service's not-found sentence, got {response}"
        );

        // The refusal created nothing: the ARN still has no record behind it, and the
        // endpoint that does exist keeps the configuration it was created with.
        let (status, response) = call(
            &router,
            "DescribeAcmeEndpoint",
            &json!({ "AcmeEndpointArn": arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused update must create no endpoint for {arn}, got {status} {response}"
        );
        let stored = describe_endpoint(&router, case, &existing).await;
        assert_eq!(
            stored["Contact"].as_str(),
            Some(CONTACTS[0]),
            "case {case:?}: a refused update must leave the stored endpoint alone, got {stored}"
        );
    }
}

/// RULE: an endpoint whose configuration ACM is still acting on — any status other than
/// `ACTIVE`, which for a freshly created endpoint is the `CREATING` the activation has
/// not yet moved — is refused `ConflictException` (HTTP 400) and keeps the
/// configuration it carries; once the activation falls due the same update is accepted.
#[tokio::test]
async fn update_of_an_unsettled_endpoint_is_a_conflict() {
    let (router, engine) = router_with_engine();

    // Both forms of update, so the refusal is about the endpoint's state and not about
    // which member the request named.
    let rows = [
        ("Contact while CREATING", json!({ "Contact": CONTACTS[1] })),
        (
            "CertificateAuthority while CREATING",
            json!({ "CertificateAuthority": update_authority(PUBLIC_KEY_ALGORITHMS) }),
        ),
    ];

    for (case, update) in rows {
        let arn = create_endpoint(&router, case, &create_body(CONTACTS[0], json!({}))).await;
        // Pin the transient status: the activation this create owes the endpoint is
        // pushed out of the test's way, so `CREATING` cannot race the wall clock.
        hold_activations(&engine);
        let creating = describe_endpoint(&router, case, &arn).await;
        assert_eq!(
            creating["Status"].as_str(),
            Some(state::ACME_ENDPOINT_STATUS_CREATING),
            "case {case:?}: the endpoint must still be CREATING, got {creating}"
        );

        let (status, response) = update_endpoint(&router, &arn, &update).await;
        assert_eq!(
            status, 400,
            "case {case:?}: updating an unsettled endpoint must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response}"
        );
        assert!(
            error_message(&response).contains(state::ACME_ENDPOINT_STATUS_CREATING),
            "case {case:?}: the refusal must name the status that blocks the update, got {response}"
        );

        // The refusal wrote nothing: the record still carries what the create stored.
        let unchanged = describe_endpoint(&router, case, &arn).await;
        assert_eq!(
            unchanged, creating,
            "case {case:?}: a refused update must leave the record alone, got {unchanged}"
        );

        // Put the activation back where the create fixed it, let the span pass, and the
        // very same update is accepted.
        wait_until(restore_activations(&engine));
        let (status, response) = update_endpoint(&router, &arn, &update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: once the endpoint is ACTIVE the same update must succeed, got {status} {response}"
        );
        let updated = describe_endpoint(&router, case, &arn).await;
        assert_eq!(
            updated["Status"].as_str(),
            Some(state::ACME_ENDPOINT_STATUS_ACTIVE),
            "case {case:?}: the endpoint must be ACTIVE now, got {updated}"
        );
        if let Some(contact) = update.get("Contact").and_then(Value::as_str) {
            assert_eq!(
                updated["Contact"].as_str(),
                Some(contact),
                "case {case:?}: expected the accepted update's Contact to be stored, got {updated}"
            );
        }
        if let Some(authority) = update.get("CertificateAuthority") {
            assert_eq!(
                updated["CertificateAuthority"]["PublicCertificateAuthority"]
                    ["AllowedKeyAlgorithms"],
                authority["PublicCertificateAuthority"]["AllowedKeyAlgorithms"],
                "case {case:?}: expected the accepted update's certificate authority to be stored, got {updated}"
            );
        }
    }
}

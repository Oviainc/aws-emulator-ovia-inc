// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the HTTP STATUS this service answers a "no such resource" refusal
//! with, driven over the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the
//! same router the emulator boots, so every assertion is about what a real SDK client
//! reads off the status line.
//!
//! RULE under test: every modelled `ResourceNotFoundException` of this service travels
//! with its one client-error status — `scaffolded::NOT_FOUND_STATUS` — and never with the
//! `@httpError 404` its shape is annotated with, because on this RPC envelope 404 belongs
//! to the transport (it is what the endpoint answers a request that addressed no operation
//! at all), while the error class travels in the body's `__type`. The body is unchanged by
//! the rule: the code stays `ResourceNotFoundException` and the message stays the
//! service's own template built from the request's identifier.
//!
//! The rows span what the rule generalises over: the operation and its kind (reader,
//! writer, delete, reference resolver), the resource kind the id names, the member the id
//! arrives in, and how the id fails to resolve (never minted, deleted, or holding an id of
//! another resource kind). The tagging operations are not among them: they validate their
//! `ResourceARN` by resolving it and answer `ValidationException` for an ARN that resolves
//! to nothing (see `tagging.rs`).

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::{wire_status, NOT_FOUND_STATUS};
use service_com_amazonaws_b2bi_b2bi::state::{
    capability_not_found, partnership_not_found, profile_not_found, transformer_job_not_found,
    transformer_not_found, transformer_reference_not_found,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, capability_configuration, error_code, given_capability, given_partnership,
    given_profile, given_transformer, text, ACCOUNT,
};

/// The statuses the model annotates this service's error shapes with
/// (`smithy_query(query="shape",shape="ResourceNotFoundException,ConflictException,ValidationException,ThrottlingException,AccessDeniedException,ServiceQuotaExceededException,InternalServerException")`).
const MODELLED_STATUSES: &[(&str, u16)] = &[
    ("ResourceNotFoundException", 404),
    ("ValidationException", 400),
    ("AccessDeniedException", 403),
    ("ConflictException", 409),
    ("ServiceQuotaExceededException", 402),
    ("ThrottlingException", 429),
    ("InternalServerException", 500),
];

/// Create one resource of every kind, delete a transformer and a profile, and answer the
/// ids the table below asks about: `(live_profile, live_capability, live_partnership,
/// live_transformer, deleted_transformer, deleted_profile)`.
async fn fixtures(router: &MultiServiceRouter) -> [String; 6] {
    let profile_id = given_profile(router, "not-found-status").await;
    let capability_id = given_capability(router, "not-found-status-capability").await;
    let partnership_id =
        given_partnership(router, &profile_id, &capability_id, "not-found-partner").await;
    let transformer_id = given_transformer(router, "not-found-status-transformer").await;

    // A transformer created and then deleted: the recorded lifecycle's own teardown read.
    let doomed_transformer = given_transformer(router, "doomed-transformer").await;
    let (status, body) = call(
        router,
        "DeleteTransformer",
        json!({ "transformerId": doomed_transformer }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the transformer under test must be deletable, got {status} {body}"
    );

    // And a profile created and then deleted, so a well-formed ARN resolves to nothing.
    let doomed_profile = given_profile(router, "doomed-profile").await;
    let (status, body) = call(
        router,
        "DeleteProfile",
        json!({ "profileId": doomed_profile }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the profile under test must be deletable, got {status} {body}"
    );

    [
        profile_id,
        capability_id,
        partnership_id,
        transformer_id,
        doomed_transformer,
        doomed_profile,
    ]
}

/// RULE: a request naming a resource this account's store does not hold is refused with
/// the modelled `ResourceNotFoundException` at [`NOT_FOUND_STATUS`] — on EVERY operation
/// that resolves an identifier, whatever the resource kind and however the id fails to
/// resolve — with the service's own message naming the id from the request.
#[tokio::test]
async fn every_not_found_refusal_of_this_service_carries_one_client_error_status() {
    let router = b2bi_router();
    let [profile_id, capability_id, partnership_id, transformer_id, deleted_transformer, deleted_profile] =
        fixtures(&router).await;

    // Well-formed per the modelled `len 1..64` / `^[a-zA-Z0-9_-]+$` on every b2bi id, so
    // what refuses them is the lookup rather than the boundary validator.
    let unknown_transformer = "tr-0123456789abcdef0";
    let unknown_profile = "p-0123456789abcdef0";
    let unknown_capability = "ca-0123456789abcdef0";
    let unknown_partnership = "ps-0123456789abcdef0";

    // Each row: the case, the operation, the request, and the message the refusal carries.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            // The recorded lifecycle's teardown read-back: a transformer read straight
            // after its delete.
            "GetTransformer on a deleted transformerId",
            "GetTransformer",
            json!({ "transformerId": deleted_transformer }),
            transformer_not_found(&deleted_transformer),
        ),
        (
            "GetTransformer on a never-minted transformerId",
            "GetTransformer",
            json!({ "transformerId": unknown_transformer }),
            transformer_not_found(unknown_transformer),
        ),
        (
            "GetTransformer on an id a capability really holds",
            "GetTransformer",
            json!({ "transformerId": capability_id.clone() }),
            transformer_not_found(&capability_id),
        ),
        (
            "DeleteTransformer on a deleted transformerId",
            "DeleteTransformer",
            json!({ "transformerId": deleted_transformer }),
            transformer_not_found(&deleted_transformer),
        ),
        (
            "UpdateTransformer on a never-minted transformerId",
            "UpdateTransformer",
            json!({ "transformerId": unknown_transformer, "status": "active" }),
            transformer_not_found(unknown_transformer),
        ),
        (
            "GetProfile on a never-minted profileId",
            "GetProfile",
            json!({ "profileId": unknown_profile }),
            profile_not_found(unknown_profile),
        ),
        (
            "DeleteProfile on a deleted profileId",
            "DeleteProfile",
            json!({ "profileId": deleted_profile }),
            profile_not_found(&deleted_profile),
        ),
        (
            "UpdateProfile on a never-minted profileId",
            "UpdateProfile",
            json!({ "profileId": unknown_profile, "name": "renamed" }),
            profile_not_found(unknown_profile),
        ),
        (
            "GetCapability on a never-minted capabilityId",
            "GetCapability",
            json!({ "capabilityId": unknown_capability }),
            capability_not_found(unknown_capability),
        ),
        (
            "GetPartnership on a never-minted partnershipId",
            "GetPartnership",
            json!({ "partnershipId": unknown_partnership }),
            partnership_not_found(unknown_partnership),
        ),
        (
            "UpdatePartnership on a never-minted partnershipId",
            "UpdatePartnership",
            json!({ "partnershipId": unknown_partnership, "name": "renamed" }),
            partnership_not_found(unknown_partnership),
        ),
        (
            // A reference another resource carries, not the request's own identifier:
            // the same refusal, on its own message template.
            "CreateCapability naming a transformer nobody holds",
            "CreateCapability",
            json!({
                "name": "capability-for-an-unknown-transformer",
                "type": "edi",
                "configuration": capability_configuration(
                    unknown_transformer,
                    "X12_850",
                    "VERSION_4010"
                )
            }),
            transformer_reference_not_found(unknown_transformer),
        ),
        (
            "CreatePartnership naming a capability nobody holds",
            "CreatePartnership",
            json!({
                "profileId": profile_id.clone(),
                "name": "partner-for-an-unknown-capability",
                "email": "partner@example.com",
                "capabilities": [unknown_capability]
            }),
            capability_not_found(unknown_capability),
        ),
        (
            "GetTransformerJob under a transformer nobody holds",
            "GetTransformerJob",
            json!({
                "transformerId": unknown_transformer,
                "transformerJobId": "0123456789abcdef012345678"
            }),
            transformer_job_not_found("0123456789abcdef012345678", ACCOUNT),
        ),
        (
            "StartTransformerJob on a never-minted transformerId",
            "StartTransformerJob",
            json!({
                "transformerId": unknown_transformer,
                "inputFile": { "bucketName": "inbound-bucket", "key": "in/850.edi" },
                "outputLocation": { "bucketName": "outbound-bucket", "key": "out/" }
            }),
            transformer_not_found(unknown_transformer),
        ),
    ];

    for (case, operation, request, message) in cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, NOT_FOUND_STATUS,
            "case {case:?}: a not-found refusal of this service is {NOT_FOUND_STATUS}, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        // `ResourceNotFoundException`'s member is `message` —
        // `smithy_query(query="shape",shape="ResourceNotFoundException")`.
        assert_eq!(
            text(&body, "message"),
            message,
            "case {case:?}: expected the service's own not-found message, got {:?}",
            text(&body, "message")
        );
    }

    // The live resources are untouched by every refusal above, so the rows refused a
    // lookup rather than a request this service cannot serve at all.
    for (case, operation, member, id) in [
        ("profile", "GetProfile", "profileId", &profile_id),
        ("capability", "GetCapability", "capabilityId", &capability_id),
        (
            "partnership",
            "GetPartnership",
            "partnershipId",
            &partnership_id,
        ),
        (
            "transformer",
            "GetTransformer",
            "transformerId",
            &transformer_id,
        ),
    ] {
        let (status, body) = call(&router, operation, json!({ member: id })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the live {case} must still be readable, got {status} {body}"
        );
    }
}

/// RULE (the other half): only the not-found status is restated. Every other modelled
/// status this service's error shapes carry — the client errors it answers on their own
/// status and the 5xx server fault — reaches the wire exactly as the model annotates it,
/// so a retry classifier still tells a server fault from a client one.
///
/// The rows are every error shape this service models, including the `@httpError 400` that
/// is already the restated status and the 500 a server fault keeps.
#[tokio::test]
async fn every_other_modelled_status_reaches_the_wire_unchanged() {
    for (shape, modelled) in MODELLED_STATUSES {
        let expected = if *shape == "ResourceNotFoundException" {
            NOT_FOUND_STATUS
        } else {
            *modelled
        };
        assert_eq!(
            wire_status(*modelled),
            expected,
            "case {shape:?}: a modelled {modelled} must reach the wire as {expected}, got {}",
            wire_status(*modelled)
        );
    }
}

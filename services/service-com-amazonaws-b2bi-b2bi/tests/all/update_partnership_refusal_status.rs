// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STATUS LINE a modelled client-error refusal of this service
//! reaches the wire with, driven over the wire (awsJson1_0, `x-amz-target:
//! B2BI.<Operation>`) through the same router the emulator boots — so every assertion is
//! about what a generic HTTP client reads BEFORE it ever parses the body.
//!
//! RULE under test: on this RPC envelope the error class travels in the body's `__type`
//! and 404 belongs to the transport (it is what the endpoint answers a request that
//! addressed no operation at all), so EVERY modelled client-error refusal — including the
//! "no such resource" refusal whose shape is annotated `@httpError 404` — answers at the
//! ordinary client-error status 400, whichever operation raises it and whether that
//! operation reads, creates, updates or deletes.
//!
//! The sibling suites assert that status through the crate constant
//! (`scaffolded::NOT_FOUND_STATUS`), which cannot catch a change to the constant itself,
//! and their operation tables leave the UPDATE path out. This suite states the status as
//! the literal the recorded responses carry and spans the mutating operations, so neither
//! gap can hide a refusal that slips back onto the transport's 404.
//!
//! The rows differ along what the rule generalises over: the operation and its kind, the
//! resource kind the identifier names, how the identifier fails to resolve (never minted,
//! deleted, holding an id of another resource kind, at either end of the modelled id
//! length) and which other members the request carries.
//!
//! The wire harness (`b2bi_router`, `call`, `error_type`, `error_code`, `text`) and the
//! `given_*` fixtures are reused from the sibling suites rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::state::{
    partnership_not_found, profile_not_found, profile_reference_not_found, transformer_not_found,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, error_code, error_type, given_capability, given_partnership, given_profile,
    given_transformer, text,
};

/// The status every modelled client-error refusal of this service carries, as the recorded
/// responses state it. Written as the literal rather than read from the crate so that a
/// change to the crate's own mapping is caught here instead of agreeing with itself.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The status this envelope reserves for the transport, and the `@httpError` the not-found
/// shape is annotated with (`smithy_query(query="shape",shape="ResourceNotFoundException")`
/// reports 404) — the one status a modelled refusal of this service must never answer with,
/// because a client would read it as a missing endpoint rather than a refused request.
const TRANSPORT_NOT_FOUND_STATUS: u16 = 404;

/// The qualified shape id this service's recorded error bodies name a missing resource by
/// (`scaffolded::wire_shape_id`).
const QUALIFIED_NOT_FOUND: &str = "com.amazonaws.visby#ResourceNotFoundException";

/// Well-formed identifiers no store holds: each satisfies the modelled `len 1..64` /
/// `^[a-zA-Z0-9_-]+$` every b2bi identifier carries, so what refuses them is the lookup and
/// not the boundary validator.
const UNKNOWN_PARTNERSHIP: &str = "ps-0123456789abcdef0";
const UNKNOWN_PROFILE: &str = "p-0123456789abcdef0";
const UNKNOWN_TRANSFORMER: &str = "tr-0123456789abcdef0";

/// Call `operation` with `request`, assert the refusal is the modelled not-found class at
/// the one client-error status, and hand back the sentence it carried.
async fn refused_as_not_found(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case {case:?}: a modelled client-error refusal of {operation} answers \
         {CLIENT_ERROR_STATUS}, got {status} {body}"
    );
    assert_ne!(
        status, TRANSPORT_NOT_FOUND_STATUS,
        "case {case:?}: {operation} must not answer a modelled refusal with the transport's \
         {TRANSPORT_NOT_FOUND_STATUS}, got {body}"
    );
    assert_eq!(
        error_type(&body),
        QUALIFIED_NOT_FOUND,
        "case {case:?}: expected __type {QUALIFIED_NOT_FOUND:?}, got {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: the shape name in __type must be the error class, got {body}"
    );
    // `ResourceNotFoundException` spells its member `message`
    // (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so a body carrying
    // the `Message` of the other class would leave this one's reader with nothing.
    assert!(
        body.get("Message").is_none(),
        "case {case:?}: a not-found body must not carry Message, got {body}"
    );
    text(&body, "message")
}

/// Read a partnership back through `GetPartnership`.
async fn get_partnership(router: &MultiServiceRouter, partnership_id: &str) -> Value {
    let (status, fetched) = call(
        router,
        "GetPartnership",
        json!({ "partnershipId": partnership_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetPartnership({partnership_id}) must succeed, got {status} {fetched}"
    );
    fetched
}

/// RULE: an update for a `partnershipId` this account's store does not hold is the modelled
/// not-found refusal at the one client-error status — never the transport's 404 — naming
/// that identifier, whatever else the update asks to write.
///
/// The rows span how the identifier fails to resolve (never minted, a partnership that was
/// created and then deleted, identifiers another b2bi resource kind really does hold, and
/// identifiers at both ends of the modelled `PartnershipId` length) crossed with the
/// members the update names beside it (a name, a capability list, an empty list, capability
/// options, and nothing at all).
#[tokio::test]
async fn an_update_for_an_unheld_partnership_answers_the_client_error_status() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "refusal-status").await;
    let capability_id = given_capability(&router, "refusal-status-capability").await;
    let live_partnership =
        given_partnership(&router, &profile_id, &capability_id, "Partner One").await;
    let transformer_id = given_transformer(&router, "refusal-status-transformer").await;

    // A partnership that really existed and was then deleted: from the delete on its id
    // resolves to nothing, so an update naming it is refused exactly like a never-minted one.
    let deleted_partnership =
        given_partnership(&router, &profile_id, &capability_id, "Partner Two").await;
    let (status, body) = call(
        &router,
        "DeletePartnership",
        json!({ "partnershipId": deleted_partnership.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the partnership under test must be deletable, got {status} {body}"
    );

    // `CapabilityOptions` members —
    // `smithy_query(query="shape",shape="CapabilityOptions")`.
    let options = json!({
        "outboundEdi": { "x12": { "common": { "validateEdi": true } } }
    });

    // (case, the addressed partnershipId, the members the update names beside it)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a never-minted partnership id, renaming",
            UNKNOWN_PARTNERSHIP.to_string(),
            json!({ "name": "renamed" }),
        ),
        (
            "a never-minted partnership id, naming no other member",
            "ps-fedcba98765432100".to_string(),
            json!({}),
        ),
        (
            "a partnership that was deleted",
            deleted_partnership.clone(),
            json!({ "name": "renamed after the delete" }),
        ),
        (
            "a partnership that was deleted, rebinding a capability that exists",
            deleted_partnership.clone(),
            json!({ "capabilities": [capability_id.clone()] }),
        ),
        (
            "a profile id that really exists",
            profile_id.clone(),
            json!({ "capabilities": [capability_id.clone()] }),
        ),
        (
            "a capability id that really exists",
            capability_id.clone(),
            json!({ "capabilityOptions": options.clone() }),
        ),
        (
            "a transformer id that really exists",
            transformer_id.clone(),
            json!({ "capabilities": [] }),
        ),
        (
            "a one-character id, the modelled minimum length",
            "p".to_string(),
            json!({ "name": "renamed" }),
        ),
        (
            "an id of the modelled maximum length",
            "p".repeat(64),
            json!({ "capabilityOptions": options.clone() }),
        ),
    ];

    let before = get_partnership(&router, &live_partnership).await;

    for (case, partnership_id, members) in &cases {
        let mut request = members.clone();
        request["partnershipId"] = json!(partnership_id);
        let message = refused_as_not_found(&router, case, "UpdatePartnership", request).await;
        assert_eq!(
            message,
            partnership_not_found(partnership_id),
            "case {case:?}: expected the refusal to name {partnership_id:?}, got {message:?}"
        );

        // The refusal wrote nothing: it minted no partnership under the id it refused, and
        // the partnership this account does hold is untouched.
        let (status, fetched) = call(
            &router,
            "GetPartnership",
            json!({ "partnershipId": partnership_id }),
        )
        .await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case:?}: the refused update must have minted nothing, got {status} {fetched}"
        );
        let live = get_partnership(&router, &live_partnership).await;
        assert_eq!(
            live, before,
            "case {case:?}: the refused update must leave the stored partnership untouched, \
             got {live} instead of {before}"
        );
    }

    // And the status rule is about REFUSALS only: the same operation on the partnership this
    // account holds is served, and the independent reader reports what it wrote.
    let (status, updated) = call(
        &router,
        "UpdatePartnership",
        json!({ "partnershipId": live_partnership.clone(), "name": "Partner One Renamed" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update for a partnership this account holds must be served, got {status} {updated}"
    );
    let fetched = get_partnership(&router, &live_partnership).await;
    assert_eq!(
        text(&fetched, "name"),
        "Partner One Renamed",
        "the update must be readable back through GetPartnership, got {fetched}"
    );
}

/// RULE (the same rule, across the operation axis): the status a modelled not-found refusal
/// carries does not depend on which operation raised it or on what that operation does to
/// the resource, so the readers, the deletes, the creates that resolve a reference and the
/// UPDATES all answer the one client-error status with the qualified class.
///
/// The rows are the operations of this service that resolve an identifier and can refuse,
/// one per kind and resource: the update path this suite is about sits beside its read,
/// delete and create siblings and the transformer and profile operations, so no single
/// operation can drift back onto the transport's 404 on its own. `UpdateCapability` is
/// absent because nothing implements it yet — it answers the scaffold's 501, which is not a
/// modelled refusal (see `client_error_bodies.rs`).
#[tokio::test]
async fn every_operation_refuses_an_unheld_resource_at_the_same_status() {
    let router = b2bi_router();

    // (case, the operation, the request, the sentence the refusal must carry)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "UpdatePartnership",
            "UpdatePartnership",
            json!({ "partnershipId": UNKNOWN_PARTNERSHIP, "name": "renamed" }),
            partnership_not_found(UNKNOWN_PARTNERSHIP),
        ),
        (
            "GetPartnership",
            "GetPartnership",
            json!({ "partnershipId": UNKNOWN_PARTNERSHIP }),
            partnership_not_found(UNKNOWN_PARTNERSHIP),
        ),
        (
            "DeletePartnership",
            "DeletePartnership",
            json!({ "partnershipId": UNKNOWN_PARTNERSHIP }),
            partnership_not_found(UNKNOWN_PARTNERSHIP),
        ),
        (
            "CreatePartnership under a profile nobody holds",
            "CreatePartnership",
            json!({
                "profileId": UNKNOWN_PROFILE,
                "name": "partner-under-an-unknown-profile",
                "email": "partner@example.com",
                "capabilities": []
            }),
            profile_reference_not_found(UNKNOWN_PROFILE),
        ),
        (
            "UpdateProfile",
            "UpdateProfile",
            json!({ "profileId": UNKNOWN_PROFILE, "name": "renamed" }),
            profile_reference_not_found(UNKNOWN_PROFILE),
        ),
        (
            "DeleteProfile",
            "DeleteProfile",
            json!({ "profileId": UNKNOWN_PROFILE }),
            profile_not_found(UNKNOWN_PROFILE),
        ),
        (
            "UpdateTransformer",
            "UpdateTransformer",
            json!({ "transformerId": UNKNOWN_TRANSFORMER, "status": "active" }),
            transformer_not_found(UNKNOWN_TRANSFORMER),
        ),
        (
            "DeleteTransformer",
            "DeleteTransformer",
            json!({ "transformerId": UNKNOWN_TRANSFORMER }),
            transformer_not_found(UNKNOWN_TRANSFORMER),
        ),
        (
            "StartTransformerJob",
            "StartTransformerJob",
            json!({
                "transformerId": UNKNOWN_TRANSFORMER,
                "inputFile": { "bucketName": "inbound-bucket", "key": "in/850.edi" },
                "outputLocation": { "bucketName": "outbound-bucket", "key": "out/" }
            }),
            transformer_not_found(UNKNOWN_TRANSFORMER),
        ),
    ];

    for (case, operation, request, message) in &cases {
        let reported = refused_as_not_found(&router, case, operation, request.clone()).await;
        assert_eq!(
            reported, *message,
            "case {case:?}: expected the service's own not-found sentence {message:?}, got \
             {reported:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for GuardDuty's publishing destinations.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `CreatePublishingDestination` mints a destination under the detector named in
//!    the URI and persists the type, the destination properties and the tags it
//!    accepted; `DescribePublishingDestination` reports exactly those back, and a
//!    destination that has never failed to publish reports a zero
//!    `publishingFailureStartTimestamp`.
//! 2. A new destination is returned in `PENDING_VERIFICATION` and the CLOCK — nothing
//!    else — moves it to `PUBLISHING`, so a caller reading straight back still sees
//!    the pending status and a reader after the span sees the settled one.
//! 3. `ClientToken` is an idempotency token: a replayed create returns the
//!    destination the first call minted, and two creates that do not share a token
//!    are two destinations.
//! 4. A request GuardDuty cannot accept is refused with HTTP 400 under the
//!    `InvalidInputException` class, naming the member at fault, and leaves no
//!    destination behind.
//! 5. A detector holds at most `state::PUBLISHING_DESTINATIONS_PER_DETECTOR`
//!    destinations — the bound that keeps the store finite — and a create past it
//!    is refused while a REPLAY at it still answers from the store.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state;

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The error class every GuardDuty client rejection is named by on the wire (see
/// `client_rejections.rs`).
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in.
const ENVELOPE: &str = "BadRequestException";

/// The only member of the `DestinationType` enum
/// (`smithy_query(shape="DestinationType")`).
const S3: &str = "S3";

/// A well-formed `DestinationId` (32 hex characters, the form every id this service
/// mints has) that no create ever minted.
const ABSENT_DESTINATION_ID: &str = "0123456789abcdef0123456789abcdef";

/// A KMS key ARN, the form `DestinationProperties.KmsKeyArn` carries ("The ARN of
/// the KMS key to use for encryption" — `smithy_query(shape="DestinationProperties")`).
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

async fn create_destination(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination"),
        Some(body),
    )
    .await
}

async fn describe_destination(
    router: &Router,
    detector_id: &str,
    destination_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        None,
    )
    .await
}

/// Create a destination and return its id, failing the test if the create did not
/// succeed.
async fn created_destination_id(router: &Router, detector_id: &str, body: Value) -> String {
    let (status, response) = create_destination(router, detector_id, body.clone()).await;
    assert_eq!(
        status, 200,
        "create-publishing-destination {body}: expected 200, got {status} {response:?}"
    );
    response["destinationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("create-publishing-destination {body}: expected a destinationId, got {response:?}")
        })
        .to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: a destination reports back exactly what the create stored.
// ---------------------------------------------------------------------------

/// One create request and what a read of the destination it minted must report.
struct RoundTrip {
    case: &'static str,
    /// The `destinationProperties` the create carries.
    properties: Value,
    /// The `tags` the create carries, or `None` for a create that names none.
    tags: Option<Value>,
}

/// The destination ARN rows are the forms the member's own documentation names: an
/// S3 bucket, and a bucket FOLDER — "To specify an S3 bucket folder use the
/// following format: `arn:aws:s3:::DOC-EXAMPLE-BUCKET/myFolder/`"
/// (`smithy_query(query="doc",shape="DestinationProperties",member="DestinationArn")`).
/// The key rows span the two KMS resource forms and more than one Region, and
/// `expectedBucketOwner` is "The 12-digit Amazon Web Services account ID of the
/// account that owns the Amazon S3 bucket" which "appears in the response only if it
/// was provided".
fn round_trips() -> Vec<RoundTrip> {
    vec![
        RoundTrip {
            case: "bucket root, no owner, no tags",
            properties: json!({
                "destinationArn": "arn:aws:s3:::guardduty-findings",
                "kmsKeyArn": KEY_ARN,
            }),
            tags: None,
        },
        RoundTrip {
            case: "bucket folder, as the member's documentation writes it",
            properties: json!({
                "destinationArn": "arn:aws:s3:::DOC-EXAMPLE-BUCKET/myFolder/",
                "kmsKeyArn": KEY_ARN,
            }),
            tags: Some(json!({"team": "secops"})),
        },
        RoundTrip {
            case: "nested folder, key alias, expected bucket owner",
            properties: json!({
                "destinationArn": "arn:aws:s3:::example-bucket/findings/2026/",
                "kmsKeyArn": "arn:aws:kms:eu-west-1:444455556666:alias/guardduty",
                "expectedBucketOwner": "111122223333",
            }),
            tags: Some(json!({"env": "prod", "owner": "team-a", "tier": "1"})),
        },
        RoundTrip {
            case: "another partition",
            properties: json!({
                "destinationArn": "arn:aws-us-gov:s3:::gov-findings-bucket/exports/",
                "kmsKeyArn": "arn:aws-us-gov:kms:us-gov-west-1:111122223333:key/abcd",
            }),
            tags: Some(json!({"classification": "controlled"})),
        },
    ]
}

#[tokio::test]
async fn a_destination_reports_back_the_properties_it_was_created_with() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for trip in round_trips() {
        let case = trip.case;
        let mut body = json!({
            "destinationType": S3,
            "destinationProperties": trip.properties.clone(),
        });
        if let Some(tags) = trip.tags.as_ref() {
            body["tags"] = tags.clone();
        }

        let destination_id = created_destination_id(&router, &detector_id, body).await;
        assert!(
            minted.insert(destination_id.clone()),
            "case {case:?}: every create mints its own destinationId, but got {destination_id:?} twice"
        );

        let (status, read) = describe_destination(&router, &detector_id, &destination_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} {read:?}"
        );
        assert_eq!(
            read["destinationId"].as_str(),
            Some(destination_id.as_str()),
            "case {case:?}: describe must report the id the create minted, got {read:?}"
        );
        assert_eq!(
            read["destinationType"].as_str(),
            Some(S3),
            "case {case:?}: expected destinationType {S3}, got {read:?}"
        );
        assert_eq!(
            read["destinationProperties"], trip.properties,
            "case {case:?}: expected the stored destinationProperties {:?}, got {:?}",
            trip.properties, read["destinationProperties"]
        );
        match trip.tags.as_ref() {
            Some(tags) => assert_eq!(
                read["tags"], *tags,
                "case {case:?}: expected the stored tags {tags:?}, got {:?}",
                read["tags"]
            ),
            // `TagMap` carries `len 1..200`, so an untagged destination reports no
            // `tags` member rather than an empty map.
            None => assert!(
                read.get("tags").is_none_or(Value::is_null),
                "case {case:?}: an untagged destination must report no tags, got {:?}",
                read["tags"]
            ),
        }
        assert_eq!(
            read["publishingFailureStartTimestamp"].as_i64(),
            Some(0),
            "case {case:?}: a destination that never failed to publish reports 0, got {:?}",
            read["publishingFailureStartTimestamp"]
        );
    }

    assert_eq!(
        minted.len(),
        round_trips().len(),
        "every create must mint a distinct destination: got {minted:?}"
    );
}

/// A destination belongs to the detector it was created under, so a read through
/// another detector — or through another Region's endpoint, since a detector is
/// regional — does not resolve it.
#[tokio::test]
async fn a_destination_belongs_to_the_detector_and_region_it_was_created_under() {
    let router = new_router();
    let home = "us-west-2";
    let (status, created) = call_in(
        &router,
        home,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-detector in {home}: expected 200, got {status} {created:?}"
    );
    let detector_id = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector in {home}: expected a detectorId, got {created:?}"))
        .to_string();

    let path = format!("/detector/{detector_id}/publishingDestination");
    let (status, response) = call_in(
        &router,
        home,
        "POST",
        &path,
        Some(json!({
            "destinationType": S3,
            "destinationProperties": {
                "destinationArn": "arn:aws:s3:::regional-findings",
                "kmsKeyArn": KEY_ARN,
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create in {home}\": expected 200, got {status} {response:?}"
    );
    let destination_id = response["destinationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"create in {home}\": expected a destinationId, got {response:?}"))
        .to_string();

    // The home Region's endpoint serves it.
    let (status, read) = call_in(
        &router,
        home,
        "GET",
        &format!("{path}/{destination_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"read in {home}\": expected 200, got {status} {read:?}"
    );

    // Another Region's endpoint does not: it does not hold the detector at all.
    for region in ["us-east-1", "eu-central-1"] {
        let (status, body) = call_in(
            &router,
            region,
            "GET",
            &format!("{path}/{destination_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case \"read in {region}\": expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            CLASS,
            "case \"read in {region}\": expected the {CLASS} class, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the clock, and only the clock, verifies a new destination.
// ---------------------------------------------------------------------------

/// `PublishingStatus` is `PENDING_VERIFICATION | PUBLISHING |
/// UNABLE_TO_PUBLISH_FIX_DESTINATION_PROPERTY | STOPPED`
/// (`smithy_query(shape="PublishingStatus")`). A create returns the destination in
/// the first of those, and the verification the clock owes moves it to the second;
/// reading it repeatedly before the span is up never moves it, and nothing a caller
/// does after the span can move it back.
#[tokio::test]
async fn a_new_destination_is_verified_by_the_clock_and_not_by_being_read() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let destination_id = created_destination_id(
        &router,
        &detector_id,
        json!({
            "destinationType": S3,
            "destinationProperties": {
                "destinationArn": "arn:aws:s3:::pending-verification-bucket",
                "kmsKeyArn": KEY_ARN,
            },
        }),
    )
    .await;

    // The span is never zero, so a caller reading straight back — and again, and
    // again — still observes the transient status.
    for read in 1..=3 {
        let (status, body) = describe_destination(&router, &detector_id, &destination_id).await;
        assert_eq!(
            status, 200,
            "case \"read {read} before the verification is due\": expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["status"].as_str(),
            Some("PENDING_VERIFICATION"),
            "case \"read {read} before the verification is due\": expected PENDING_VERIFICATION, got {:?}",
            body["status"]
        );
    }

    // Past the span the destination reports the settled status, and keeps it.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    for read in 1..=2 {
        let (status, body) = describe_destination(&router, &detector_id, &destination_id).await;
        assert_eq!(
            status, 200,
            "case \"read {read} after the verification is due\": expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["status"].as_str(),
            Some("PUBLISHING"),
            "case \"read {read} after the verification is due\": expected PUBLISHING, got {:?}",
            body["status"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: ClientToken is an idempotency token.
// ---------------------------------------------------------------------------

/// `ClientToken` is `@idempotencyToken` (`smithy_query(operation="CreatePublishingDestination")`),
/// so replaying a create with the same token returns the destination the first call
/// minted. The rows span the ways two creates can relate: same token, different
/// tokens, and no token at all.
#[tokio::test]
async fn a_replayed_client_token_returns_the_destination_the_first_create_minted() {
    // (case, first token, second token, do the two creates name one destination?)
    let cases: [(&str, Option<&str>, Option<&str>, bool); 4] = [
        ("same token", Some("token-alpha"), Some("token-alpha"), true),
        (
            "same long token",
            Some("0123456789012345678901234567890123456789012345678901234567890123"),
            Some("0123456789012345678901234567890123456789012345678901234567890123"),
            true,
        ),
        (
            "different tokens",
            Some("token-beta"),
            Some("token-gamma"),
            false,
        ),
        ("no token", None, None, false),
    ];

    for (case, first_token, second_token, same) in cases {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let body = |token: Option<&str>| {
            let mut body = json!({
                "destinationType": S3,
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::idempotency-bucket",
                    "kmsKeyArn": KEY_ARN,
                },
            });
            if let Some(token) = token {
                body["clientToken"] = json!(token);
            }
            body
        };

        let first = created_destination_id(&router, &detector_id, body(first_token)).await;
        let second = created_destination_id(&router, &detector_id, body(second_token)).await;
        assert_eq!(
            first == second,
            same,
            "case {case:?}: expected the two creates to name {} destination(s), got {first:?} and {second:?}",
            if same { "one" } else { "two" }
        );

        // Whatever the ids, each one reads back as its own destination.
        for id in [&first, &second] {
            let (status, read) = describe_destination(&router, &detector_id, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: describe {id:?} expected 200, got {status} {read:?}"
            );
        }
    }
}

/// A refused create persists nothing, so the token it carried is still free: a later
/// create replaying that token mints a destination instead of returning one the
/// refused call left behind.
#[tokio::test]
async fn a_refused_create_leaves_no_destination_behind() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let token = "token-refused-then-accepted";

    let (status, body) = create_destination(
        &router,
        &detector_id,
        json!({
            "destinationType": S3,
            "destinationProperties": {"kmsKeyArn": KEY_ARN},
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"create with no destinationArn\": expected 400, got {status} {body:?}"
    );

    let destination_id = created_destination_id(
        &router,
        &detector_id,
        json!({
            "destinationType": S3,
            "destinationProperties": {
                "destinationArn": "arn:aws:s3:::accepted-bucket",
                "kmsKeyArn": KEY_ARN,
            },
            "clientToken": token,
        }),
    )
    .await;
    let (status, read) = describe_destination(&router, &detector_id, &destination_id).await;
    assert_eq!(
        status, 200,
        "case \"replay the refused token\": expected 200, got {status} {read:?}"
    );
    assert_eq!(
        read["destinationProperties"]["destinationArn"].as_str(),
        Some("arn:aws:s3:::accepted-bucket"),
        "case \"replay the refused token\": expected the accepted create's ARN, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a request the service cannot accept is a classified 400.
// ---------------------------------------------------------------------------

/// Both ARNs are required — `DestinationProperties` carries "the ARNs for the
/// destination and the KMS key used for encryption"
/// (`smithy_query(query="doc",shape="CreatePublishingDestinationRequest",member="DestinationProperties")`)
/// — each must be a well-formed ARN of the service its own member names,
/// `destinationType` must be a `DestinationType` member, and
/// `expectedBucketOwner` must be a 12-DIGIT account id. The rows span every one of
/// those, with a value that is wrong in a different way in each.
///
/// Two rows are refused by the MODEL rather than by the handler: `destinationType`
/// targets an enum (`smithy_query(shape="DestinationType")`) and
/// `expectedBucketOwner` carries `len 12..12`
/// (`smithy_query(shape="DestinationProperties")`), so the boundary validator
/// answers a value outside either under the modelled `BadRequestException` (see
/// `modeled_constraint_refusals.rs`) before any handler runs. Every other row is a
/// rule the model cannot express, so the handler refuses it under GuardDuty's own
/// `InvalidInputException` class.
#[tokio::test]
async fn a_create_the_service_cannot_accept_is_refused() {
    // (case, request body, the member the message must name, the error class)
    let cases: [(&str, Value, &str, &str); 12] = [
        (
            "no destinationArn",
            json!({"destinationType": S3, "destinationProperties": {"kmsKeyArn": KEY_ARN}}),
            "destinationArn",
            CLASS,
        ),
        (
            "empty destinationArn",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "", "kmsKeyArn": KEY_ARN}}),
            "destinationArn",
            CLASS,
        ),
        (
            "destinationArn is not an ARN at all",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "guardduty-findings", "kmsKeyArn": KEY_ARN}}),
            "destinationArn",
            CLASS,
        ),
        (
            "destinationArn names another service",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "arn:aws:sqs:us-east-1:111122223333:findings", "kmsKeyArn": KEY_ARN}}),
            "destinationArn",
            CLASS,
        ),
        (
            "destinationArn has no resource",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "arn:aws:s3:::", "kmsKeyArn": KEY_ARN}}),
            "destinationArn",
            CLASS,
        ),
        (
            "no kmsKeyArn",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "arn:aws:s3:::bucket"}}),
            "kmsKeyArn",
            CLASS,
        ),
        (
            "kmsKeyArn names another service",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "arn:aws:s3:::bucket", "kmsKeyArn": "arn:aws:s3:::bucket"}}),
            "kmsKeyArn",
            CLASS,
        ),
        (
            "kmsKeyArn is not an ARN at all",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "arn:aws:s3:::bucket", "kmsKeyArn": "1234abcd-12ab-34cd-56ef-1234567890ab"}}),
            "kmsKeyArn",
            CLASS,
        ),
        (
            "destinationType outside the enum",
            json!({"destinationType": "SQS", "destinationProperties": {"destinationArn": "arn:aws:sqs:us-east-1:111122223333:findings", "kmsKeyArn": KEY_ARN}}),
            "destinationType",
            ENVELOPE,
        ),
        (
            "expectedBucketOwner is 12 characters but not 12 digits",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "arn:aws:s3:::bucket", "kmsKeyArn": KEY_ARN, "expectedBucketOwner": "owner1234567"}}),
            "expectedBucketOwner",
            CLASS,
        ),
        (
            "expectedBucketOwner is one digit short of the modelled length",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "arn:aws:s3:::bucket", "kmsKeyArn": KEY_ARN, "expectedBucketOwner": "11112222333"}}),
            "expectedBucketOwner",
            ENVELOPE,
        ),
        (
            "expectedBucketOwner is one digit over the modelled length",
            json!({"destinationType": S3, "destinationProperties": {"destinationArn": "arn:aws:s3:::bucket", "kmsKeyArn": KEY_ARN, "expectedBucketOwner": "1111222233334"}}),
            "expectedBucketOwner",
            ENVELOPE,
        ),
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    for (case, body, member, class) in cases {
        let (status, response) = create_destination(&router, &detector_id, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            class,
            "case {case:?}: expected the {class} class, got {response:?}"
        );
        // Either way the error SHAPE on the wire is the only client error this
        // service's model declares, never the framework's `ValidationException`.
        assert_eq!(
            error_shape(&response),
            ENVELOPE,
            "case {case:?}: expected the {ENVELOPE} envelope shape, got {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name {member}, got {message:?}"
        );
    }
}

/// A detector holds publishing destinations up to
/// `state::PUBLISHING_DESTINATIONS_PER_DETECTOR` and refuses the create past it —
/// the bound itself, and one past it, are both asserted. The maximum is what keeps
/// the destination store (and the `ClientToken` index each record carries) finite
/// whatever a caller sends.
#[tokio::test]
async fn the_per_detector_maximum_is_enforced_at_its_bound() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let limit = state::PUBLISHING_DESTINATIONS_PER_DETECTOR;

    let mut ids = BTreeSet::new();
    // The first destination carries a `ClientToken`, so the replay below can be
    // asserted at the maximum.
    let token = "token-at-the-limit";
    for index in 0..limit {
        let case = format!("destination {index} of {limit}");
        let mut body = json!({
            "destinationType": S3,
            "destinationProperties": {
                "destinationArn": format!("arn:aws:s3:::bounded-{index}"),
                "kmsKeyArn": KEY_ARN,
            },
        });
        if index == 0 {
            body["clientToken"] = json!(token);
        }
        let id = created_destination_id(&router, &detector_id, body).await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: every create mints its own destinationId, got {id:?} twice"
        );
    }

    let case = format!("one past {limit}");
    let (status, body) = create_destination(
        &router,
        &detector_id,
        json!({
            "destinationType": S3,
            "destinationProperties": {
                "destinationArn": "arn:aws:s3:::over-the-limit",
                "kmsKeyArn": KEY_ARN,
            },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 past the maximum, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        CLASS,
        "case {case:?}: expected the {CLASS} class, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(state::publishing_destination_limit_message().as_str()),
        "case {case:?}: expected the limit message, got {body:?}"
    );

    // A REPLAY at the maximum still answers from the store: it mints nothing, so
    // it needs no slot.
    let (status, body) = create_destination(
        &router,
        &detector_id,
        json!({
            "destinationType": S3,
            "destinationProperties": {
                "destinationArn": "arn:aws:s3:::bounded-0",
                "kmsKeyArn": KEY_ARN,
            },
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"replay a token at the maximum\": expected 200, got {status} {body:?}"
    );
    assert!(
        body["destinationId"]
            .as_str()
            .is_some_and(|id| ids.contains(id)),
        "case \"replay a token at the maximum\": expected one of the destinations already minted, got {body:?}"
    );
}

/// An id neither operation's store resolves is the same classified 400: a value that
/// is not an id of this service at all is a parameter failure, a well-formed one the
/// account does not own is an ownership failure, and neither message quotes the
/// caller's value back at them.
#[tokio::test]
async fn an_id_the_store_does_not_hold_is_refused_by_both_operations() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let destination_id = created_destination_id(
        &router,
        &detector_id,
        json!({
            "destinationType": S3,
            "destinationProperties": {
                "destinationArn": "arn:aws:s3:::refusal-bucket",
                "kmsKeyArn": KEY_ARN,
            },
        }),
    )
    .await;

    // (case, method, path, body, the member the message must name)
    let cases: [(&str, &str, String, Option<Value>, &str); 6] = [
        (
            "create under an absent detector",
            "POST",
            format!("/detector/{ABSENT_DETECTOR_ID}/publishingDestination"),
            Some(json!({
                "destinationType": S3,
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::refusal-bucket",
                    "kmsKeyArn": KEY_ARN,
                },
            })),
            "detectorId",
        ),
        (
            "create under a detector id that is not one",
            "POST",
            "/detector/not-a-detector-id/publishingDestination".to_string(),
            Some(json!({
                "destinationType": S3,
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::refusal-bucket",
                    "kmsKeyArn": KEY_ARN,
                },
            })),
            "detectorId",
        ),
        (
            "describe under an absent detector",
            "GET",
            format!("/detector/{ABSENT_DETECTOR_ID}/publishingDestination/{destination_id}"),
            None,
            "detectorId",
        ),
        (
            "describe an absent destination",
            "GET",
            format!("/detector/{detector_id}/publishingDestination/{ABSENT_DESTINATION_ID}"),
            None,
            "destinationId",
        ),
        (
            "describe a destination id that is not one",
            "GET",
            format!("/detector/{detector_id}/publishingDestination/not-a-destination-id"),
            None,
            "destinationId",
        ),
        (
            "describe a destination id the caller upper-cased",
            "GET",
            format!(
                "/detector/{detector_id}/publishingDestination/{}",
                destination_id.to_uppercase()
            ),
            None,
            "destinationId",
        ),
    ];

    for (case, method, path, body, member) in cases {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            CLASS,
            "case {case:?}: expected the {CLASS} class, got {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name {member}, got {message:?}"
        );
        assert!(
            !message.contains(ABSENT_DETECTOR_ID) && !message.contains(ABSENT_DESTINATION_ID),
            "case {case:?}: the refusal must not quote the caller's id back, got {message:?}"
        );
    }
}

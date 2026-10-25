// SPDX-License-Identifier: CC0-1.0

//! `DeletePublishingDestination`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together. The create, the describe, the update and the list are what the
//! `publishing_destination_lifecycle`, `publishing_destination_update` and
//! `publishing_destination_listing` suites own; this one owns the DELETE.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A delete DROPS the destination its `DestinationId` names off the detector its
//!    `DetectorId` names: the call answers HTTP 200 with a body carrying no members
//!    (`DeletePublishingDestinationResponse` declares none,
//!    `smithy_query(operation="DeletePublishingDestination")`), afterwards every
//!    operation that addresses that destination by id refuses it — including a second
//!    delete — `ListPublishingDestinations` no longer names it, and the detector's own
//!    `updatedAt` moves because it holds one fewer sub-resource. The detector's other
//!    destinations are untouched.
//! 2. The RECORD goes, so everything it held goes with it: the slot it took against
//!    `state::PUBLISHING_DESTINATIONS_PER_DETECTOR` is free for a fresh create, the
//!    `ClientToken` that minted it stops replaying (the index lives on the record), and
//!    its tag ARN resolves nothing.
//! 3. A destination belongs to ONE detector, which is regional: a delete reaches only
//!    the destination held by the detector the request names under the Region the
//!    request is signed for. A delete addressed through another Region's endpoint, or
//!    under another detector's id, is refused and writes nothing.
//! 4. A refusal is the classified HTTP 400 every id rejection on this service carries —
//!    the `BadRequestException` envelope, the `InvalidInputException` class, a message
//!    naming the member at fault and never the caller's value — and persists nothing.
//!
//! The rows span what the rule generalises over: the stored destination's PROPERTIES
//! (bucket, folder depth, KMS resource form, partition, `expectedBucketOwner`, tags),
//! its STATUS at the instant of the delete (inside the clock-owed verification span and
//! past it), and the NUMBER of destinations the detector holds. Every ARN form is one
//! `smithy_query(query="doc",shape="DestinationProperties",member="DestinationArn")`
//! names.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DESTINATION_ID_MEMBER, DETECTOR_ID_MEMBER, PENDING_VERIFICATION, PUBLISHING,
    PUBLISHING_DESTINATIONS_PER_DETECTOR, VERIFICATION_DELAY_MILLIS,
};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator (see `client_rejections.rs`).
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in — the only client error
/// `DeletePublishingDestination` declares
/// (`smithy_query(operation="DeletePublishingDestination")`).
const ENVELOPE: &str = "BadRequestException";

/// The only member of the `DestinationType` enum
/// (`smithy_query(shape="DestinationType")`).
const S3: &str = "S3";

/// The Region the shared harness signs for, and so the one `enabled_detector`'s
/// detector is reachable through.
const HOME_REGION: &str = "us-east-1";

/// Another Region, for the rows that assert a destination is regional.
const AWAY_REGION: &str = "eu-west-1";

/// A well-formed `DestinationId` (32 hexadecimal characters, the form every id this
/// service mints has) that no create ever minted.
const ABSENT_DESTINATION_ID: &str = "0123456789abcdef0123456789abcdef";

/// This service's segment in an ARN it owns (`smithy_query(query="service_info")`
/// reports `guardduty` as both the `arnNamespace` and the `endpointPrefix`).
const ARN_SERVICE: &str = "guardduty";

/// Longer than `state::VERIFICATION_DELAY_MILLIS`, so a call after this wait is past
/// the span the clock owes the `PENDING_VERIFICATION` -> `PUBLISHING` move at.
const PAST_THE_VERIFICATION_SPAN: std::time::Duration = std::time::Duration::from_millis(900);
const _: () = assert!(PAST_THE_VERIFICATION_SPAN.as_millis() as i64 > VERIFICATION_DELAY_MILLIS);

/// A create body differing from its neighbours in bucket, folder depth, KMS resource
/// form, partition, whether it names an `expectedBucketOwner` and whether it carries
/// tags, so the rows span those classes rather than repeating one request.
fn create_body(index: usize) -> Value {
    let partition = if index.is_multiple_of(5) {
        "aws-us-gov"
    } else {
        "aws"
    };
    let destination_arn = match index % 3 {
        0 => format!("arn:{partition}:s3:::findings-{index:03}"),
        1 => format!("arn:{partition}:s3:::findings-{index:03}/exports/"),
        _ => format!("arn:{partition}:s3:::findings-{index:03}/exports/2026/{index:03}/"),
    };
    let kms_key_arn = if index.is_multiple_of(2) {
        format!("arn:{partition}:kms:us-east-1:111122223333:key/0000abcd-12ab-34cd-56ef-{index:012}")
    } else {
        format!("arn:{partition}:kms:eu-west-1:444455556666:alias/guardduty-{index:03}")
    };
    let mut properties = json!({
        "destinationArn": destination_arn,
        "kmsKeyArn": kms_key_arn,
    });
    if index.is_multiple_of(3) {
        properties["expectedBucketOwner"] = json!("111122223333");
    }
    let mut body = json!({
        "destinationType": S3,
        "destinationProperties": properties,
    });
    if index.is_multiple_of(4) {
        body["tags"] = json!({"index": format!("{index:03}")});
    }
    body
}

async fn create_in(router: &Router, region: &str, detector_id: &str, body: Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination"),
        Some(body),
    )
    .await
}

/// Create one destination and return its minted id, failing the test if the create
/// did not succeed.
async fn created(
    router: &Router,
    region: &str,
    detector_id: &str,
    case: &str,
    body: Value,
) -> String {
    let (status, got) = create_in(router, region, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to answer 200, got {status} {got:?}"
    );
    got["destinationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a destinationId, got {got:?}"))
        .to_string()
}

async fn describe_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    destination_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        None,
    )
    .await
}

async fn update_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    destination_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        Some(json!({
            "destinationProperties": {"destinationArn": "arn:aws:s3:::moved-bucket"},
        })),
    )
    .await
}

async fn delete_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    destination_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "DELETE",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        None,
    )
    .await
}

async fn delete(router: &Router, detector_id: &str, destination_id: &str) -> (u16, Value) {
    delete_in(router, HOME_REGION, detector_id, destination_id).await
}

/// The ids one `ListPublishingDestinations` page reports, failing the test if the
/// list did not succeed. The whole collection fits in one page: `MaxResults` is
/// `range 1..50` and a detector holds at most
/// `PUBLISHING_DESTINATIONS_PER_DETECTOR` destinations.
async fn listed_ids(router: &Router, region: &str, detector_id: &str, case: &str) -> BTreeSet<String> {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/publishingDestination?maxResults=50"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the list to answer 200, got {status} {body:?}"
    );
    body["destinations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected destinations in {body:?}"))
        .iter()
        .map(|entry| {
            entry["destinationId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a destinationId in {entry:?}"))
                .to_string()
        })
        .collect()
}

/// The detector's own `updatedAt`, as `GetDetector` reports it.
async fn detector_updated_at(router: &Router, region: &str, detector_id: &str, case: &str) -> String {
    let (status, got) = call_in(router, region, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the detector's updatedAt, got {got:?}"))
        .to_string()
}

/// Assert one classified refusal: HTTP 400, the modelled envelope shape, the service's
/// error class, a message naming `member` and a message that does not quote `quoted`
/// back at the caller.
fn assert_refusal(case: &str, status: u16, body: &Value, member: &str, quoted: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case:?}: expected the {ENVELOPE} envelope shape, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(member),
        "case {case:?}: the refusal must name {member}, got {message:?}"
    );
    assert!(
        !message.contains(quoted),
        "case {case:?}: the refusal must not quote the caller's value back, got {message:?}"
    );
}

/// `DeletePublishingDestinationResponse` declares no members, so a successful delete
/// answers with an empty body — never an invented field.
fn assert_empty_response(case: &str, body: &Value) {
    let empty = match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: expected a delete response with no members, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: a delete drops exactly the destination it names, and every reader sees it.
// ---------------------------------------------------------------------------

/// One row: the destination's STATUS at the instant of the delete, and how long to
/// wait before deleting to reach it. A delete is accepted in either status — nothing
/// about the clock-owed verification gates it.
fn status_rows() -> Vec<(&'static str, Option<std::time::Duration>, &'static str)> {
    vec![
        (
            "deleted inside the verification span the clock owes",
            None,
            PENDING_VERIFICATION,
        ),
        (
            "deleted once the verification the clock owed has fallen due",
            Some(PAST_THE_VERIFICATION_SPAN),
            PUBLISHING,
        ),
    ]
}

#[tokio::test]
async fn a_delete_drops_exactly_the_destination_it_names() {
    for (case, wait, expected_status) in status_rows() {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        // Siblings the row must leave exactly as they are, created from create bodies
        // that differ in every property class `create_body` spans.
        let mut siblings = BTreeSet::new();
        for index in 1..4usize {
            siblings.insert(
                created(
                    &router,
                    HOME_REGION,
                    &detector_id,
                    case,
                    create_body(index),
                )
                .await,
            );
        }
        let victim = created(&router, HOME_REGION, &detector_id, case, create_body(9)).await;

        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }

        // The status the row stands for is the one the destination really reports at
        // the instant of the delete.
        let (status, read) = describe_in(&router, HOME_REGION, &detector_id, &victim).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the describe before the delete to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read["status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected the destination to be {expected_status} when deleted, got {read:?}"
        );

        let before = detector_updated_at(&router, HOME_REGION, &detector_id, case).await;
        let (status, deleted) = delete(&router, &detector_id, &victim).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
        );
        assert_empty_response(case, &deleted);

        // Every operation that addresses the destination by id now refuses it — the
        // second delete included, because the destination must be there to be deleted.
        let (status, refused) = describe_in(&router, HOME_REGION, &detector_id, &victim).await;
        assert_refusal(
            &format!("{case}: a describe after the delete"),
            status,
            &refused,
            DESTINATION_ID_MEMBER,
            &victim,
        );
        let (status, refused) = update_in(&router, HOME_REGION, &detector_id, &victim).await;
        assert_refusal(
            &format!("{case}: an update after the delete"),
            status,
            &refused,
            DESTINATION_ID_MEMBER,
            &victim,
        );
        let (status, refused) = delete(&router, &detector_id, &victim).await;
        assert_refusal(
            &format!("{case}: a second delete"),
            status,
            &refused,
            DESTINATION_ID_MEMBER,
            &victim,
        );

        // The list reports the detector's remaining destinations, nothing more.
        let listed = listed_ids(&router, HOME_REGION, &detector_id, case).await;
        assert_eq!(
            listed, siblings,
            "case {case:?}: expected only the siblings after the delete, got {listed:?}"
        );

        // And each sibling still reports exactly what its own create stored.
        for (index, sibling) in siblings.iter().enumerate() {
            let (status, read) = describe_in(&router, HOME_REGION, &detector_id, sibling).await;
            assert_eq!(
                status, 200,
                "case {case:?}: sibling {index} expected 200 after the delete, got {status} {read:?}"
            );
            assert_eq!(
                read["destinationId"].as_str(),
                Some(sibling.as_str()),
                "case {case:?}: sibling {index} must report its own id, got {read:?}"
            );
        }

        // The detector holds one fewer sub-resource than before, which is the change
        // the create records on it in the other direction.
        let after = detector_updated_at(&router, HOME_REGION, &detector_id, case).await;
        assert!(
            after > before,
            "case {case:?}: the delete changed the detector, so its updatedAt must move past \
             {before:?}, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the record goes, so the slot, the token and the tag ARN it held go too.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_frees_the_slot_the_destination_held() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let case = "the per-detector maximum";

    let mut ids = Vec::new();
    for index in 0..PUBLISHING_DESTINATIONS_PER_DETECTOR {
        ids.push(created(&router, HOME_REGION, &detector_id, case, create_body(index)).await);
    }

    // One past the bound is refused: this is the fact the delete has to undo.
    let (status, refused) = create_in(
        &router,
        HOME_REGION,
        &detector_id,
        create_body(PUBLISHING_DESTINATIONS_PER_DETECTOR),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a create past the bound must be refused, got {status} {refused:?}"
    );

    // Deleting one frees exactly one slot: the next create lands, the one after it is
    // refused again.
    let victim = ids[PUBLISHING_DESTINATIONS_PER_DETECTOR / 2].clone();
    let (status, deleted) = delete(&router, &detector_id, &victim).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
    );
    let replacement = created(
        &router,
        HOME_REGION,
        &detector_id,
        "the freed slot",
        create_body(PUBLISHING_DESTINATIONS_PER_DETECTOR),
    )
    .await;
    assert_ne!(
        replacement, victim,
        "case {case:?}: the create in the freed slot must mint a FRESH id, got {replacement:?}"
    );
    let (status, refused) = create_in(
        &router,
        HOME_REGION,
        &detector_id,
        create_body(PUBLISHING_DESTINATIONS_PER_DETECTOR + 1),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: once the freed slot is taken again a create must be refused, got {status} {refused:?}"
    );

    // The collection is the same size it was, with the replacement where the victim was.
    let listed = listed_ids(&router, HOME_REGION, &detector_id, case).await;
    assert_eq!(
        listed.len(),
        PUBLISHING_DESTINATIONS_PER_DETECTOR,
        "case {case:?}: expected {PUBLISHING_DESTINATIONS_PER_DETECTOR} destinations, got {listed:?}"
    );
    assert!(
        !listed.contains(&victim) && listed.contains(&replacement),
        "case {case:?}: expected the replacement and not the deleted destination, got {listed:?}"
    );
}

#[tokio::test]
async fn a_delete_drops_the_client_token_that_minted_the_destination() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // Token forms differing in length and alphabet: the index is a plain replay key,
    // so nothing about the token's shape changes the rule.
    let tokens = [
        "a",
        "idempotency-token-1",
        "A_MIXED.case~token-0123456789",
        "0123456789abcdef0123456789abcdef0123456789abcdef",
    ];

    for (index, token) in tokens.iter().enumerate() {
        let case = format!("the token {token:?}");
        let mut body = create_body(index);
        body["clientToken"] = json!(token);

        let first = created(&router, HOME_REGION, &detector_id, &case, body.clone()).await;
        // A replay before the delete answers with the destination the create minted.
        let replayed = created(&router, HOME_REGION, &detector_id, &case, body.clone()).await;
        assert_eq!(
            replayed, first,
            "case {case:?}: a replay before the delete must answer with the minted destination"
        );

        let (status, deleted) = delete(&router, &detector_id, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
        );

        // The mapping lived ON the record, so the token now mints a destination of its
        // own rather than resolving to an id the store no longer holds.
        let after = created(&router, HOME_REGION, &detector_id, &case, body).await;
        assert_ne!(
            after, first,
            "case {case:?}: a replay after the delete must mint a FRESH destination, got {after:?}"
        );
        let (status, read) = describe_in(&router, HOME_REGION, &detector_id, &after).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the freshly minted destination must be readable, got {status} {read:?}"
        );

        // Leave the detector as this row found it, so the next row starts clean.
        let (status, deleted) = delete(&router, &detector_id, &after).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the cleanup delete to answer 200, got {status} {deleted:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_takes_the_destinations_tags_with_it() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let case = "a tagged destination";

    let mut body = create_body(4);
    body["tags"] = json!({"Env": "prod", "Team/Owner": "security"});
    let destination_id = created(&router, HOME_REGION, &detector_id, case, body).await;

    // The tag ARN is the resource's own REST path, built exactly as this emulator
    // builds every ARN it reports, and `/tags/{ResourceArn}` binds a NON-GREEDY URI
    // label (`smithy_query(operation="UntagResource")`), so its separators travel
    // percent-encoded.
    let arn = engine::emulator_arn(
        ARN_SERVICE,
        HOME_REGION,
        &format!("detector/{detector_id}/publishingDestination/{destination_id}"),
    );
    let tags_path = format!("/tags/{}?tagKeys=Env", arn.replace('/', "%2F"));

    // Before the delete the ARN resolves the destination and the tags are the create's.
    let (status, untagged) = call(&router, "DELETE", &tags_path, None).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected the untag before the delete to answer its modelled 204, got {status} {untagged:?}"
    );
    let (status, read) = describe_in(&router, HOME_REGION, &detector_id, &destination_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the describe before the delete to answer 200, got {status} {read:?}"
    );
    assert_eq!(
        read["tags"],
        json!({"Team/Owner": "security"}),
        "case {case:?}: expected the remaining tag, got {read:?}"
    );

    let (status, deleted) = delete(&router, &detector_id, &destination_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
    );

    // The record is gone, so there is no resource for the tag ARN to name any more.
    let (status, refused) = call(&router, "DELETE", &tags_path, None).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the tag ARN must resolve nothing after the delete, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a delete reaches one detector in one Region.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_reaches_only_the_detector_and_region_it_names() {
    let router = new_router();
    let home = enabled_detector(&router).await;

    // A second detector in another Region: GuardDuty holds one per account per Region,
    // and the Region comes from the request's credential scope.
    let (status, created_detector) = call_in(
        &router,
        AWAY_REGION,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the away detector: expected 200, got {status} {created_detector:?}"
    );
    let away = created_detector["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the away detector: expected a detectorId, got {created_detector:?}"))
        .to_string();

    let home_destination = created(&router, HOME_REGION, &home, "the home destination", create_body(2)).await;
    let away_destination = created(&router, AWAY_REGION, &away, "the away destination", create_body(2)).await;

    // Addressing the other Region's detector through this Region's endpoint reaches
    // nothing, and neither does addressing a destination held by another detector.
    let (status, refused) = delete(&router, &away, &away_destination).await;
    assert_refusal(
        "a delete on another Region's detector through this Region's endpoint",
        status,
        &refused,
        DETECTOR_ID_MEMBER,
        &away,
    );
    let (status, refused) = delete_in(&router, AWAY_REGION, &home, &home_destination).await;
    assert_refusal(
        "a delete on this Region's detector through another Region's endpoint",
        status,
        &refused,
        DETECTOR_ID_MEMBER,
        &home,
    );
    let (status, refused) = delete(&router, &home, &away_destination).await;
    assert_refusal(
        "a delete of a destination another detector holds",
        status,
        &refused,
        DESTINATION_ID_MEMBER,
        &away_destination,
    );

    // None of those refusals wrote anything.
    assert_eq!(
        listed_ids(&router, HOME_REGION, &home, "after the refusals").await,
        BTreeSet::from([home_destination.clone()]),
        "case after the refusals: expected the home destination untouched"
    );
    assert_eq!(
        listed_ids(&router, AWAY_REGION, &away, "after the refusals").await,
        BTreeSet::from([away_destination.clone()]),
        "case after the refusals: expected the away destination untouched"
    );

    // Deleting the home destination leaves the other Region's alone.
    let (status, deleted) = delete(&router, &home, &home_destination).await;
    assert_eq!(
        status, 200,
        "case the home delete: expected 200, got {status} {deleted:?}"
    );
    assert!(
        listed_ids(&router, HOME_REGION, &home, "after the home delete")
            .await
            .is_empty(),
        "case after the home delete: expected the home detector to hold nothing"
    );
    let (status, read) = describe_in(&router, AWAY_REGION, &away, &away_destination).await;
    assert_eq!(
        status, 200,
        "case the away destination after the home delete: expected 200, got {status} {read:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: an id the store does not resolve is the classified 400, and writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_of_something_the_store_does_not_hold_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let destination_id = created(&router, HOME_REGION, &detector_id, "the baseline", create_body(1)).await;

    // (case, detector id, destination id, the member the message must name). The
    // member at fault is also the value the message must not quote back, so each row
    // names which of its two ids that is rather than assuming the destination.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a detector id nothing minted",
            ABSENT_DETECTOR_ID.to_string(),
            destination_id.clone(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a detector id that is not one at all",
            "not-a-detector-id".to_string(),
            destination_id.clone(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a destination id nothing minted",
            detector_id.clone(),
            ABSENT_DESTINATION_ID.to_string(),
            DESTINATION_ID_MEMBER,
        ),
        (
            "a destination id that is not one at all",
            detector_id.clone(),
            "not-a-destination-id".to_string(),
            DESTINATION_ID_MEMBER,
        ),
        (
            "the stored destination id upper-cased",
            detector_id.clone(),
            destination_id.to_uppercase(),
            DESTINATION_ID_MEMBER,
        ),
    ];

    let before = detector_updated_at(&router, HOME_REGION, &detector_id, "before the refusals").await;
    for (case, detector, destination, member) in rows {
        let (status, refused) = delete(&router, &detector, &destination).await;
        // The value at fault is the one the refusing member carried.
        let at_fault = if member == DETECTOR_ID_MEMBER {
            detector.clone()
        } else {
            destination.clone()
        };
        assert_refusal(case, status, &refused, member, &at_fault);
    }

    // Nothing was written: the destination is still there, and the detector did not
    // change either.
    let (status, read) = describe_in(&router, HOME_REGION, &detector_id, &destination_id).await;
    assert_eq!(
        status, 200,
        "case after the refusals: expected the stored destination to be readable, got {status} {read:?}"
    );
    assert_eq!(
        listed_ids(&router, HOME_REGION, &detector_id, "after the refusals").await,
        BTreeSet::from([destination_id.clone()]),
        "case after the refusals: expected the stored destination still listed"
    );
    let after = detector_updated_at(&router, HOME_REGION, &detector_id, "after the refusals").await;
    assert_eq!(
        after, before,
        "case after the refusals: a refused delete must not move the detector's updatedAt, \
         expected {before:?}, got {after:?}"
    );
}

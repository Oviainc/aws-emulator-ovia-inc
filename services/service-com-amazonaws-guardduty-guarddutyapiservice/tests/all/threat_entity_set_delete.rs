// SPDX-License-Identifier: CC0-1.0

//! `DeleteThreatEntitySet`, end to end over the wire, read back through
//! `GetThreatEntitySet` and `ListThreatEntitySets`.
//!
//! `threat_entity_set_lifecycle.rs` owns the create-and-read half and
//! `threat_entity_set_listing.rs` the list; this suite owns the delete, which
//! answered HTTP 501 `NotImplemented` and so left the
//! `create -> DELETE_PENDING -> DELETED` half of the state machine unverifiable.
//! Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `DeleteThreatEntitySet` COMPLETES: the detector stops holding the set, so
//!    `GetThreatEntitySet` refuses its id with the entity-set families' absence
//!    refusal — HTTP 404 under the `ResourceNotFoundException` class — straight
//!    after the delete and for ever afterwards, whatever status the set was in when
//!    the delete arrived and whatever the create stored. A SECOND delete is that
//!    same refusal.
//! 2. The delete is PERSISTED and is observable on the detector: its own `UpdatedAt`
//!    moves on the first delete and not on a refused repeat.
//! 3. A deleted set is no longer one GuardDuty keeps, so it occupies none of the
//!    detector's slots and `ListThreatEntitySets` stops naming it. A replayed
//!    `ClientToken` whose set was deleted mints a new set rather than handing back
//!    the deleted one.
//! 4. A delete touches only the set it names: the detector's other threat entity
//!    sets, its trusted entity set and another detector's sets are untouched.
//! 5. An id the store does not resolve is refused the same way as a deleted one, and
//!    a threat entity set is a sub-resource of a REGIONAL detector, so only the
//!    Region that created it may delete it — where the DETECTOR is what the request
//!    fails to resolve, so the refusal is the detector family's.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::set_delete_terminality::assert_entity_set_absent;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, ENTITY_SET_ACTIVATING, ENTITY_SET_ACTIVE, ENTITY_SET_INACTIVE,
    ENTITY_SET_TRANSITION_MILLIS, THREAT_ENTITY_SETS_PER_DETECTOR,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed entity-set id (32 lowercase hex characters, the form this service
/// mints) that no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// A valid Amazon S3 location, the one member most rows here keep fixed.
const LOCATION: &str = "s3://guardduty-lists/threat.txt";

/// Long enough for the clock to owe the next status, plus slack.
fn past_the_transition() -> std::time::Duration {
    let span = u64::try_from(ENTITY_SET_TRANSITION_MILLIS).unwrap_or_default();
    std::time::Duration::from_millis(span + 250)
}

// ---------------------------------------------------------------------------
// The four routes this suite drives, under `/detector/{DetectorId}/threatentityset`.
// ---------------------------------------------------------------------------

async fn create_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/threatentityset"),
        Some(body),
    )
    .await
}

async fn delete_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
        None,
    )
    .await
}

async fn get_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
        None,
    )
    .await
}

/// Create a threat entity set and return its id, failing the test if the create did
/// not succeed.
async fn created(router: &Router, detector_id: &str, case: &str, body: Value) -> String {
    let (status, got) = create_set(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected create to answer 200, got {status} {got:?}"
    );
    got["threatEntitySetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a threatEntitySetId, got {got:?}"))
        .to_string()
}

/// Read a set back, failing the test if the read did not succeed.
async fn read(router: &Router, detector_id: &str, set_id: &str, case: &str) -> Value {
    let (status, got) = get_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected read to answer 200, got {status} {got:?}"
    );
    got
}

/// The `Status` a read reports.
async fn status_of(router: &Router, detector_id: &str, set_id: &str, case: &str) -> String {
    read(router, detector_id, set_id, case).await["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Delete a set, failing the test if the call did not succeed.
async fn deleted(router: &Router, detector_id: &str, set_id: &str, case: &str) {
    let (status, got) = delete_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected delete to answer 200, got {status} {got:?}"
    );
    assert!(
        got.get("status").is_none(),
        "case {case:?}: the delete's output is Unit, so it reports no entity set, got {got:?}"
    );
}

/// The ids `ListThreatEntitySets` names for a detector. The whole collection is
/// bounded well under `ThreatEntitySetIds`'s own `len 0..50`, so it arrives in one
/// page and this helper needs no token walk (`threat_entity_set_listing.rs` owns
/// pagination).
async fn listed(router: &Router, detector_id: &str, case: &str) -> Vec<String> {
    let (status, got) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/threatentityset"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected list to answer 200, got {status} {got:?}"
    );
    assert!(
        got["nextToken"].is_null(),
        "case {case:?}: the whole collection fits one page, got {got:?}"
    );
    got["threatEntitySetIds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected threatEntitySetIds, got {got:?}"))
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect()
}

/// `GetDetector`, so an assertion can see the detector's own `UpdatedAt`.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

/// A detector's own `UpdatedAt`, which `GetDetector` reports as an ISO-8601 string
/// rather than as the epoch seconds an entity set's timestamps carry
/// (`smithy_query(operation="GetDetector")`). The format is fixed and UTC, so the
/// strings order the same way the instants do.
fn detector_updated_at(got: &Value, case: &str) -> String {
    got["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the detector's updatedAt, got {got:?}"))
        .to_string()
}

/// A router holding one enabled detector.
async fn detector() -> (Router, String) {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    (router, detector_id)
}

/// Assert a refusal is this service's classified HTTP 400, names `member` and echoes
/// none of `never_echoed`.
fn assert_classified_refusal(
    case: &str,
    status: u16,
    body: &Value,
    member: &str,
    never_echoed: &[&str],
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(member),
        "case {case:?}: the refusal must name {member}, got {message:?}"
    );
    for value in never_echoed {
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not echo {value:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1 + 2: the set is gone, and the detector records that it is.
// ---------------------------------------------------------------------------

/// One delete case: the create body, and the status the set is in when the delete
/// arrives.
///
/// The rows differ along everything the rule generalises over: the status the set
/// held when the delete arrived (each of the three a created set can be in, the
/// transitional one included), the FORMAT (drawn from
/// `ThreatEntitySetFormat`'s modelled values, `smithy_query(shape=
/// "ThreatEntitySetFormat")`), whether the caller activated the set, whether it
/// carried an expected bucket owner, the number of TAGS and the SIZE of the name
/// (both ends of the modelled `len 1..300`).
struct DeleteRow {
    case: &'static str,
    create: Value,
    /// Wait past the transition before deleting, so the set is settled first.
    settle_first: bool,
    status_when_deleted: &'static str,
}

fn delete_rows() -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            case: "an INACTIVE set, created without tags",
            create: json!({
                "name": "a",
                "format": "TXT",
                "location": LOCATION,
                "activate": false,
            }),
            settle_first: false,
            status_when_deleted: ENTITY_SET_INACTIVE,
        },
        DeleteRow {
            case: "an ACTIVATING set, deleted mid-transition",
            create: json!({
                "name": "activating-threat-set",
                "format": "PROOF_POINT",
                "location": "https://s3.amazonaws.com/guardduty-lists/threat.pp",
                "activate": true,
                "tags": {"team": "secops"},
            }),
            settle_first: false,
            status_when_deleted: ENTITY_SET_ACTIVATING,
        },
        DeleteRow {
            case: "an ACTIVE set, settled before the delete",
            create: json!({
                "name": "ALIEN_vault-1",
                "format": "ALIEN_VAULT",
                "location": "https://guardduty-lists.s3.us-west-2.amazonaws.com/otx.csv",
                "activate": true,
                "expectedBucketOwner": "123456789012",
                "tags": {"team": "secops", "env": "test"},
            }),
            settle_first: true,
            status_when_deleted: ENTITY_SET_ACTIVE,
        },
        DeleteRow {
            case: "an INACTIVE set with the longest name the model accepts",
            create: json!({
                "name": "f".repeat(300),
                "format": "FIRE_EYE",
                "location": "s3://guardduty-lists/deep/nested/fire-eye.txt",
                "activate": false,
                "expectedBucketOwner": "000000000000",
            }),
            settle_first: true,
            status_when_deleted: ENTITY_SET_INACTIVE,
        },
    ]
}

#[tokio::test]
async fn a_delete_takes_the_set_off_the_detector_at_once() {
    for row in delete_rows() {
        let case = row.case;
        let (router, detector_id) = detector().await;
        let set_id = created(&router, &detector_id, case, row.create.clone()).await;
        if row.settle_first {
            tokio::time::sleep(past_the_transition()).await;
        }

        let before = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            before["status"], row.status_when_deleted,
            "case {case:?}: expected status {} before the delete, got {:?}",
            row.status_when_deleted, before["status"]
        );
        let detector_before = read_detector(&router, &detector_id, case).await;

        deleted(&router, &detector_id, &set_id, case).await;

        // Rule 1: the delete completed, so there is nothing left to read — straight
        // afterwards, with no span for a caller to observe.
        let (status, gone) = get_set(&router, &detector_id, &set_id).await;
        assert_entity_set_absent(case, status, &gone, &set_id);

        // Rule 2: the detector holds one fewer sub-resource, so its own UpdatedAt
        // moved.
        let detector_after = read_detector(&router, &detector_id, case).await;
        assert!(
            detector_updated_at(&detector_after, case) > detector_updated_at(&detector_before, case),
            "case {case:?}: the first delete changed the detector, so its updatedAt must move, \
             got {:?} after {:?}",
            detector_after["updatedAt"],
            detector_before["updatedAt"]
        );

        // A second delete is the same absence refusal, and moves neither the store
        // nor the detector.
        let (status, body) = delete_set(&router, &detector_id, &set_id).await;
        assert_entity_set_absent(case, status, &body, &set_id);
        let detector_again = read_detector(&router, &detector_id, case).await;
        assert_eq!(
            detector_updated_at(&detector_again, case),
            detector_updated_at(&detector_after, case),
            "case {case:?}: a refused second delete changed nothing, so the detector's updatedAt \
             must stay put, got {detector_again:?}"
        );

        // And once every transition this service opens would have come due, the set
        // is still gone: nothing brings a deleted set back.
        tokio::time::sleep(past_the_transition()).await;
        let (status, still_gone) = get_set(&router, &detector_id, &set_id).await;
        assert_entity_set_absent(case, status, &still_gone, &set_id);
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a deleted set leaves the collection and frees its slot.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_set_leaves_the_collection_and_frees_its_slot() {
    // The rows differ along whether the list and the next create arrive straight
    // after the delete or once every transition would have come due.
    for (case, wait) in [
        ("straight after the delete", false),
        ("once every transition would have come due", true),
    ] {
        let (router, detector_id) = detector().await;

        // Fill the detector to its ceiling, so the next create has nowhere to go.
        let mut ids = Vec::new();
        for index in 0..THREAT_ENTITY_SETS_PER_DETECTOR {
            ids.push(
                created(
                    &router,
                    &detector_id,
                    case,
                    json!({
                        "name": format!("threat-set-{index}"),
                        "format": "TXT",
                        "location": format!("s3://guardduty-lists/threat-{index}.txt"),
                        "activate": index % 2 == 0,
                    }),
                )
                .await,
            );
        }
        let mut expected = ids.clone();
        expected.sort();
        assert_eq!(
            listed(&router, &detector_id, case).await,
            expected,
            "case {case:?}: the list must name every created set"
        );

        let (status, refused) = create_set(
            &router,
            &detector_id,
            json!({
                "name": "one-too-many",
                "format": "STIX",
                "location": LOCATION,
                "activate": false,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a create past the ceiling to be refused, got {status} \
             {refused:?}"
        );

        // Delete the middle one, so the assertion is not about the first or the last.
        let victim = ids[ids.len() / 2].clone();
        deleted(&router, &detector_id, &victim, case).await;
        if wait {
            tokio::time::sleep(past_the_transition()).await;
        }

        let remaining = listed(&router, &detector_id, case).await;
        assert!(
            !remaining.contains(&victim),
            "case {case:?}: a deleted set is no longer part of the collection, got {remaining:?}"
        );
        let mut still_there: Vec<String> =
            ids.iter().filter(|id| **id != victim).cloned().collect();
        still_there.sort();
        assert_eq!(
            remaining, still_there,
            "case {case:?}: the delete must leave every other set in the list"
        );

        // It is gone: leaving the collection and leaving the detector are the same
        // thing for this family, so its own id resolves nothing either.
        let (status, body) = get_set(&router, &detector_id, &victim).await;
        assert_entity_set_absent(case, status, &body, &victim);

        // The slot is free again, so the create that was just refused now succeeds.
        let replacement = created(
            &router,
            &detector_id,
            case,
            json!({
                "name": "one-too-many",
                "format": "STIX",
                "location": LOCATION,
                "activate": false,
            }),
        )
        .await;
        assert!(
            !ids.contains(&replacement),
            "case {case:?}: the create after the delete must mint a new id, got {replacement}"
        );
        let after = listed(&router, &detector_id, case).await;
        assert_eq!(
            after.len(),
            THREAT_ENTITY_SETS_PER_DETECTOR,
            "case {case:?}: the detector is back at its ceiling, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_whose_set_was_deleted_mints_a_new_set() {
    for (case, wait) in [
        ("replayed straight after the delete", false),
        ("replayed once every transition would have come due", true),
    ] {
        let (router, detector_id) = detector().await;
        let body = json!({
            "name": "token-set",
            "format": "OTX_CSV",
            "location": LOCATION,
            "activate": true,
            "clientToken": "replayed-token",
        });

        let first = created(&router, &detector_id, case, body.clone()).await;
        // Before the delete the token is a pure replay key: it hands back that set.
        let replay = created(&router, &detector_id, case, body.clone()).await;
        assert_eq!(
            replay, first,
            "case {case:?}: a replayed token must hand back the set it minted"
        );

        deleted(&router, &detector_id, &first, case).await;
        if wait {
            tokio::time::sleep(past_the_transition()).await;
        }

        let second = created(&router, &detector_id, case, body).await;
        assert_ne!(
            second, first,
            "case {case:?}: a replayed token whose set was deleted must mint a new set, got \
             {second} again"
        );
        let got = read(&router, &detector_id, &second, case).await;
        assert_eq!(
            got["status"], ENTITY_SET_ACTIVATING,
            "case {case:?}: the new set starts its own transition, got {:?}",
            got["status"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a delete touches only the set it names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_touches_only_the_set_it_names() {
    let router = new_router();
    let case = "sibling resources";
    let first_detector = enabled_detector(&router).await;
    // A second detector in another Region, so it is a store of its own.
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the second detector to be created, got {status} {body:?}"
    );
    let other_detector = body["detectorId"]
        .as_str()
        .expect("a detectorId")
        .to_string();

    let mine = created(
        &router,
        &first_detector,
        case,
        json!({
            "name": "mine",
            "format": "TXT",
            "location": LOCATION,
            "activate": false,
        }),
    )
    .await;
    // A SECOND threat entity set under the same detector: a detector may hold
    // several, so a delete has siblings in its own collection to leave alone.
    let sibling = created(
        &router,
        &first_detector,
        case,
        json!({
            "name": "sibling",
            "format": "STIX",
            "location": "s3://guardduty-lists/sibling.txt",
            "activate": false,
        }),
    )
    .await;
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        &format!("/detector/{other_detector}/threatentityset"),
        Some(json!({
            "name": "theirs",
            "format": "TXT",
            "location": LOCATION,
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other detector's create to answer 200, got {status} {body:?}"
    );
    let theirs = body["threatEntitySetId"]
        .as_str()
        .expect("a threatEntitySetId")
        .to_string();

    // A TRUSTED entity set under the same detector: a different collection, which a
    // threat delete must not reach into.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{first_detector}/trustedentityset"),
        Some(json!({
            "name": "trusted-set",
            "format": "TXT",
            "location": "s3://guardduty-lists/trusted.txt",
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the trusted create to answer 200, got {status} {body:?}"
    );
    let trusted = body["trustedEntitySetId"]
        .as_str()
        .expect("a trustedEntitySetId")
        .to_string();

    deleted(&router, &first_detector, &mine, case).await;

    let sibling_status = status_of(&router, &first_detector, &sibling, case).await;
    assert_eq!(
        sibling_status, ENTITY_SET_INACTIVE,
        "case {case:?}: a sibling threat entity set must be untouched, got {sibling_status:?}"
    );

    let (status, other) = call_in(
        &router,
        "eu-west-1",
        "GET",
        &format!("/detector/{other_detector}/threatentityset/{theirs}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other detector's set to still read, got {status} {other:?}"
    );
    assert_eq!(
        other["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: another detector's set must be untouched, got {other:?}"
    );

    let (status, trusted_set) = call(
        &router,
        "GET",
        &format!("/detector/{first_detector}/trustedentityset/{trusted}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the trusted set to still read, got {status} {trusted_set:?}"
    );
    assert_eq!(
        trusted_set["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: a threat delete must not touch a trusted entity set, got {trusted_set:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: an id the store does not resolve, and the Region that owns the set.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_id_is_refused_rather_than_answered() {
    let (router, detector_id) = detector().await;
    let set_id = created(
        &router,
        &detector_id,
        "rejections",
        json!({
            "name": "rejection-set",
            "format": "TXT",
            "location": LOCATION,
            "activate": true,
        }),
    )
    .await;

    // Each row is an id no store resolves: a well-formed detector id and a value
    // that cannot be one, then a well-formed set id and a value that cannot be one.
    // The detector rows are refused with the detector family's own refusal and the
    // set rows with the entity-set families' absence refusal, because they name
    // different resources.
    for (case, detector_arg) in [
        ("absent detector", ABSENT_DETECTOR_ID.to_string()),
        ("malformed detector", "not-a-detector".to_string()),
    ] {
        let (status, body) = delete_set(&router, &detector_arg, &set_id).await;
        assert_classified_refusal(
            case,
            status,
            &body,
            DETECTOR_ID_MEMBER,
            &[detector_arg.as_str(), set_id.as_str()],
        );
        // The refusal deleted nothing: the set that does exist is still live.
        let (status, body) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must not touch the stored set, got {status} {body:?}"
        );
    }
    for (case, set_arg) in [
        ("absent set", ABSENT_SET_ID),
        ("malformed set", "not-a-set"),
    ] {
        let (status, body) = delete_set(&router, &detector_id, set_arg).await;
        assert_entity_set_absent(case, status, &body, set_arg);
        let (status, body) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must not touch the stored set, got {status} {body:?}"
        );
    }

    // A threat entity set is a sub-resource of a REGIONAL detector, so a request
    // signed for another Region resolves neither the detector nor the set.
    let (status, body) = call_in(
        &router,
        "ap-southeast-2",
        "DELETE",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
        None,
    )
    .await;
    assert_classified_refusal(
        "another Region",
        status,
        &body,
        DETECTOR_ID_MEMBER,
        &[detector_id.as_str(), set_id.as_str()],
    );
    let survived = status_of(&router, &detector_id, &set_id, "another Region").await;
    // The refusal moved nothing, so the set is still one of the detector's LIVE
    // sets: it is in whichever status its own transition has reached, never one the
    // delete would have put it in.
    assert!(
        survived == ENTITY_SET_ACTIVATING || survived == ENTITY_SET_ACTIVE,
        "case \"another Region\": the set must be untouched, so it is still \
         {ENTITY_SET_ACTIVATING} or {ENTITY_SET_ACTIVE}, got {survived:?}"
    );
}

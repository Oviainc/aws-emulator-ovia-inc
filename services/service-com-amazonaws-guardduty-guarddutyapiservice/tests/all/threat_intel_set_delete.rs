// SPDX-License-Identifier: CC0-1.0

//! `DeleteThreatIntelSet`, end to end over the wire, read back through
//! `GetThreatIntelSet`.
//!
//! `threat_intel_sets.rs` owns the create-and-read half of the threat-list lifecycle;
//! this suite owns the delete, which answered HTTP 501 `NotImplemented` and so left
//! the `create -> (ACTIVATING -> ACTIVE) -> DELETED` state machine
//! unverifiable. Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. `DeleteThreatIntelSet` completes at once: the list settles on `DELETED`, the
//!    terminal member of `GetThreatIntelSet`'s `@required` `Status` enum
//!    (`smithy_query(shape="ThreatIntelSetStatus")`), so the status after a delete is
//!    something a read reports rather than a refusal — whatever status the list was
//!    in when the delete arrived, and without touching any other member.
//! 2. The id stops being one the detector holds, so a SECOND delete is refused
//!    exactly like an id it never held, and writes nothing.
//! 3. A deleted list occupies none of the detector's
//!    `state::THREAT_INTEL_SETS_PER_DETECTOR` slots, so a delete frees room for a
//!    create, its `ClientToken` no longer resolves to it, `ListThreatIntelSets`
//!    stops naming it, and nothing a delete did touches the detector's other threat
//!    lists.
//! 4. The retained deleted records are BOUNDED at
//!    `state::DELETED_THREAT_INTEL_SETS_RETAINED` per detector, oldest deletion
//!    first, so create-and-delete in a loop does not make the store grow for ever.
//! 5. An id the store does not resolve is refused with HTTP 400 under the
//!    `InvalidInputException` class, naming the MEMBER and never echoing the caller's
//!    value, and it deletes nothing — and a threat list is a sub-resource of a
//!    REGIONAL detector, so only the Region that created it may delete it.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::SET_ID_NOT_FOUND_MESSAGE;

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modeled error SHAPE that class is enveloped in, which restJson1 resolves the
/// error shape from (the `x-amzn-errortype` header).
const ENVELOPE: &str = "BadRequestException";

/// `ThreatIntelSetStatus` members this slice reaches
/// (`smithy_query(shape="ThreatIntelSetStatus")`).
const INACTIVE: &str = "INACTIVE";
const ACTIVATING: &str = "ACTIVATING";
const ACTIVE: &str = "ACTIVE";
const DELETED: &str = "DELETED";

/// A margin on top of a transition span, for the wall clock the handler reads.
const SETTLE_MARGIN_MILLIS: u64 = 150;

/// Long enough that an activated list's `ACTIVATING` -> `ACTIVE` transition is due.
pub(crate) fn past_activation() -> Duration {
    let span = u64::try_from(state::THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS).unwrap_or_default();
    Duration::from_millis(span + SETTLE_MARGIN_MILLIS)
}

/// Long enough that any transition this family opens would have come due.
pub(crate) fn past_deletion() -> Duration {
    let span = u64::try_from(state::THREAT_INTEL_SET_DELETION_SPAN_MILLIS).unwrap_or_default();
    Duration::from_millis(span + SETTLE_MARGIN_MILLIS)
}

// ---------------------------------------------------------------------------
// The three routes this suite drives, under `/detector/{DetectorId}/threatintelset`.
// ---------------------------------------------------------------------------

async fn create_set(router: &Router, detector_id: &str, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/threatintelset"),
        Some(body.clone()),
    )
    .await
}

async fn get_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/threatintelset/{set_id}"),
        None,
    )
    .await
}

async fn delete_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/threatintelset/{set_id}"),
        None,
    )
    .await
}

/// Every `ThreatIntelSetId` the detector names, failing the test if the list did not
/// succeed. `maxResults` is left unset, so one page carries them all here.
async fn listed_ids(router: &Router, detector_id: &str, case: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/threatintelset"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-threat-intel-sets expected 200, got {status} {body:?}"
    );
    let mut ids: Vec<String> = body["threatIntelSetIds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected threatIntelSetIds, got {body:?}"))
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// Create a list and return its minted id, failing the test if the create did not
/// succeed.
async fn created_set_id(router: &Router, detector_id: &str, body: &Value) -> String {
    let (status, created) = create_set(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {body}: create-threat-intel-set expected 200, got {status} {created:?}"
    );
    created["threatIntelSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: expected a threatIntelSetId, got {created:?}"))
        .to_string()
}

/// Read a list back, failing the test if the read did not succeed.
async fn read(router: &Router, detector_id: &str, set_id: &str, case: &str) -> Value {
    let (status, body) = get_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-threat-intel-set expected 200, got {status} {body:?}"
    );
    body
}

/// Accept a delete, failing the test if it did not succeed. `DeleteThreatIntelSet`'s
/// output shape has no members (`smithy_query(operation="DeleteThreatIntelSet")`), so
/// success is the modelled 200 and a body carrying nothing.
async fn accepted_delete(router: &Router, detector_id: &str, set_id: &str, case: &str) {
    let (status, body) = delete_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: delete-threat-intel-set expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body,
        json!({}),
        "case {case:?}: delete-threat-intel-set answers a body with no members, got {body:?}"
    );
}

/// Assert one rejection: the status, the error class in all three places it appears,
/// the modeled envelope shape, and the classified message.
fn assert_rejection(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some(CLASS),
        "case {case:?}: expected the error class in __type, got {body:?}"
    );
    assert_eq!(
        body["type"].as_str(),
        Some(CLASS),
        "case {case:?}: expected the error class in the type member, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        CLASS,
        "case {case:?}: expected the error class on the wire, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case:?}: expected the modeled envelope shape in x-amzn-errortype, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected_message),
        "case {case:?}: expected the classified message, got {body:?}"
    );
}

fn invalid_parameter(member: &str) -> String {
    format!("The request is rejected because the parameter {member} has an invalid value.")
}

fn not_owned(member: &str) -> String {
    format!(
        "The request is rejected because the input {member} is not owned by the current account."
    )
}

// ---------------------------------------------------------------------------
// Rule 1: a delete settles the list on DELETED, whatever status it was in.
// ---------------------------------------------------------------------------

/// The rows differ along the status the list was in when the delete arrived: the
/// settled `INACTIVE`, the TRANSIENT `ACTIVATING` (a transition the clock had not
/// settled yet), and the settled `ACTIVE`.
#[tokio::test]
async fn a_delete_settles_any_status_on_deleted() {
    for (case, request, wait_before_delete, status_before) in [
        (
            "a list nobody activated",
            json!({
                "name": "inactive-list",
                "format": "TXT",
                "location": "s3://threat-lists/inactive.txt",
                "activate": false,
                "tags": {"team": "security"},
            }),
            false,
            INACTIVE,
        ),
        (
            "a list still activating",
            json!({
                "name": "activating-list",
                "format": "STIX",
                "location": "s3://threat-lists/activating.xml",
                "activate": true,
            }),
            false,
            ACTIVATING,
        ),
        (
            "a list whose activation is due",
            json!({
                "name": "active-list",
                "format": "OTX_CSV",
                "location": "s3://threat-lists/active.csv",
                "activate": true,
                "expectedBucketOwner": "123456789012",
            }),
            true,
            ACTIVE,
        ),
    ] {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let set_id = created_set_id(&router, &detector_id, &request).await;
        if wait_before_delete {
            sleep(past_activation());
        }

        let before = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            before["status"].as_str(),
            Some(status_before),
            "case {case:?}: expected status {status_before} before the delete, got {:?}",
            before["status"]
        );

        accepted_delete(&router, &detector_id, &set_id, case).await;

        // The list is still READABLE on its own id, reporting the terminal status
        // the delete settled it on: `DELETED` is a member of the modelled enum, so a
        // delete is not an erasure a read refuses. There is no span to observe, so
        // the status is terminal STRAIGHT after the delete.
        let deleted = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            deleted["status"].as_str(),
            Some(DELETED),
            "case {case:?}: a read straight after the delete must report {DELETED}, got {:?}",
            deleted["status"]
        );
        // Reading changes nothing a later read sees.
        let again = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            again, deleted,
            "case {case:?}: reading must not move the status, got {deleted:?} then {again:?}"
        );
        // And the delete touched nothing else the create stored.
        for member in ["name", "format", "location", "expectedBucketOwner", "tags"] {
            // A read always reports a tag map, empty when the create named none.
            let expected = request.get(member).cloned().unwrap_or(if member == "tags" {
                json!({})
            } else {
                Value::Null
            });
            assert_eq!(
                deleted[member], expected,
                "case {case:?}: member {member} must survive the delete, got {:?}",
                deleted[member]
            );
        }

        sleep(past_deletion());

        // `DELETED` is terminal and persisted, not recomputed per call: waiting out
        // any span this service opens moves nothing.
        let stable = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            stable, deleted,
            "case {case:?}: {DELETED} is terminal, got {deleted:?} then {stable:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a second delete is refused like an id the detector never held.
// ---------------------------------------------------------------------------

/// The rows differ along WHEN the second delete arrives: straight after the first,
/// and once every transition this service opens would have come due.
#[tokio::test]
async fn a_repeated_delete_is_refused_and_moves_nothing() {
    for (case, wait_before_second) in [
        ("a second delete straight after the first", false),
        ("a second delete a span later", true),
    ] {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let set_id = created_set_id(
            &router,
            &detector_id,
            &json!({
                "name": "twice-deleted",
                "format": "TXT",
                "location": "s3://threat-lists/twice.txt",
                "activate": true,
            }),
        )
        .await;

        accepted_delete(&router, &detector_id, &set_id, case).await;
        if wait_before_second {
            sleep(past_deletion());
        }
        let before = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            before["status"].as_str(),
            Some(DELETED),
            "case {case:?}: expected status {DELETED} before the second delete, got {:?}",
            before["status"]
        );

        // The id is no longer one the detector holds, so the second delete is
        // refused with the family's absence sentence and writes nothing.
        let (status, body) = delete_set(&router, &detector_id, &set_id).await;
        assert_rejection(case, status, &body, SET_ID_NOT_FOUND_MESSAGE);
        let after = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must move nothing, got {before:?} then {after:?}"
        );

        // Nor does it reopen a transition: the record is still DELETED a span later.
        sleep(past_deletion());
        let later = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            later, before,
            "case {case:?}: a refused delete must not reopen a transition, got {later:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a deleted list occupies none of the detector's quota.
// ---------------------------------------------------------------------------

/// The body of the `position`-th list of a quota-filling run.
fn quota_request(position: usize) -> Value {
    json!({
        "name": format!("list-{position}"),
        "format": "TXT",
        "location": format!("s3://threat-lists/{position}.txt"),
        "activate": false,
        "clientToken": format!("token-{position}"),
    })
}

#[tokio::test]
async fn a_deleted_list_frees_a_slot_of_the_published_quota() {
    let quota = state::THREAT_INTEL_SETS_PER_DETECTOR;
    let limit_message = format!(
        "The request is rejected because the account has reached the limit of {quota} threat \
         lists."
    );
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let mut ids: Vec<String> = Vec::new();
    for position in 0..quota {
        ids.push(created_set_id(&router, &detector_id, &quota_request(position)).await);
    }

    let over_quota = json!({
        "name": "over-quota",
        "format": "STIX",
        "location": "s3://threat-lists/over.xml",
        "activate": false,
        "clientToken": "token-past-the-quota",
    });
    let (status, refused) = create_set(&router, &detector_id, &over_quota).await;
    assert_rejection("a create with the quota full", status, &refused, &limit_message);

    // One delete frees exactly one slot.
    accepted_delete(&router, &detector_id, &ids[0], "the first list of the quota").await;

    // A replay of the DELETED list's `ClientToken` is no longer answered with it: a
    // token resolves to a list the service is still keeping, so this is a fresh
    // create, which the freed slot admits.
    let replayed = created_set_id(&router, &detector_id, &quota_request(0)).await;
    assert_ne!(
        replayed, ids[0],
        "case \"a replayed token of a deleted list\": must mint a new list, got {replayed:?}"
    );

    // The slot is taken again, so the next create is refused once more: the delete
    // freed ONE slot, not the quota.
    let (status, refused) = create_set(&router, &detector_id, &over_quota).await;
    assert_rejection(
        "a create after the freed slot was taken",
        status,
        &refused,
        &limit_message,
    );

    // A second delete frees another slot, for an untokened create this time.
    accepted_delete(&router, &detector_id, &ids[1], "the second list of the quota").await;
    let fresh = created_set_id(
        &router,
        &detector_id,
        &json!({
            "name": "after-the-second-delete",
            "format": "FIRE_EYE",
            "location": "s3://threat-lists/after.txt",
            "activate": false,
        }),
    )
    .await;
    let got = read(&router, &detector_id, &fresh, "the list the second delete made room for").await;
    assert_eq!(
        got["name"].as_str(),
        Some("after-the-second-delete"),
        "case \"the list the second delete made room for\": got {got:?}"
    );
    assert_eq!(
        got["status"].as_str(),
        Some(INACTIVE),
        "case \"the list the second delete made room for\": got {got:?}"
    );

    // Both deleted lists are still reported, in their terminal status, and every
    // list the deletes did not name is untouched — same name, same status.
    for (position, id) in ids.iter().enumerate().take(2) {
        let deleted = read(&router, &detector_id, id, "a deleted list of the quota run").await;
        assert_eq!(
            deleted["status"].as_str(),
            Some(DELETED),
            "case \"deleted list {position}\": got {deleted:?}"
        );
    }
    for (position, id) in ids.iter().enumerate().skip(2) {
        let survivor = read(&router, &detector_id, id, "a list no delete named").await;
        assert_eq!(
            survivor["name"].as_str(),
            Some(format!("list-{position}").as_str()),
            "case \"list {position} after the deletes\": got {survivor:?}"
        );
        assert_eq!(
            survivor["status"].as_str(),
            Some(INACTIVE),
            "case \"list {position} after the deletes\": a delete elsewhere must not move this \
             status, got {survivor:?}"
        );
    }
}

/// A deleted list is no longer one of the detector's threat lists, so
/// `ListThreatIntelSets` stops naming it — while it stays readable on its own id,
/// because being readable is not the same as being part of the collection a list
/// enumerates. The rows differ along WHICH of the detector's lists is deleted, since
/// the listing is ordered by id and a delete must not disturb the rest of the page.
#[tokio::test]
async fn a_deleted_list_is_no_longer_named_by_the_listing() {
    for (case, victim) in [
        ("delete the first of three", 0usize),
        ("delete the middle of three", 1),
        ("delete the last of three", 2),
    ] {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let mut ids: Vec<String> = Vec::new();
        for position in 0..3 {
            ids.push(created_set_id(&router, &detector_id, &quota_request(position)).await);
        }
        let mut before = ids.clone();
        before.sort();
        assert_eq!(
            listed_ids(&router, &detector_id, case).await,
            before,
            "case {case:?}: the listing must name every list the creates minted"
        );

        accepted_delete(&router, &detector_id, &ids[victim], case).await;

        let mut expected: Vec<String> = ids
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != victim)
            .map(|(_, id)| id.clone())
            .collect();
        expected.sort();
        assert_eq!(
            listed_ids(&router, &detector_id, case).await,
            expected,
            "case {case:?}: the listing must drop the deleted list and keep the others"
        );
        // Still readable on its own id, with the terminal status the delete settled
        // it on.
        let deleted = read(&router, &detector_id, &ids[victim], case).await;
        assert_eq!(
            deleted["status"].as_str(),
            Some(DELETED),
            "case {case:?}: the deleted list must still be readable, got {deleted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the retained deleted records are bounded.
// ---------------------------------------------------------------------------
/// Create and delete one list at a time, one more time than the retention ceiling:
/// the newest `DELETED_THREAT_INTEL_SETS_RETAINED` deleted lists are still reported
/// and the oldest deletion has been dropped, so the store does not grow for ever.
#[tokio::test]
async fn the_retained_deleted_lists_are_bounded_oldest_deletion_first() {
    let retained = state::DELETED_THREAT_INTEL_SETS_RETAINED;
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let mut ids: Vec<String> = Vec::new();
    for position in 0..=retained {
        let id = created_set_id(
            &router,
            &detector_id,
            &json!({
                "name": format!("churn-{position}"),
                "format": "TXT",
                "location": format!("s3://threat-lists/churn-{position}.txt"),
                "activate": false,
            }),
        )
        .await;
        accepted_delete(&router, &detector_id, &id, "a churned list").await;
        ids.push(id);
    }

    // The oldest deletion is the one evicted.
    let (status, refused) = get_set(&router, &detector_id, &ids[0]).await;
    assert_rejection(
        "the oldest deleted list, past the retention ceiling",
        status,
        &refused,
        SET_ID_NOT_FOUND_MESSAGE,
    );

    // Every later deletion is still reported, in the terminal status its delete
    // settled it on.
    for (position, id) in ids.iter().enumerate().skip(1) {
        let got = read(&router, &detector_id, id, "a retained deleted list").await;
        assert_eq!(
            got["status"].as_str(),
            Some(DELETED),
            "case \"retained deleted list {position}\": expected {DELETED}, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: an id the store does not resolve is refused, and deletes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_the_detector_does_not_hold_is_refused_and_deletes_nothing() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let stored = created_set_id(
        &router,
        &detector_id,
        &json!({
            "name": "stored-list",
            "format": "TXT",
            "location": "s3://threat-lists/stored.txt",
            "activate": false,
        }),
    )
    .await;

    // An id the detector does not hold is ONE refusal on this family, whatever the
    // form of the value: the rows differ along LENGTH (both sides of the 32-character
    // boundary) and along the ALPHABET.
    for (id, why) in [
        ("deadbeefdeadbeefdeadbeefdeadbee", "31 characters"),
        ("deadbeefdeadbeefdeadbeefdeadbeef0", "33 characters"),
        ("deadbeefdeadbeefdeadbeefdeadbeeg", "32 characters, 'g' is not hex"),
        ("not-a-threat-intel-set-id", "not hexadecimal at all"),
        ("0", "a single character"),
    ] {
        let (status, body) = delete_set(&router, &detector_id, id).await;
        assert_rejection(why, status, &body, SET_ID_NOT_FOUND_MESSAGE);
        assert!(
            !body["message"].as_str().unwrap_or_default().contains(id),
            "case {why:?}: a rejection must not echo the caller's value, got {body:?}"
        );
    }

    // Well-formed ids no create minted here, including the stored id upper-cased:
    // case is not significant to the FORM, and the list is absent either way.
    for (id, why) in [
        (
            "deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
            "32 lowercase hex characters",
        ),
        ("00000000000000000000000000000000".to_string(), "32 zeros"),
        (stored.to_uppercase(), "a stored id, upper-cased"),
    ] {
        let (status, body) = delete_set(&router, &detector_id, &id).await;
        assert_rejection(why, status, &body, SET_ID_NOT_FOUND_MESSAGE);
    }

    // A detector the request's Region does not hold is refused on the DETECTOR, and a
    // value that is not a detector id at all is the parameter failure.
    for (detector, why, expected) in [
        (
            ABSENT_DETECTOR_ID.to_string(),
            "an absent detector",
            not_owned("detectorId"),
        ),
        (
            "not-a-detector-id".to_string(),
            "a malformed detector id",
            invalid_parameter("detectorId"),
        ),
    ] {
        let (status, body) = delete_set(&router, &detector, &stored).await;
        assert_rejection(why, status, &body, &expected);
    }

    // Nothing above deleted anything: the stored list is still live.
    let got = read(&router, &detector_id, &stored, "the stored list after the refusals").await;
    assert_eq!(
        got["status"].as_str(),
        Some(INACTIVE),
        "case \"the stored list after the refusals\": a refused delete must move nothing, got \
         {got:?}"
    );
}

/// A threat list is a sub-resource of a REGIONAL detector, so only the Region that
/// created it may delete it.
#[tokio::test]
async fn only_the_region_that_created_a_list_may_delete_it() {
    let home = "eu-central-1";
    let elsewhere = "ap-southeast-2";
    let router = new_router();

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
        "case \"setup\": create-detector in {home} got {status} {created:?}"
    );
    let detector_id = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector: expected a detectorId, got {created:?}"))
        .to_string();

    let (status, created) = call_in(
        &router,
        home,
        "POST",
        &format!("/detector/{detector_id}/threatintelset"),
        Some(json!({
            "name": "regional-list",
            "format": "TXT",
            "location": "s3://threat-lists/regional.txt",
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"setup\": create-threat-intel-set in {home} got {status} {created:?}"
    );
    let set_id = created["threatIntelSetId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let path = format!("/detector/{detector_id}/threatintelset/{set_id}");
    let (status, refused) = call_in(&router, elsewhere, "DELETE", &path, None).await;
    assert_rejection(
        "a delete from another Region",
        status,
        &refused,
        &not_owned("detectorId"),
    );

    // The list is untouched, read through its own Region's endpoint.
    let (status, got) = call_in(&router, home, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case \"read through its own Region\": expected 200, got {status} {got:?}"
    );
    assert_eq!(
        got["status"].as_str(),
        Some(INACTIVE),
        "case \"read through its own Region\": a refused delete must move nothing, got {got:?}"
    );

    // Its own Region deletes it.
    let (status, body) = call_in(&router, home, "DELETE", &path, None).await;
    assert_eq!(
        status, 200,
        "case \"a delete from its own Region\": expected 200, got {status} {body:?}"
    );
    let (_, got) = call_in(&router, home, "GET", &path, None).await;
    assert_eq!(
        got["status"].as_str(),
        Some(DELETED),
        "case \"a delete from its own Region\": expected {DELETED}, got {got:?}"
    );
}

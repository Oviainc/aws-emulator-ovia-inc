// SPDX-License-Identifier: CC0-1.0

//! `DeleteTrustedEntitySet`, end to end over the wire, read back through
//! `GetTrustedEntitySet`.
//!
//! `trusted_entity_set_lifecycle.rs` owns the create-and-read half; this suite owns
//! the delete, which answered HTTP 501 `NotImplemented` and so left the
//! `create -> DELETE_PENDING -> DELETED` half of the state machine unverifiable.
//! Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `DeleteTrustedEntitySet` moves the set to `DELETE_PENDING` and the clock owes
//!    it `DELETED` — `GetTrustedEntitySet`'s `Status` is `@required` and its enum
//!    carries both (`smithy_query(operation="GetTrustedEntitySet")`) — whatever
//!    status the set was in when the delete arrived, and whatever the create stored.
//!    A repeated delete is accepted and moves nothing.
//! 2. The delete PERSISTS: every other member the create stored is still reported
//!    afterwards, and the detector's own `UpdatedAt` moves on the first delete only.
//! 3. A deleted set is no longer one GuardDuty keeps, so it occupies none of the
//!    detector's slots: "at any given time, you can have only one trusted entity
//!    set", and a delete frees that one. A replayed `ClientToken` whose set was
//!    deleted mints a new set rather than handing back the deleted one.
//! 4. A delete touches only the set it names: another detector's trusted set and the
//!    same detector's threat entity sets are untouched.
//! 5. An id the store does not resolve is refused with HTTP 400 under the
//!    `InvalidInputException` class, naming the MEMBER and never echoing the
//!    caller's value — and a trusted entity set is a sub-resource of a REGIONAL
//!    detector, so only the Region that created it may delete it.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, ENTITY_SET_ACTIVATING, ENTITY_SET_ACTIVE, ENTITY_SET_DELETED,
    ENTITY_SET_DELETE_PENDING, ENTITY_SET_INACTIVE, ENTITY_SET_TRANSITION_MILLIS,
    TRUSTED_ENTITY_SET_ID_MEMBER,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed entity-set id (32 lowercase hex characters, the form this service
/// mints) that no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// A valid Amazon S3 location, the one member every row here keeps fixed.
const LOCATION: &str = "s3://guardduty-lists/trusted.txt";

/// Long enough for the clock to owe the next status, plus slack.
fn past_the_transition() -> std::time::Duration {
    std::time::Duration::from_millis(ENTITY_SET_TRANSITION_MILLIS as u64 + 250)
}

// ---------------------------------------------------------------------------
// The three routes this suite drives, under `/detector/{DetectorId}/trustedentityset`.
// ---------------------------------------------------------------------------

async fn create_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/trustedentityset"),
        Some(body),
    )
    .await
}

async fn delete_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/trustedentityset/{set_id}"),
        None,
    )
    .await
}

async fn get_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/trustedentityset/{set_id}"),
        None,
    )
    .await
}

/// Create a trusted entity set and return its id, failing the test if the create did
/// not succeed.
async fn created(router: &Router, detector_id: &str, case: &str, body: Value) -> String {
    let (status, got) = create_set(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected create to answer 200, got {status} {got:?}"
    );
    got["trustedEntitySetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a trustedEntitySetId, got {got:?}"))
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

/// `GetDetector`, so an assertion can see the detector's own `UpdatedAt`.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

/// The epoch-seconds timestamp `member` carries.
fn timestamp(got: &Value, member: &str, case: &str) -> f64 {
    got[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} timestamp, got {got:?}"))
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
// Rule 1 + 2: DELETE_PENDING now, DELETED when the clock owes it.
// ---------------------------------------------------------------------------

/// One delete case: the create body, and the status the set is in when the delete
/// arrives.
///
/// The rows differ along everything the rule generalises over: the status the set
/// held when the delete arrived (each of the three a created set can be in, the
/// transitional one included), the FORMAT, whether the caller activated the set,
/// whether it carried an expected bucket owner, the number of TAGS and the SIZE of
/// the name (both ends of the modelled `len 1..300`).
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
                "name": "inactive-set",
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
                "name": "activating-set",
                "format": "STIX",
                "location": "https://s3.amazonaws.com/guardduty-lists/trusted.stix",
                "activate": true,
                "tags": {"team": "secops"},
            }),
            settle_first: false,
            status_when_deleted: ENTITY_SET_ACTIVATING,
        },
        DeleteRow {
            case: "an ACTIVE set, settled before the delete",
            create: json!({
                "name": "ACTIVE_set-1",
                "format": "OTX_CSV",
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
async fn a_delete_reports_delete_pending_and_then_deleted() {
    for row in delete_rows() {
        let case = row.case;
        // One set per detector: "at any given time, you can have only one trusted
        // entity set", so each row gets its own store.
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

        // The span is never zero, so a caller reading straight after the delete
        // still observes DELETE_PENDING.
        let pending = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            pending["status"], ENTITY_SET_DELETE_PENDING,
            "case {case:?}: expected {ENTITY_SET_DELETE_PENDING} straight after the delete, got {:?}",
            pending["status"]
        );
        // Rule 2: the delete moved the STATUS and nothing else the create stored.
        for member in ["name", "format", "location", "expectedBucketOwner", "tags"] {
            assert_eq!(
                pending[member], before[member],
                "case {case:?}: the delete must not change {member}, got {:?} after {:?}",
                pending[member], before[member]
            );
        }
        assert_eq!(
            timestamp(&pending, "createdAt", case),
            timestamp(&before, "createdAt", case),
            "case {case:?}: the delete must not move createdAt, got {pending:?}"
        );
        assert!(
            timestamp(&pending, "updatedAt", case) >= timestamp(&before, "updatedAt", case),
            "case {case:?}: the delete changed the set, so updatedAt must not go backwards, got {pending:?}"
        );

        // The detector holds one fewer live sub-resource, so its own UpdatedAt moved.
        let detector_after = read_detector(&router, &detector_id, case).await;
        assert!(
            detector_updated_at(&detector_after, case) > detector_updated_at(&detector_before, case),
            "case {case:?}: the first delete changed the detector, so its updatedAt must move, \
             got {:?} after {:?}",
            detector_after["updatedAt"],
            detector_before["updatedAt"]
        );

        // A repeated delete is accepted and moves nothing: neither the status nor
        // the instant the clock owes it, and not the detector either.
        deleted(&router, &detector_id, &set_id, case).await;
        let still_pending = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            still_pending, pending,
            "case {case:?}: a repeated delete must not restart the transition, got \
             {still_pending:?} after {pending:?}"
        );
        let detector_again = read_detector(&router, &detector_id, case).await;
        assert_eq!(
            detector_updated_at(&detector_again, case),
            detector_updated_at(&detector_after, case),
            "case {case:?}: a repeated delete changed nothing, so the detector's updatedAt must \
             stay put, got {detector_again:?}"
        );

        tokio::time::sleep(past_the_transition()).await;

        let gone = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            gone["status"], ENTITY_SET_DELETED,
            "case {case:?}: expected {ENTITY_SET_DELETED} once the clock owed it, got {:?}",
            gone["status"]
        );
        // DELETED is terminal: nothing moves it again, a repeated delete included,
        // and a read never changes what a later read sees.
        deleted(&router, &detector_id, &set_id, case).await;
        let again = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            again, gone,
            "case {case:?}: {ENTITY_SET_DELETED} is terminal, got {again:?} after {gone:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a delete frees the detector's single slot.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_frees_the_detectors_single_trusted_entity_set_slot() {
    // The rows differ along whether the delete is still pending or has settled when
    // the next create arrives, and along whether the creates carry an idempotency
    // token at all.
    for (case, wait, token) in [
        ("while the delete is still DELETE_PENDING", false, None),
        ("once the set is DELETED", true, None),
        (
            "a replayed clientToken whose set was deleted",
            false,
            Some("replayed-token"),
        ),
        (
            "a replayed clientToken once the set is DELETED",
            true,
            Some("replayed-token"),
        ),
    ] {
        let (router, detector_id) = detector().await;
        let mut body = json!({
            "name": "first-set",
            "format": "TXT",
            "location": LOCATION,
            "activate": true,
        });
        if let Some(token) = token {
            body["clientToken"] = json!(token);
        }
        let first = created(&router, &detector_id, case, body.clone()).await;

        // Before the delete the single slot is taken, so a second create is refused.
        let (status, refused) = create_set(
            &router,
            &detector_id,
            json!({
                "name": "second-set",
                "format": "STIX",
                "location": "s3://guardduty-lists/other.txt",
                "activate": false,
                "clientToken": "a-different-token",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second create to be refused while the slot is taken, \
             got {status} {refused:?}"
        );

        deleted(&router, &detector_id, &first, case).await;
        if wait {
            tokio::time::sleep(past_the_transition()).await;
        }

        // The slot is free again, so the create that was just refused now succeeds —
        // under the SAME token as the deleted set, too: a replayed token must not
        // hand back a set the service is no longer keeping.
        let second = created(&router, &detector_id, case, body).await;
        assert_ne!(
            second, first,
            "case {case:?}: the create after the delete must mint a new set, got {second} again"
        );
        let got = read(&router, &detector_id, &second, case).await;
        assert_eq!(
            got["status"], ENTITY_SET_ACTIVATING,
            "case {case:?}: the new set starts its own transition, got {:?}",
            got["status"]
        );

        // The deleted set is still readable on its own id, in the status the delete
        // put it in.
        let deleted_status = status_of(&router, &detector_id, &first, case).await;
        let expected = if wait {
            ENTITY_SET_DELETED
        } else {
            ENTITY_SET_DELETE_PENDING
        };
        assert_eq!(
            deleted_status, expected,
            "case {case:?}: expected the deleted set to report {expected}, got {deleted_status:?}"
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
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        &format!("/detector/{other_detector}/trustedentityset"),
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
    let theirs = body["trustedEntitySetId"]
        .as_str()
        .expect("a trustedEntitySetId")
        .to_string();

    // A THREAT entity set under the same detector: a different collection, which a
    // trusted delete must not reach into.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{first_detector}/threatentityset"),
        Some(json!({
            "name": "threat-set",
            "format": "TXT",
            "location": "s3://guardduty-lists/threat.txt",
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the threat create to answer 200, got {status} {body:?}"
    );
    let threat = body["threatEntitySetId"]
        .as_str()
        .expect("a threatEntitySetId")
        .to_string();

    deleted(&router, &first_detector, &mine, case).await;

    let (status, other) = call_in(
        &router,
        "eu-west-1",
        "GET",
        &format!("/detector/{other_detector}/trustedentityset/{theirs}"),
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

    let (status, threat_set) = call(
        &router,
        "GET",
        &format!("/detector/{first_detector}/threatentityset/{threat}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the threat set to still read, got {status} {threat_set:?}"
    );
    assert_eq!(
        threat_set["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: a trusted delete must not touch a threat entity set, got {threat_set:?}"
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
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "absent detector",
            ABSENT_DETECTOR_ID.to_string(),
            set_id.clone(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "malformed detector",
            "not-a-detector".to_string(),
            set_id.clone(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "absent set",
            detector_id.clone(),
            ABSENT_SET_ID.to_string(),
            TRUSTED_ENTITY_SET_ID_MEMBER,
        ),
        (
            "malformed set",
            detector_id.clone(),
            "not-a-set".to_string(),
            TRUSTED_ENTITY_SET_ID_MEMBER,
        ),
    ];
    for (case, detector_arg, set_arg, member) in rows {
        let (status, body) = delete_set(&router, &detector_arg, &set_arg).await;
        assert_classified_refusal(
            case,
            status,
            &body,
            member,
            &[detector_arg.as_str(), set_arg.as_str()],
        );
        // The refusal deleted nothing: the set that does exist is still live.
        let status_now = status_of(&router, &detector_id, &set_id, case).await;
        assert_ne!(
            status_now, ENTITY_SET_DELETE_PENDING,
            "case {case:?}: a refused delete must not touch the stored set, got {status_now:?}"
        );
    }

    // A trusted entity set is a sub-resource of a REGIONAL detector, so a request
    // signed for another Region resolves neither the detector nor the set.
    let (status, body) = call_in(
        &router,
        "ap-southeast-2",
        "DELETE",
        &format!("/detector/{detector_id}/trustedentityset/{set_id}"),
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
    assert_eq!(
        survived, ENTITY_SET_ACTIVATING,
        "case \"another Region\": the set must be untouched, got {survived:?}"
    );
}

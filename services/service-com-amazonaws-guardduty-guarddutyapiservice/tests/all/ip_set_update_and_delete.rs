// SPDX-License-Identifier: CC0-1.0

//! The rest of the GuardDuty IPSet (trusted IP list) lifecycle, end to end over the
//! wire: `UpdateIPSet` and `DeleteIPSet`, read back through `GetIPSet`.
//!
//! `ip_set_lifecycle.rs` owns the create-and-read half; this suite owns the half that
//! answered HTTP 501 `NotImplemented` and so left the `create -> ACTIVE -> update ->
//! DELETED` state machine unverifiable. Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `UpdateIPSet` is a PARTIAL update: every member but the two ids is optional and
//!    documented as "the updated …" value
//!    (`smithy_query(operation="UpdateIPSet")`), so a member the request carries
//!    replaces the stored one and a member it omits leaves the stored one exactly
//!    where it was. `Format` is not updatable at all, and `Tags` is not a member of
//!    this request, so neither ever changes.
//! 2. `Activate` on an update starts the same transition `CreateIPSet` starts: the
//!    status becomes the transitional one and only the CLOCK settles it
//!    (`state::IP_SET_TRANSITION_MILLIS`); asking for the status the list already has
//!    moves nothing, and an update that omits `Activate` leaves the status alone.
//! 3. `DeleteIPSet` moves the list to `DELETE_PENDING` and the clock owes it
//!    `DELETED` — `GetIPSet`'s `Status` is `@required` and its enum carries both
//!    (`smithy_query(operation="GetIPSet")`) — whatever status the list was in when
//!    the delete arrived. A repeated delete is accepted and moves nothing, and an
//!    update of a deleted list collides with the delete already running on it, so it
//!    is refused under the `InternalServerErrorException` class with the 400 of a
//!    request the service will not perform, carrying
//!    `state::OPERATION_IN_PROGRESS_MESSAGE`.
//! 4. A deleted list occupies none of the detector's `state::IP_SETS_PER_DETECTOR`
//!    slots, so a delete frees room for a create, and nothing a delete did touches
//!    the detector's other IPSets.
//! 5. An id the store does not resolve is refused with HTTP 400 under the
//!    `InvalidInputException` class, naming the MEMBER and never echoing the caller's
//!    value — and an IPSet is a sub-resource of a REGIONAL detector, so only the
//!    Region that created it may update or delete it.

use serde_json::{json, Value};

use crate::client_rejections::{invalid_parameter, not_owned};
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::ip_set_lifecycle::past_the_transition;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, IP_SETS_PER_DETECTOR, IP_SET_ACTIVATING, IP_SET_ACTIVE,
    IP_SET_DEACTIVATING, IP_SET_DELETED, IP_SET_DELETE_PENDING, IP_SET_INACTIVE,
    OPERATION_IN_PROGRESS_MESSAGE, SET_ID_NOT_FOUND_MESSAGE,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The error class GuardDuty names a change that collides with work already running
/// on the list by.
const IN_PROGRESS_CLASS: &str = "InternalServerErrorException";

/// A well-formed `IpSetId` (32 lowercase hex characters, the form this service
/// mints) that no create ever minted.
const ABSENT_IP_SET_ID: &str = "0123456789abcdef0123456789abcdef";

// ---------------------------------------------------------------------------
// The four routes this suite drives, all under `/detector/{DetectorId}/ipset`.
// ---------------------------------------------------------------------------

async fn create_ip_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/ipset"),
        Some(body),
    )
    .await
}

async fn update_ip_set(
    router: &Router,
    detector_id: &str,
    ip_set_id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/ipset/{ip_set_id}"),
        Some(body),
    )
    .await
}

async fn delete_ip_set(router: &Router, detector_id: &str, ip_set_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/ipset/{ip_set_id}"),
        None,
    )
    .await
}

async fn get_ip_set(router: &Router, detector_id: &str, ip_set_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/ipset/{ip_set_id}"),
        None,
    )
    .await
}

/// Create an IPSet and return its id, failing the test if the call did not succeed.
async fn created(router: &Router, detector_id: &str, case: &str, body: Value) -> String {
    let (status, response) = create_ip_set(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-ip-set expected 200, got {status} {response:?}"
    );
    response["ipSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an ipSetId, got {response:?}"))
        .to_string()
}

/// Read an IPSet back, failing the test if the read did not succeed.
async fn read(router: &Router, detector_id: &str, ip_set_id: &str, case: &str) -> Value {
    let (status, body) = get_ip_set(router, detector_id, ip_set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-ip-set expected 200, got {status} {body:?}"
    );
    body
}

/// The `Status` a read reports.
async fn status_of(router: &Router, detector_id: &str, ip_set_id: &str, case: &str) -> String {
    read(router, detector_id, ip_set_id, case).await["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `GetDetector`, so an assertion can see the detector's own `UpdatedAt`.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-detector expected 200, got {status} {body:?}"
    );
    body
}

/// Update an IPSet, failing the test if the call did not succeed.
async fn updated(router: &Router, detector_id: &str, ip_set_id: &str, case: &str, body: Value) {
    let (status, response) = update_ip_set(router, detector_id, ip_set_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: update-ip-set expected the modelled 200, got {status} {response:?}"
    );
}

/// Assert a refusal is this service's classified HTTP 400, carries `expected_message`
/// and echoes none of `never_echoed`.
fn assert_classified_refusal(
    case: &str,
    status: u16,
    body: &Value,
    expected_message: &str,
    echoed: &[&str],
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
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected message {expected_message:?}, got {message:?}"
    );
    for value in echoed {
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not echo {value:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1: an update replaces what it carries and leaves the rest alone.
// ---------------------------------------------------------------------------

/// The IPSet every row of the partial-update table starts from. `activate` is false
/// so the status is settled and nothing in rule 1 depends on the clock.
fn base_create(case_tag: &str, with_owner: bool) -> Value {
    let mut body = json!({
        "name": format!("base-{case_tag}"),
        "format": "STIX",
        "location": format!("s3://my-bucket/{case_tag}.xml"),
        "activate": false,
        "tags": {"Env": "test", "Team/Owner": "security"}
    });
    if with_owner {
        body["expectedBucketOwner"] = json!("210987654321");
    }
    body
}

/// One partial-update case: whether the create recorded an `ExpectedBucketOwner`, the
/// update body, and the members the read must report afterwards.
///
/// The rows differ along what the rule generalises over: WHICH optional members the
/// request carries (each alone, several together, none at all), the SIZE of the value
/// (both ends of what `Name`/`Location` admit), its ALPHABET (the documented name
/// punctuation, a non-ASCII object key), the FORM of the location (the S3 URL
/// spellings) and whether the member already had a stored value. The alphabet `Name`
/// itself is held to is the `ip_set_name_alphabet` suite's rule, and the S3 URL
/// syntax `Location` is held to the `ip_set_location_syntax` suite's.
struct UpdateRow {
    case: &'static str,
    with_owner: bool,
    update: Value,
}

fn partial_update_rows() -> Vec<UpdateRow> {
    let longest_name = "n".repeat(300);
    let longest_location = format!("s3://bucket/{}", "p".repeat(287));
    vec![
        UpdateRow {
            case: "name alone",
            with_owner: false,
            update: json!({"name": "renamed list"}),
        },
        UpdateRow {
            case: "location alone",
            with_owner: false,
            update: json!({"location": "s3://other-bucket/moved.txt"}),
        },
        UpdateRow {
            case: "expectedBucketOwner recorded by the update, not by the create",
            with_owner: false,
            update: json!({"expectedBucketOwner": "123456789012"}),
        },
        UpdateRow {
            case: "expectedBucketOwner replaced, having been set by the create",
            with_owner: true,
            update: json!({"expectedBucketOwner": "123456789012"}),
        },
        UpdateRow {
            case: "every updatable member at once",
            with_owner: true,
            update: json!({
                "name": "all three",
                "location": "s3://other-bucket/all-three.txt",
                "expectedBucketOwner": "123456789012"
            }),
        },
        UpdateRow {
            case: "the longest name and location the model accepts",
            with_owner: false,
            update: json!({"name": longest_name, "location": longest_location}),
        },
        UpdateRow {
            case: "the shortest name and location the members admit",
            with_owner: true,
            update: json!({"name": "a", "location": "s3://b/k"}),
        },
        UpdateRow {
            case: "a non-ASCII object key round-trips byte for byte",
            with_owner: false,
            update: json!({
                "name": "renamed list 2",
                "location": "https://my-bucket.s3.us-west-2.amazonaws.com/\u{66f4}\u{65b0}.xml"
            }),
        },
        UpdateRow {
            case: "an update that carries no updatable member at all",
            with_owner: true,
            update: json!({}),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_only_the_members_it_carries() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for (index, row) in partial_update_rows().into_iter().enumerate() {
        let case = row.case;
        let create = base_create(&format!("row{index}"), row.with_owner);
        let id = created(&router, &detector, case, create.clone()).await;

        updated(&router, &detector, &id, case, row.update.clone()).await;
        let body = read(&router, &detector, &id, case).await;

        // A member the update carried reports the updated value; one it omitted
        // reports what the create stored.
        for member in ["name", "location", "expectedBucketOwner"] {
            let expected = row
                .update
                .get(member)
                .or_else(|| create.get(member))
                .cloned();
            match expected {
                Some(expected) => assert_eq!(
                    body[member], expected,
                    "case {case:?}: expected {member} {expected:?}, got {:?}",
                    body[member]
                ),
                // `ExpectedBucketOwner` "appears in the response only if it was
                // provided during IPSet creation or update".
                None => assert!(
                    body.get(member).is_none_or(Value::is_null),
                    "case {case:?}: {member} must stay absent when neither the create \
                     nor the update carried it, got {:?}",
                    body[member]
                ),
            }
        }

        // `Format` is not a member of the update request at all, and `Tags` is not
        // either, so neither can have moved.
        assert_eq!(
            body["format"], create["format"],
            "case {case:?}: an update must not change the format, got {:?}",
            body["format"]
        );
        assert_eq!(
            body["tags"], create["tags"],
            "case {case:?}: an update must not change the tags, got {:?}",
            body["tags"]
        );
        // The update carried no `activate`, so the settled status stays put.
        assert_eq!(
            body["status"].as_str().unwrap_or_default(),
            IP_SET_INACTIVE,
            "case {case:?}: an update without activate must leave the status alone, got {:?}",
            body["status"]
        );
    }
}

/// An update is persisted, not merely answered: a second update sees what the first
/// one wrote, and the last write is what a read reports.
#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_ip_set() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let id = created(
        &router,
        &detector,
        "the accumulating IPSet",
        base_create("accumulate", false),
    )
    .await;

    // Each step carries ONE member, so what a read reports afterwards can only come
    // from the steps before it.
    let steps = [
        ("first: name", json!({"name": "step one"}), "step one", "s3://my-bucket/accumulate.xml"),
        (
            "second: location, name untouched",
            json!({"location": "s3://my-bucket/step-two.txt"}),
            "step one",
            "s3://my-bucket/step-two.txt",
        ),
        (
            "third: name again, location untouched",
            json!({"name": "step three"}),
            "step three",
            "s3://my-bucket/step-two.txt",
        ),
    ];

    for (case, update, expected_name, expected_location) in steps {
        updated(&router, &detector, &id, case, update).await;
        let body = read(&router, &detector, &id, case).await;
        let name = body["name"].as_str().unwrap_or_default();
        let location = body["location"].as_str().unwrap_or_default();
        assert_eq!(
            name, expected_name,
            "case {case:?}: expected the name {expected_name:?}, got {name:?}"
        );
        assert_eq!(
            location, expected_location,
            "case {case:?}: expected the location {expected_location:?}, got {location:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: Activate on an update starts a transition only the clock settles.
// ---------------------------------------------------------------------------

/// One activation case: what the create asked for, whether the create's own
/// transition was allowed to settle first, what the update asks for, and the status
/// the read must report before and after the clock owes the settled one.
struct ActivationRow {
    case: &'static str,
    create_activate: bool,
    settle_first: bool,
    update_activate: Option<bool>,
    transient: &'static str,
    settled: &'static str,
}

fn activation_rows() -> Vec<ActivationRow> {
    vec![
        ActivationRow {
            case: "an ACTIVE list asked to stop being used",
            create_activate: true,
            settle_first: true,
            update_activate: Some(false),
            transient: IP_SET_DEACTIVATING,
            settled: IP_SET_INACTIVE,
        },
        ActivationRow {
            case: "an INACTIVE list asked to start being used",
            create_activate: false,
            settle_first: true,
            update_activate: Some(true),
            transient: IP_SET_ACTIVATING,
            settled: IP_SET_ACTIVE,
        },
        ActivationRow {
            case: "an ACTIVE list asked for the status it already has",
            create_activate: true,
            settle_first: true,
            update_activate: Some(true),
            transient: IP_SET_ACTIVE,
            settled: IP_SET_ACTIVE,
        },
        ActivationRow {
            case: "an INACTIVE list asked for the status it already has",
            create_activate: false,
            settle_first: true,
            update_activate: Some(false),
            transient: IP_SET_INACTIVE,
            settled: IP_SET_INACTIVE,
        },
        ActivationRow {
            case: "an update that omits activate leaves a transition running",
            create_activate: true,
            settle_first: false,
            update_activate: None,
            transient: IP_SET_ACTIVATING,
            settled: IP_SET_ACTIVE,
        },
        ActivationRow {
            case: "an update that reverses a transition still running",
            create_activate: true,
            settle_first: false,
            update_activate: Some(false),
            transient: IP_SET_DEACTIVATING,
            settled: IP_SET_INACTIVE,
        },
    ]
}

#[tokio::test]
async fn an_update_moves_the_activation_status_only_through_the_clock() {
    for (index, row) in activation_rows().into_iter().enumerate() {
        let case = row.case;
        // Each row gets its OWN account state: a row that leaves a list in use has
        // consumed the account's activated-list allowance
        // (`state::ACTIVATED_IP_SETS_PER_ACCOUNT`, the
        // `ip_set_activation_allowance` suite's rule), which must not decide what
        // the next row's create is answered.
        let router = new_router();
        let detector = enabled_detector(&router).await;
        let id = created(
            &router,
            &detector,
            case,
            json!({
                "name": format!("activation-{index}"),
                "format": "TXT",
                "location": "s3://my-bucket/activation.txt",
                "activate": row.create_activate
            }),
        )
        .await;

        if row.settle_first {
            tokio::time::sleep(past_the_transition()).await;
        }

        let mut update = json!({});
        if let Some(activate) = row.update_activate {
            update["activate"] = json!(activate);
        }
        updated(&router, &detector, &id, case, update).await;

        // The span is never zero, so a caller reading straight after the update
        // still observes the transitional status.
        let first = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            first, row.transient,
            "case {case:?}: expected {} straight after the update, got {first:?}",
            row.transient
        );
        // A read does not advance the clock: the second read sees what the first saw.
        let second = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            second, row.transient,
            "case {case:?}: a read must not settle the status early, got {second:?}"
        );

        tokio::time::sleep(past_the_transition()).await;

        let after = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            after, row.settled,
            "case {case:?}: expected {} once the clock owes it, got {after:?}",
            row.settled
        );
        let again = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            again, row.settled,
            "case {case:?}: a settled status must stay put, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a delete reports DELETE_PENDING, then DELETED.
// ---------------------------------------------------------------------------

/// The statuses a list can be in when the delete arrives: both settled ones, and a
/// transition still running. The delete's answer must not depend on which.
fn delete_rows() -> Vec<(&'static str, bool, bool)> {
    vec![
        ("an ACTIVE list", true, true),
        ("an INACTIVE list", false, true),
        ("a list still ACTIVATING", true, false),
    ]
}

#[tokio::test]
async fn a_delete_reports_delete_pending_and_then_deleted() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for (index, (case, activate, settle_first)) in delete_rows().into_iter().enumerate() {
        let id = created(
            &router,
            &detector,
            case,
            json!({
                "name": format!("deleted-{index}"),
                "format": "TXT",
                "location": "s3://my-bucket/deleted.txt",
                "activate": activate
            }),
        )
        .await;
        if settle_first {
            tokio::time::sleep(past_the_transition()).await;
        }

        let (status, body) = delete_ip_set(&router, &detector, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete-ip-set expected the modelled 200, got {status} {body:?}"
        );

        // The span is never zero, so a caller reading straight after the delete
        // still observes DELETE_PENDING.
        let pending = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            pending, IP_SET_DELETE_PENDING,
            "case {case:?}: expected {IP_SET_DELETE_PENDING} straight after the delete, \
             got {pending:?}"
        );

        // The record is still reported, and reports what the create stored: a delete
        // moves the status and nothing else.
        let body = read(&router, &detector, &id, case).await;
        let expected_name = format!("deleted-{index}");
        assert_eq!(
            body["name"].as_str().unwrap_or_default(),
            expected_name,
            "case {case:?}: a deleted list must still report the stored name, got {:?}",
            body["name"]
        );

        // A repeated delete is accepted and moves nothing.
        let detector_before = read_detector(&router, &detector, case).await;
        let (status, body) = delete_ip_set(&router, &detector, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated delete must be accepted, got {status} {body:?}"
        );
        let still_pending = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            still_pending, IP_SET_DELETE_PENDING,
            "case {case:?}: a repeated delete must not restart the transition, got \
             {still_pending:?}"
        );
        // The detector's own `UpdatedAt` moves when the set of LIVE sub-resources it
        // holds changes, so the FIRST delete moved it and a repeat does not.
        let detector_after = read_detector(&router, &detector, case).await;
        assert_eq!(
            detector_after["updatedAt"], detector_before["updatedAt"],
            "case {case:?}: a repeated delete must not move the detector's updatedAt, \
             expected {:?}, got {:?}",
            detector_before["updatedAt"], detector_after["updatedAt"]
        );

        tokio::time::sleep(past_the_transition()).await;

        let deleted = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            deleted, IP_SET_DELETED,
            "case {case:?}: expected {IP_SET_DELETED} once the clock owes it, got {deleted:?}"
        );
        // DELETED is terminal: nothing moves it again, a repeated delete included.
        let (status, body) = delete_ip_set(&router, &detector, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a delete of a DELETED list must be accepted, got {status} {body:?}"
        );
        let again = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            again, IP_SET_DELETED,
            "case {case:?}: {IP_SET_DELETED} is terminal, got {again:?}"
        );
    }
}

/// An update of a list whose delete has been accepted collides with the delete
/// already running on it — in both the transitional and the terminal status — and the
/// refusal changes nothing.
#[tokio::test]
async fn an_update_of_a_deleted_ip_set_is_refused() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for (case, wait) in [
        ("while the delete is still DELETE_PENDING", false),
        ("once the list is DELETED", true),
    ] {
        let id = created(
            &router,
            &detector,
            case,
            json!({
                "name": "refused update",
                "format": "TXT",
                "location": "s3://my-bucket/refused.txt",
                "activate": true
            }),
        )
        .await;
        let (status, body) = delete_ip_set(&router, &detector, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must be accepted, got {status} {body:?}"
        );
        if wait {
            tokio::time::sleep(past_the_transition()).await;
        }
        let before = status_of(&router, &detector, &id, case).await;

        let (status, body) = update_ip_set(
            &router,
            &detector,
            &id,
            json!({"name": "resurrected", "activate": true}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        let class = error_code(&body);
        assert_eq!(
            class, IN_PROGRESS_CLASS,
            "case {case:?}: expected the {IN_PROGRESS_CLASS} class, got {class:?} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, OPERATION_IN_PROGRESS_MESSAGE,
            "case {case:?}: expected the in-progress sentence, got {message:?}"
        );
        assert!(
            !message.contains(id.as_str()),
            "case {case:?}: the refusal must not echo {id:?}, got {message:?}"
        );

        // The refusal wrote nothing: neither the name nor the status moved.
        let body = read(&router, &detector, &id, case).await;
        assert_eq!(
            body["name"].as_str().unwrap_or_default(),
            "refused update",
            "case {case:?}: the refused update must not have been applied, got {:?}",
            body["name"]
        );
        let after = body["status"].as_str().unwrap_or_default();
        assert_eq!(
            after, before,
            "case {case:?}: the refused update must not move the status, expected \
             {before:?}, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a deleted list frees its slot and leaves the others alone.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_frees_one_of_the_detectors_slots() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    let mut ids = Vec::new();
    for index in 0..IP_SETS_PER_DETECTOR {
        let case = format!("ip set {index} of {IP_SETS_PER_DETECTOR}");
        ids.push(
            created(
                &router,
                &detector,
                &case,
                json!({
                    "name": format!("slot-{index}"),
                    "format": "TXT",
                    "location": format!("s3://my-bucket/slot-{index}.txt"),
                    "activate": false
                }),
            )
            .await,
        );
    }

    // At the bound, a create is refused.
    let case = format!("one past {IP_SETS_PER_DETECTOR}");
    let (status, body) = create_ip_set(
        &router,
        &detector,
        json!({
            "name": "over-the-limit",
            "format": "TXT",
            "location": "s3://my-bucket/over.txt",
            "activate": false
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 past the maximum, got {status} {body:?}"
    );

    // Deleting one frees its slot: a list being deleted is no longer live.
    let freed = ids.remove(0);
    let (status, body) = delete_ip_set(&router, &detector, &freed).await;
    assert_eq!(
        status, 200,
        "case \"the freeing delete\": expected 200, got {status} {body:?}"
    );
    let replacement = created(
        &router,
        &detector,
        "the create the delete made room for",
        json!({
            "name": "replacement",
            "format": "TXT",
            "location": "s3://my-bucket/replacement.txt",
            "activate": false
        }),
    )
    .await;
    assert!(
        !ids.contains(&replacement) && replacement != freed,
        "case \"the create the delete made room for\": the replacement must be its own \
         IPSet, got {replacement:?}"
    );

    // The deleted list still reports its own status, and every other IPSet is
    // exactly where it was.
    let deleted_status = status_of(&router, &detector, &freed, "the deleted list").await;
    assert_eq!(
        deleted_status, IP_SET_DELETE_PENDING,
        "case \"the deleted list\": expected {IP_SET_DELETE_PENDING}, got {deleted_status:?}"
    );
    for (index, id) in ids.iter().enumerate() {
        let case = format!("ip set {} after the delete", index + 1);
        let body = read(&router, &detector, id, &case).await;
        let expected = format!("slot-{}", index + 1);
        let name = body["name"].as_str().unwrap_or_default();
        assert_eq!(
            name, expected,
            "case {case:?}: a delete must not touch the detector's other IPSets, \
             expected {expected:?}, got {name:?}"
        );
        let status = body["status"].as_str().unwrap_or_default();
        assert_eq!(
            status, IP_SET_INACTIVE,
            "case {case:?}: a delete must not move another IPSet's status, got {status:?}"
        );
    }
}

/// An update of one IPSet is an update of THAT IPSet: its sibling under the same
/// detector reports exactly what its own create stored.
#[tokio::test]
async fn an_update_touches_only_the_ip_set_it_names() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    let first = created(&router, &detector, "the first list", base_create("first", false)).await;
    let second = created(&router, &detector, "the second list", base_create("second", true)).await;

    updated(
        &router,
        &detector,
        &first,
        "the first list",
        json!({"name": "only the first", "location": "s3://my-bucket/only-first.txt"}),
    )
    .await;

    let sibling = read(&router, &detector, &second, "the second list").await;
    assert_eq!(
        sibling["name"].as_str().unwrap_or_default(),
        "base-second",
        "case \"the second list\": the sibling's name must not move, got {:?}",
        sibling["name"]
    );
    assert_eq!(
        sibling["location"].as_str().unwrap_or_default(),
        "s3://my-bucket/second.xml",
        "case \"the second list\": the sibling's location must not move, got {:?}",
        sibling["location"]
    );
}

// ---------------------------------------------------------------------------
// Rule 5: unresolvable ids are classified 400s, and the resource is regional.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_id_is_refused_by_update_and_delete() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let ip_set = created(
        &router,
        &detector,
        "the stored IPSet",
        base_create("refusal", false),
    )
    .await;

    // Each row: the case, the ids the call uses, and the exact message the refusal
    // carries. They span an id that cannot be an id of its kind at all and a
    // well-formed one the store does not hold, on both labels: on the IPSet label the
    // two are one refusal, on the detector label they are not.
    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "a well-formed detectorId no create minted",
            ABSENT_DETECTOR_ID.to_string(),
            ip_set.clone(),
            not_owned(DETECTOR_ID_MEMBER),
        ),
        (
            "a value that cannot be a detectorId at all",
            "not-a-detector".to_string(),
            ip_set.clone(),
            invalid_parameter(DETECTOR_ID_MEMBER),
        ),
        (
            "a well-formed ipSetId this detector does not hold",
            detector.clone(),
            ABSENT_IP_SET_ID.to_string(),
            SET_ID_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a value that cannot be an ipSetId at all",
            detector.clone(),
            "not-an-ip-set".to_string(),
            SET_ID_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "an ipSetId a one-character typo produced",
            detector.clone(),
            format!("{}0", &ip_set[..ip_set.len() - 1]),
            SET_ID_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (case, detector_id, ip_set_id, expected) in rows {
        let echoed = [detector_id.as_str(), ip_set_id.as_str()];

        let update_case = format!("{case}: UpdateIPSet");
        let (status, body) =
            update_ip_set(&router, &detector_id, &ip_set_id, json!({"name": "nope"})).await;
        assert_classified_refusal(&update_case, status, &body, &expected, &echoed);

        let delete_case = format!("{case}: DeleteIPSet");
        let (status, body) = delete_ip_set(&router, &detector_id, &ip_set_id).await;
        assert_classified_refusal(&delete_case, status, &body, &expected, &echoed);
    }

    // None of those refusals touched the stored IPSet.
    let body = read(&router, &detector, &ip_set, "the stored IPSet").await;
    assert_eq!(
        body["name"].as_str().unwrap_or_default(),
        "base-refusal",
        "case \"the stored IPSet\": a refused call must change nothing, got {:?}",
        body["name"]
    );
    assert_eq!(
        body["status"].as_str().unwrap_or_default(),
        IP_SET_INACTIVE,
        "case \"the stored IPSet\": a refused call must not move the status, got {:?}",
        body["status"]
    );
}

#[tokio::test]
async fn another_region_may_neither_update_nor_delete_the_ip_set() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let ip_set = created(
        &router,
        &detector,
        "the us-east-1 IPSet",
        base_create("regional", false),
    )
    .await;

    let path = format!("/detector/{detector}/ipset/{ip_set}");
    for region in ["eu-west-1", "ap-northeast-1", "us-west-2"] {
        let (status, body) = call_in(
            &router,
            region,
            "POST",
            &path,
            Some(json!({"name": "cross-region"})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region:?}: another Region's endpoint must not update the IPSet, got \
             {status} {body:?}"
        );

        let (status, body) = call_in(&router, region, "DELETE", &path, None).await;
        assert_eq!(
            status, 400,
            "case {region:?}: another Region's endpoint must not delete the IPSet, got \
             {status} {body:?}"
        );
    }

    // The owning Region's record is untouched by all of that, and it still serves
    // both calls itself.
    let body = read(&router, &detector, &ip_set, "the owning Region").await;
    assert_eq!(
        body["name"].as_str().unwrap_or_default(),
        "base-regional",
        "case \"the owning Region\": expected the stored name, got {:?}",
        body["name"]
    );
    updated(
        &router,
        &detector,
        &ip_set,
        "the owning Region",
        json!({"name": "updated in place"}),
    )
    .await;
    let (status, body) = delete_ip_set(&router, &detector, &ip_set).await;
    assert_eq!(
        status, 200,
        "case \"the owning Region\": the owning Region must serve the delete, got \
         {status} {body:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `UpdateThreatIntelSet`, end to end over the wire, read back through
//! `GetThreatIntelSet`.
//!
//! `threat_intel_sets.rs` owns the create-and-read half of the threat-list lifecycle
//! and `threat_intel_set_delete.rs` the delete; this suite owns the UPDATE, which
//! answered HTTP 501 `NotImplemented` and so left the `create -> update -> read`
//! half of that state machine unverifiable. Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. `UpdateThreatIntelSet` is a PARTIAL update: every member but the two ids is
//!    optional and documented as "the updated …" value
//!    (`smithy_query(operation="UpdateThreatIntelSet")`), so a member the request
//!    carries replaces the stored one and a member it omits leaves the stored one
//!    exactly where it was. `Format` and `Tags` are not members of this request, so
//!    neither ever changes.
//! 2. `Activate` on an update starts the same transition `CreateThreatIntelSet`
//!    starts: the status becomes the transitional one and only the CLOCK settles it
//!    (`state::THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS`); asking for the status the
//!    list already has moves nothing, and an update that omits `Activate` leaves the
//!    status alone.
//! 3. An update is PERSISTED, not merely answered: a second update sees what the
//!    first one wrote, the last write is what a read reports, and an update touches
//!    only the list its id names.
//! 4. An id the store does not resolve is refused with HTTP 400 under the
//!    `InvalidInputException` class, naming the MEMBER and never echoing the
//!    caller's value, and it updates nothing — a threat list being a sub-resource of
//!    a REGIONAL detector, an id only the Region that created it may update.
//! 5. An id whose own delete is already running is a DIFFERENT refusal: the change
//!    collides with work in flight rather than naming a list that is not there, so it
//!    is refused under the `InternalServerErrorException` class with the 400 of a
//!    request the service will not perform, carrying
//!    `state::OPERATION_IN_PROGRESS_MESSAGE`, and it updates nothing.

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state;

use crate::client_rejections::{invalid_parameter, not_owned};
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::threat_intel_set_delete::{past_activation, past_deletion};

/// The error class every GuardDuty client rejection carries in the body's `__type`
/// discriminator.
const CLASS: &str = "InvalidInputException";

/// The error class GuardDuty names a change that collides with work already running
/// on the list by — the class this service's own `in_progress` refusal carries in the
/// body's `__type` and in `x-amzn-errortype`.
const IN_PROGRESS_CLASS: &str = "InternalServerErrorException";

/// A well-formed `ThreatIntelSetId` (32 lowercase hex characters, the form this
/// service mints) that no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

// ---------------------------------------------------------------------------
// The four routes this suite drives, under `/detector/{DetectorId}/threatintelset`.
// ---------------------------------------------------------------------------

async fn create_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/threatintelset"),
        Some(body),
    )
    .await
}

async fn update_set(
    router: &Router,
    detector_id: &str,
    set_id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/threatintelset/{set_id}"),
        Some(body),
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

/// Create a threat list and return its id, failing the test if the call did not
/// succeed.
async fn created(router: &Router, detector_id: &str, case: &str, body: Value) -> String {
    let (status, response) = create_set(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-threat-intel-set expected 200, got {status} {response:?}"
    );
    response["threatIntelSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a threatIntelSetId, got {response:?}"))
        .to_string()
}

/// Read a threat list back, failing the test if the read did not succeed.
async fn read(router: &Router, detector_id: &str, set_id: &str, case: &str) -> Value {
    let (status, body) = get_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-threat-intel-set expected 200, got {status} {body:?}"
    );
    body
}

/// The `Status` a read reports.
async fn status_of(router: &Router, detector_id: &str, set_id: &str, case: &str) -> String {
    read(router, detector_id, set_id, case).await["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Update a threat list, failing the test if the call did not succeed.
async fn updated(router: &Router, detector_id: &str, set_id: &str, case: &str, body: Value) {
    let (status, response) = update_set(router, detector_id, set_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: update-threat-intel-set expected the modelled 200, got {status} \
         {response:?}"
    );
}

/// Accept a list's deletion, so the next row of a table starts with the detector's
/// live quota (`state::THREAT_INTEL_SETS_PER_DETECTOR`) free again.
async fn retire(router: &Router, detector_id: &str, set_id: &str, case: &str) {
    let (status, body) = delete_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: delete-threat-intel-set expected 200, got {status} {body:?}"
    );
}

/// Assert a refusal is this service's classified HTTP 400, carries `expected_message`
/// and echoes none of `never_echoed`.
fn assert_classified_refusal(
    case: &str,
    status: u16,
    body: &Value,
    expected_message: &str,
    never_echoed: &[&str],
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, CLASS,
        "case {case:?}: expected the {CLASS} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected message {expected_message:?}, got {message:?}"
    );
    for value in never_echoed {
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not echo {value:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1: an update replaces what it carries and leaves the rest alone.
// ---------------------------------------------------------------------------

/// The threat list every row of the partial-update table starts from. `activate` is
/// false so the status is settled and nothing in rule 1 depends on the clock.
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

/// One partial-update case: whether the create recorded an `ExpectedBucketOwner`,
/// and the update body.
///
/// The rows differ along what the rule generalises over: WHICH optional members the
/// request carries (each alone, several together, none at all), the SIZE of the
/// value (both ends of `Name`/`Location`'s modelled `len 1..300`), its ALPHABET
/// (ASCII and non-ASCII) and whether the member already had a stored value.
struct UpdateRow {
    case: &'static str,
    with_owner: bool,
    update: Value,
}

fn partial_update_rows() -> Vec<UpdateRow> {
    // `Name` and `Location` are both `len 1..300`
    // (`smithy_query(operation="UpdateThreatIntelSet")`), so these are the longest
    // values the model accepts.
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
            case: "a non-ASCII name round-trips byte for byte",
            with_owner: false,
            update: json!({
                "name": "\u{30ea}\u{30b9}\u{30c8} \u{2013} \u{66f4}\u{65b0}",
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
                // provided during ThreatIntelSet creation or update".
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
        let status = body["status"].as_str().unwrap_or_default();
        assert_eq!(
            status,
            state::THREAT_INTEL_SET_INACTIVE,
            "case {case:?}: an update without activate must leave the status alone, got \
             {status:?}"
        );

        // Free the detector's live quota for the next row.
        retire(&router, &detector, &id, case).await;
    }
}

/// An update is persisted, not merely answered: a second update sees what the first
/// one wrote, and the last write is what a read reports.
#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_threat_list() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let id = created(
        &router,
        &detector,
        "the accumulating threat list",
        base_create("accumulate", false),
    )
    .await;

    // Each step carries ONE member, so what a read reports afterwards can only come
    // from the steps before it.
    let steps = [
        (
            "first: name",
            json!({"name": "step one"}),
            "step one",
            "s3://my-bucket/accumulate.xml",
        ),
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

/// An update touches only the list its id names: the detector's other threat lists
/// report exactly what their own creates stored.
#[tokio::test]
async fn an_update_leaves_the_detectors_other_threat_lists_alone() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // Three lists, each with its own format and name, so a leak between them is
    // visible in any member.
    let formats = ["TXT", "STIX", "PROOF_POINT"];
    let mut ids = Vec::new();
    for (index, format) in formats.iter().enumerate() {
        let body = json!({
            "name": format!("sibling-{index}"),
            "format": format,
            "location": format!("s3://my-bucket/sibling-{index}.txt"),
            "activate": false
        });
        ids.push(created(&router, &detector, "the sibling lists", body).await);
    }

    // Update the middle one only.
    let target = ids[1].clone();
    updated(
        &router,
        &detector,
        &target,
        "the middle list",
        json!({"name": "only this one", "location": "s3://other/only-this-one.txt"}),
    )
    .await;

    for (index, id) in ids.iter().enumerate() {
        let case = format!("sibling {index}");
        let body = read(&router, &detector, id, &case).await;
        let name = body["name"].as_str().unwrap_or_default();
        let expected_name = if *id == target {
            "only this one".to_string()
        } else {
            format!("sibling-{index}")
        };
        assert_eq!(
            name, expected_name,
            "case {case:?}: expected the name {expected_name:?}, got {name:?}"
        );
        let format = body["format"].as_str().unwrap_or_default();
        assert_eq!(
            format, formats[index],
            "case {case:?}: expected the format {:?}, got {format:?}",
            formats[index]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: Activate on an update starts a transition only the clock settles.
// ---------------------------------------------------------------------------

/// One activation case: what the create asked for, whether the create's own
/// transition was allowed to settle first, what the update asks for, and the status
/// a read must report before and after the clock owes the settled one.
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
            transient: state::THREAT_INTEL_SET_DEACTIVATING,
            settled: state::THREAT_INTEL_SET_INACTIVE,
        },
        ActivationRow {
            case: "an INACTIVE list asked to start being used",
            create_activate: false,
            settle_first: true,
            update_activate: Some(true),
            transient: state::THREAT_INTEL_SET_ACTIVATING,
            settled: state::THREAT_INTEL_SET_ACTIVE,
        },
        ActivationRow {
            case: "an ACTIVE list asked for the status it already has",
            create_activate: true,
            settle_first: true,
            update_activate: Some(true),
            transient: state::THREAT_INTEL_SET_ACTIVE,
            settled: state::THREAT_INTEL_SET_ACTIVE,
        },
        ActivationRow {
            case: "an INACTIVE list asked for the status it already has",
            create_activate: false,
            settle_first: true,
            update_activate: Some(false),
            transient: state::THREAT_INTEL_SET_INACTIVE,
            settled: state::THREAT_INTEL_SET_INACTIVE,
        },
        ActivationRow {
            case: "an update that omits activate leaves a transition running",
            create_activate: true,
            settle_first: false,
            update_activate: None,
            transient: state::THREAT_INTEL_SET_ACTIVATING,
            settled: state::THREAT_INTEL_SET_ACTIVE,
        },
        ActivationRow {
            case: "an update that reverses a transition still running",
            create_activate: true,
            settle_first: false,
            update_activate: Some(false),
            transient: state::THREAT_INTEL_SET_DEACTIVATING,
            settled: state::THREAT_INTEL_SET_INACTIVE,
        },
    ]
}

#[tokio::test]
async fn an_update_moves_the_activation_status_only_through_the_clock() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for (index, row) in activation_rows().into_iter().enumerate() {
        let case = row.case;
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
            tokio::time::sleep(past_activation()).await;
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

        tokio::time::sleep(past_activation()).await;

        let after = status_of(&router, &detector, &id, case).await;
        assert_eq!(
            after, row.settled,
            "case {case:?}: expected {} once the clock owes it, got {after:?}",
            row.settled
        );

        // Free the detector's live quota for the next row.
        retire(&router, &detector, &id, case).await;
    }
}

/// Every status an update can leave a list in is one of the modelled
/// `ThreatIntelSetStatus` members, and `Status` is `@required` on the read
/// (`smithy_query(operation="GetThreatIntelSet")`).
#[tokio::test]
async fn every_status_an_update_reports_is_modelled() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let id = created(
        &router,
        &detector,
        "the status walk",
        base_create("status-walk", false),
    )
    .await;

    let mut seen = Vec::new();
    for activate in [true, false, true] {
        updated(
            &router,
            &detector,
            &id,
            "the status walk",
            json!({"activate": activate}),
        )
        .await;
        seen.push(status_of(&router, &detector, &id, "the status walk").await);
        tokio::time::sleep(past_activation()).await;
        seen.push(status_of(&router, &detector, &id, "the status walk").await);
    }

    for status in &seen {
        assert!(
            service_com_amazonaws_guardduty_guarddutyapiservice::ops::types::THREAT_INTEL_SET_STATUS_VALUES
                .contains(&status.as_str()),
            "case {status:?}: every status reported must be a modelled ThreatIntelSetStatus, \
             got {status:?}"
        );
    }
    // The walk really moved: it saw both transitional statuses and both settled
    // ones.
    for expected in [
        state::THREAT_INTEL_SET_ACTIVATING,
        state::THREAT_INTEL_SET_ACTIVE,
        state::THREAT_INTEL_SET_DEACTIVATING,
        state::THREAT_INTEL_SET_INACTIVE,
    ] {
        assert!(
            seen.iter().any(|status| status == expected),
            "case {expected:?}: the activation walk must report it, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an id the store does not resolve is refused, and updates nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_id_is_refused_and_updates_nothing() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let id = created(
        &router,
        &detector,
        "the untouched list",
        base_create("refusals", false),
    )
    .await;
    let before = read(&router, &detector, &id, "the untouched list").await;

    let rename = json!({"name": "must not land"});

    // A `DetectorId` no create minted: the refusal names the detector member.
    let (status, body) = update_set(&router, ABSENT_DETECTOR_ID, &id, rename.clone()).await;
    assert_classified_refusal(
        "a detectorId no create minted",
        status,
        &body,
        &not_owned(state::DETECTOR_ID_MEMBER),
        &[ABSENT_DETECTOR_ID],
    );

    // A `DetectorId` that is not an id of that form at all.
    let malformed_detector = "not-a-detector";
    let (status, body) = update_set(&router, malformed_detector, &id, rename.clone()).await;
    assert_classified_refusal(
        "a malformed detectorId",
        status,
        &body,
        &invalid_parameter(state::DETECTOR_ID_MEMBER),
        &[malformed_detector],
    );

    // A well-formed `ThreatIntelSetId` this detector does not hold.
    let (status, body) = update_set(&router, &detector, ABSENT_SET_ID, rename.clone()).await;
    assert_classified_refusal(
        "a threatIntelSetId no create minted",
        status,
        &body,
        state::SET_ID_NOT_FOUND_MESSAGE,
        &[ABSENT_SET_ID],
    );

    // A `ThreatIntelSetId` that is not an id of that form at all: the list is absent
    // either way, so it is the same refusal.
    let malformed_set = "not-a-threat-list";
    let (status, body) = update_set(&router, &detector, malformed_set, rename.clone()).await;
    assert_classified_refusal(
        "a malformed threatIntelSetId",
        status,
        &body,
        state::SET_ID_NOT_FOUND_MESSAGE,
        &[malformed_set],
    );

    // A threat list is a sub-resource of a REGIONAL detector, so the endpoint of
    // another Region resolves neither.
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        &format!("/detector/{detector}/threatintelset/{id}"),
        Some(rename.clone()),
    )
    .await;
    assert_classified_refusal(
        "another Region's endpoint",
        status,
        &body,
        &not_owned(state::DETECTOR_ID_MEMBER),
        &[detector.as_str()],
    );

    // None of the refusals wrote anything.
    let after = read(&router, &detector, &id, "the untouched list").await;
    assert_eq!(
        after, before,
        "a refused update must change nothing: expected {before:?}, got {after:?}"
    );
}

/// A list whose delete has been accepted is one the account's own delete is still
/// running on, so an update of it collides with that work: HTTP 400 under the
/// `InternalServerErrorException` class with the fixed in-progress sentence — and
/// the record stays on its way out rather than being walked back into use.
///
/// The rows are both ends of the deletion's own transition, and both values of
/// `Activate`, so the refusal is not a function of what the update asks for.
#[tokio::test]
async fn an_update_of_a_deleted_list_is_refused() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // Both ends of the deletion's own transition: the record the clock still owes
    // `DELETED`, and the one it has already settled.
    for (case, wait) in [
        ("while the deletion is still pending", false),
        ("once the deletion has settled", true),
    ] {
        for activate in [true, false] {
            let case = &format!("{case}, activate={activate}");
            let id = created(&router, &detector, case, base_create("deleted", false)).await;
            retire(&router, &detector, &id, case).await;
            if wait {
                tokio::time::sleep(past_deletion()).await;
            }
            let expected = status_of(&router, &detector, &id, case).await;

            let (status, body) =
                update_set(&router, &detector, &id, json!({"activate": activate})).await;
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
                message,
                state::OPERATION_IN_PROGRESS_MESSAGE,
                "case {case:?}: expected the in-progress sentence, got {message:?}"
            );
            assert!(
                !message.contains(id.as_str()),
                "case {case:?}: the refusal must not echo {id:?}, got {message:?}"
            );

            let after = status_of(&router, &detector, &id, case).await;
            assert_eq!(
                after, expected,
                "case {case:?}: a refused update must not move a deleted list's status, got \
                 {after:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for GuardDuty's ThreatIntelSet create/read loop.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `CreateThreatIntelSet` persists every member the request carried under a freshly
//!    minted id, and `GetThreatIntelSet` reports exactly that — for any `Format` the
//!    model declares and any `Location` the model admits.
//! 2. `Activate` decides the status: a list the create activates reports the transient
//!    `ACTIVATING` until the activation it started comes due and `ACTIVE` afterwards,
//!    while a list nobody activated reports `INACTIVE` and stays there. Reading never
//!    moves a status the clock does not owe.
//! 3. `ClientToken` is an `@idempotencyToken`: a replayed create returns the list the
//!    first call minted and mints nothing new; a fresh token mints a new list.
//! 4. A list belongs to the detector it was created under: an id that detector does
//!    not hold is refused with HTTP 400 under the `InvalidInputException` class,
//!    classified from the REQUEST, never answered with a hollow 200.
//! 5. A detector holds at most the published quota of threat lists; a create past it
//!    is refused and stores nothing, while a replayed token consumes none of it.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state;

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modeled error SHAPE that class is enveloped in, which restJson1 resolves the
/// error shape from (the `x-amzn-errortype` header).
const ENVELOPE: &str = "BadRequestException";

/// Every member of `ThreatIntelSetFormat`, in model order
/// (`smithy_query(shape="ThreatIntelSetFormat")`). A create takes one of these six
/// and a read reports back the one it stored — including the five no recorded call
/// cited.
const FORMATS: [&str; 6] = [
    "TXT",
    "STIX",
    "OTX_CSV",
    "ALIEN_VAULT",
    "PROOF_POINT",
    "FIRE_EYE",
];

/// `ThreatIntelSetStatus` members this slice can reach
/// (`smithy_query(shape="ThreatIntelSetStatus")`).
const INACTIVE: &str = "INACTIVE";
const ACTIVATING: &str = "ACTIVATING";
const ACTIVE: &str = "ACTIVE";

/// The published bounds of `Location` (`smithy_query(shape="Location")`: len 1..300).
const LOCATION_MIN: usize = 1;
const LOCATION_MAX: usize = 300;

/// A margin on top of the activation span, for the wall clock the handler reads.
const SETTLE_MARGIN_MILLIS: u64 = 150;

/// Long enough that an activated list's `ACTIVATING` -> `ACTIVE` transition is due.
fn past_activation() -> Duration {
    let span = u64::try_from(state::THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS).unwrap_or_default();
    Duration::from_millis(span + SETTLE_MARGIN_MILLIS)
}

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

/// Create a list and return its minted id, failing the test if the create did not
/// succeed.
async fn created_set_id(router: &Router, detector_id: &str, body: &Value) -> String {
    let (status, created) = create_set(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {body}: create-threat-intel-set expected 200, got {status} {created:?}"
    );
    let id = created["threatIntelSetId"].as_str().unwrap_or_else(|| {
        panic!("case {body}: expected a threatIntelSetId, got {created:?}")
    });
    assert_eq!(
        id.len(),
        32,
        "case {body}: expected a 32-character ThreatIntelSetId, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit()),
        "case {body}: expected a hexadecimal ThreatIntelSetId, got {id:?}"
    );
    id.to_string()
}

/// A `Location` of exactly `len` characters, built on an S3 URI prefix so the value
/// stays a plausible object URI at every size.
fn location_of(len: usize) -> String {
    let prefix = "s3://threat-lists/";
    if len <= prefix.len() {
        return "s".repeat(len);
    }
    format!("{prefix}{}", "a".repeat(len - prefix.len()))
}

// ---------------------------------------------------------------------------
// Rule 1: a read reports exactly what the create stored.
// ---------------------------------------------------------------------------

/// The rows differ along the members the rule generalises over: the `Format` (every
/// one the model declares), the `Location` (ASCII URI, non-ASCII text, and both
/// published length bounds), the presence and content of `Tags`, and the presence of
/// `ExpectedBucketOwner`.
fn round_trip_requests() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = FORMATS
        .iter()
        .map(|format| {
            (
                format!("format {format}"),
                json!({
                    "name": format!("list-for-{format}"),
                    "format": format,
                    "location": format!("s3://threat-lists/{}.txt", format.to_lowercase()),
                    "activate": false,
                }),
            )
        })
        .collect();

    rows.push((
        "a non-ASCII location and name".to_string(),
        json!({
            // `Location` is a plain string (len 1..300), so a URI carrying non-ASCII
            // characters is a legal value and must round-trip byte for byte.
            "name": "liste-de-menaces-日本語-Ω",
            "format": "TXT",
            "location": "s3://threat-lists/menaces-日本語-Ω/ips.txt",
            "activate": false,
        }),
    ));
    rows.push((
        format!("a location at the published minimum of {LOCATION_MIN}"),
        json!({
            "name": "n",
            "format": "STIX",
            "location": location_of(LOCATION_MIN),
            "activate": false,
        }),
    ));
    rows.push((
        format!("a location at the published maximum of {LOCATION_MAX}"),
        json!({
            "name": "a".repeat(300),
            "format": "FIRE_EYE",
            "location": location_of(LOCATION_MAX),
            "activate": false,
        }),
    ));
    rows.push((
        "tags, including one with an empty value".to_string(),
        json!({
            "name": "tagged-list",
            "format": "OTX_CSV",
            "location": "s3://threat-lists/tagged.csv",
            "activate": false,
            // `TagValue` is len 0..256 (`smithy_query(shape="TagMap")`), so an empty
            // value is a legal tag.
            "tags": {"team": "security", "env": ""},
        }),
    ));
    rows.push((
        "an expected bucket owner".to_string(),
        json!({
            "name": "owned-bucket-list",
            "format": "PROOF_POINT",
            "location": "s3://someone-elses-bucket/ips.txt",
            "activate": false,
            "expectedBucketOwner": "123456789012",
        }),
    ));
    rows.push((
        "an activated list".to_string(),
        json!({
            "name": "activated-list",
            "format": "TXT",
            "location": "s3://threat-lists/activated.txt",
            "activate": true,
        }),
    ));
    rows
}

#[tokio::test]
async fn a_read_reports_every_member_the_create_stored() {
    // One detector holds at most `THREAT_INTEL_SETS_PER_DETECTOR` lists, so the rows
    // are run in chunks of that size, each against its own emulator.
    for chunk in round_trip_requests().chunks(state::THREAT_INTEL_SETS_PER_DETECTOR) {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let mut seen_ids: Vec<String> = Vec::new();

        for (case, request) in chunk {
            let set_id = created_set_id(&router, &detector_id, request).await;
            assert!(
                !seen_ids.contains(&set_id),
                "case {case:?}: every create must mint a fresh id, got {set_id:?} again"
            );
            seen_ids.push(set_id.clone());

            let (status, got) = get_set(&router, &detector_id, &set_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: get-threat-intel-set expected 200, got {status} {got:?}"
            );
            for member in ["name", "format", "location", "expectedBucketOwner", "tags"] {
                let expected = request.get(member).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    got[member], expected,
                    "case {case:?}: member {member} must read back what the create stored, \
                     got {:?}",
                    got[member]
                );
            }
            // `Activate` is the only input that is not echoed: it decides the status.
            let expected_status = if request["activate"] == json!(true) {
                ACTIVATING
            } else {
                INACTIVE
            };
            assert_eq!(
                got["status"].as_str(),
                Some(expected_status),
                "case {case:?}: expected status {expected_status}, got {:?}",
                got["status"]
            );
        }

        // Every list is still readable after all the others were created: a create
        // adds to the detector's lists rather than replacing them.
        for (position, set_id) in seen_ids.iter().enumerate() {
            let (status, got) = get_set(&router, &detector_id, set_id).await;
            assert_eq!(
                status, 200,
                "case \"list {position} after every create\": expected 200, got {status} {got:?}"
            );
        }
    }
}

/// An untagged list reports NO `tags` member: `TagMap` is modelled len 1..200
/// (`smithy_query(shape="TagMap")`), so an empty map is not a value of that member —
/// which is also why a create that sends one is refused at the modelled boundary.
#[tokio::test]
async fn an_untagged_list_reports_no_tag_map() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for (case, request) in [
        (
            "no tags member at all",
            json!({"name": "untagged", "format": "TXT", "location": "s3://b/k", "activate": false}),
        ),
        (
            "a single tag, then read back",
            json!({"name": "tagged", "format": "TXT", "location": "s3://b/k2", "activate": false, "tags": {"team": "security"}}),
        ),
    ] {
        let expected = request.get("tags").cloned().unwrap_or(Value::Null);
        let set_id = created_set_id(&router, &detector_id, &request).await;
        let (_, got) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            got.get("tags").cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: expected the stored tag map, got {:?}",
            got["tags"]
        );
    }

    // One character below the published minimum of the map: zero entries.
    let (status, body) = create_set(
        &router,
        &detector_id,
        &json!({
            "name": "empty-tag-map",
            "format": "TXT",
            "location": "s3://b/k3",
            "activate": false,
            "tags": {},
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an empty tag map\": expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        body.get("threatIntelSetId").is_none(),
        "case \"an empty tag map\": a refused create must mint nothing, got {body:?}"
    );
}

/// A `Location` one character past the published maximum is refused at the modelled
/// boundary, and the refusal mints nothing.
#[tokio::test]
async fn a_location_past_the_published_maximum_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let case = format!("a location of {} characters", LOCATION_MAX + 1);
    let (status, body) = create_set(
        &router,
        &detector_id,
        &json!({
            "name": "too-long-location",
            "format": "TXT",
            "location": location_of(LOCATION_MAX + 1),
            "activate": false,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        body.get("threatIntelSetId").is_none(),
        "case {case:?}: a refused create must mint nothing, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: `Activate` decides the status, and the clock settles the activation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activate_decides_the_status_and_the_clock_settles_an_activation() {
    // The rows are the two values of `Activate`: one starts a transition the clock
    // owes, the other reaches its terminal status straight away.
    for (case, activate, immediately, once_due) in [
        ("activate: true", true, ACTIVATING, ACTIVE),
        ("activate: false", false, INACTIVE, INACTIVE),
    ] {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let set_id = created_set_id(
            &router,
            &detector_id,
            &json!({
                "name": "status-list",
                "format": "TXT",
                "location": "s3://threat-lists/status.txt",
                "activate": activate,
            }),
        )
        .await;

        // A caller reading straight after the create sees the status the create left.
        let (_, got) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            got["status"].as_str(),
            Some(immediately),
            "case {case:?}: a read straight after the create must report {immediately}, \
             got {:?}",
            got["status"]
        );
        // And reading does not settle the transition early: the status is the same
        // until the due instant passes.
        let (_, again) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            again["status"], got["status"],
            "case {case:?}: reading must not move the status, got {:?} then {:?}",
            got["status"], again["status"]
        );

        sleep(past_activation());

        let (_, settled) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            settled["status"].as_str(),
            Some(once_due),
            "case {case:?}: once the transition is due the status must be {once_due}, \
             got {:?}",
            settled["status"]
        );
        // A settled status is persisted, not recomputed per call, and the transition
        // changes nothing else about the list.
        let (_, stable) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            stable, settled,
            "case {case:?}: a settled list must read the same twice, got {settled:?} then \
             {stable:?}"
        );
        assert_eq!(
            settled["location"].as_str(),
            Some("s3://threat-lists/status.txt"),
            "case {case:?}: settling must not touch the stored members, got {settled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: `ClientToken` is an idempotency token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_returns_the_list_the_first_create_minted() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let first = json!({
        "name": "idempotent-list",
        "format": "TXT",
        "location": "s3://threat-lists/idempotent.txt",
        "activate": false,
        "clientToken": "token-one",
    });
    let first_id = created_set_id(&router, &detector_id, &first).await;

    // The rows differ along what the REPLAY carries beside the same token: the
    // identical body, and bodies whose other members differ. Every one of them is
    // the same request as far as the token is concerned.
    for (case, replay) in [
        ("the identical body", first.clone()),
        (
            "a different name",
            json!({
                "name": "renamed-on-replay",
                "format": "TXT",
                "location": "s3://threat-lists/idempotent.txt",
                "activate": false,
                "clientToken": "token-one",
            }),
        ),
        (
            "a different location, format and activate",
            json!({
                "name": "idempotent-list",
                "format": "STIX",
                "location": "s3://threat-lists/elsewhere.txt",
                "activate": true,
                "clientToken": "token-one",
            }),
        ),
    ] {
        let (status, body) = create_set(&router, &detector_id, &replay).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed create must succeed, got {status} {body:?}"
        );
        assert_eq!(
            body["threatIntelSetId"].as_str(),
            Some(first_id.as_str()),
            "case {case:?}: a replayed token must return the first list, got {body:?}"
        );
        // The replay minted nothing, so the stored list is still the first call's.
        let (_, got) = get_set(&router, &detector_id, &first_id).await;
        assert_eq!(
            got["name"].as_str(),
            Some("idempotent-list"),
            "case {case:?}: a replay must not rewrite the stored list, got {got:?}"
        );
        assert_eq!(
            got["status"].as_str(),
            Some(INACTIVE),
            "case {case:?}: a replay must not start an activation, got {got:?}"
        );
    }

    // A FRESH token is a different request: it mints a second list.
    let second_id = created_set_id(
        &router,
        &detector_id,
        &json!({
            "name": "second-list",
            "format": "TXT",
            "location": "s3://threat-lists/second.txt",
            "activate": false,
            "clientToken": "token-two",
        }),
    )
    .await;
    assert_ne!(
        second_id, first_id,
        "case \"a fresh token\": must mint a new list, got {second_id:?}"
    );

    // And so is no token at all: two untokened creates of the same body are two
    // lists.
    let untokened = json!({
        "name": "untokened-list",
        "format": "TXT",
        "location": "s3://threat-lists/untokened.txt",
        "activate": false,
    });
    let third_id = created_set_id(&router, &detector_id, &untokened).await;
    let fourth_id = created_set_id(&router, &detector_id, &untokened).await;
    assert_ne!(
        third_id, fourth_id,
        "case \"no client token\": each create must mint its own list, got {third_id:?} twice"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a list belongs to the detector it was created under.
// ---------------------------------------------------------------------------

/// Assert one rejection: the status, the error class in all three places it appears,
/// and the classified message.
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

#[tokio::test]
async fn a_threat_intel_set_id_the_detector_does_not_hold_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    // One stored list, so the classification depends on the input and not on whether
    // the detector holds anything at all.
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

    // Values that cannot be an id this service mints: the rows differ along LENGTH
    // (both sides of the 32-character boundary) and along the ALPHABET.
    for (id, why) in [
        ("deadbeefdeadbeefdeadbeefdeadbee", "31 characters"),
        ("deadbeefdeadbeefdeadbeefdeadbeef0", "33 characters"),
        ("deadbeefdeadbeefdeadbeefdeadbeeg", "32 characters, 'g' is not hex"),
        ("not-a-threat-intel-set-id", "not hexadecimal at all"),
        ("0", "a single character"),
    ] {
        let (status, body) = get_set(&router, &detector_id, id).await;
        assert_rejection(why, status, &body, &invalid_parameter("threatIntelSetId"));
        assert!(
            !body["message"].as_str().unwrap_or_default().contains(id),
            "case {why:?}: a rejection must not echo the caller's value, got {body:?}"
        );
    }

    // Well-formed ids no create minted here, including the id of a list the detector
    // does hold, upper-cased: case is not significant to the FORM, so it is the
    // ownership failure and not a parameter one.
    for (id, why) in [
        ("deadbeefdeadbeefdeadbeefdeadbeef".to_string(), "32 lowercase hex characters"),
        ("00000000000000000000000000000000".to_string(), "32 zeros"),
        (stored.to_uppercase(), "a stored id, upper-cased"),
    ] {
        let (status, body) = get_set(&router, &detector_id, &id).await;
        assert_rejection(why, status, &body, &not_owned("threatIntelSetId"));
    }
}

#[tokio::test]
async fn a_create_or_read_against_a_detector_this_region_does_not_hold_is_refused() {
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
    let home_detector = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector: expected a detectorId, got {created:?}"))
        .to_string();

    let request = json!({
        "name": "regional-list",
        "format": "TXT",
        "location": "s3://threat-lists/regional.txt",
        "activate": false,
    });
    let (status, created) = call_in(
        &router,
        home,
        "POST",
        &format!("/detector/{home_detector}/threatintelset"),
        Some(request.clone()),
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

    // A detector another Region's endpoint created is not owned here, so neither the
    // create nor the read resolves.
    for (case, method, path, body) in [
        (
            "create against a detector of another Region",
            "POST",
            format!("/detector/{home_detector}/threatintelset"),
            Some(request.clone()),
        ),
        (
            "read a list of another Region",
            "GET",
            format!("/detector/{home_detector}/threatintelset/{set_id}"),
            None,
        ),
    ] {
        let (status, refused) = call_in(&router, elsewhere, method, &path, body).await;
        assert_rejection(case, status, &refused, &not_owned("detectorId"));
    }

    // A detector id no account owns is refused the same way, and a value that is not
    // a detector id at all is the parameter failure.
    let (status, refused) = call_in(
        &router,
        home,
        "POST",
        &format!("/detector/{ABSENT_DETECTOR_ID}/threatintelset"),
        Some(request.clone()),
    )
    .await;
    assert_rejection(
        "create against an absent detector",
        status,
        &refused,
        &not_owned("detectorId"),
    );
    let (status, refused) = call_in(
        &router,
        home,
        "POST",
        "/detector/not-a-detector-id/threatintelset",
        Some(request),
    )
    .await;
    assert_rejection(
        "create against a malformed detector id",
        status,
        &refused,
        &invalid_parameter("detectorId"),
    );

    // The list is still there, read through its own Region's endpoint: a refused
    // call changed nothing.
    let (status, got) = call_in(
        &router,
        home,
        "GET",
        &format!("/detector/{home_detector}/threatintelset/{set_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"read through its own Region\": expected 200, got {status} {got:?}"
    );
    assert_eq!(
        got["location"].as_str(),
        Some("s3://threat-lists/regional.txt"),
        "case \"read through its own Region\": got {got:?}"
    );
}

/// A list is scoped to its detector: deleting the detector takes its threat lists
/// with it, so the id stops resolving.
#[tokio::test]
async fn deleting_the_detector_drops_its_threat_lists() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let set_id = created_set_id(
        &router,
        &detector_id,
        &json!({
            "name": "cascade-list",
            "format": "TXT",
            "location": "s3://threat-lists/cascade.txt",
            "activate": false,
        }),
    )
    .await;

    let (status, body) = call(&router, "DELETE", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case \"delete-detector\": expected 200, got {status} {body:?}"
    );

    let (status, refused) = get_set(&router, &detector_id, &set_id).await;
    assert_rejection(
        "read a list of a deleted detector",
        status,
        &refused,
        &not_owned("detectorId"),
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the published quota of threat lists per account.
// ---------------------------------------------------------------------------

/// Filling the quota is accepted, the next create is refused, and the refusal
/// changes nothing: the stored lists are all still there and all still readable.
#[tokio::test]
async fn a_create_past_the_published_quota_of_threat_lists_is_refused() {
    let quota = state::THREAT_INTEL_SETS_PER_DETECTOR;
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let mut ids: Vec<String> = Vec::new();
    for position in 0..quota {
        let request = json!({
            "name": format!("list-{position}"),
            "format": FORMATS[position % FORMATS.len()],
            "location": format!("s3://threat-lists/{position}.txt"),
            "activate": false,
            "clientToken": format!("token-{position}"),
        });
        let id = created_set_id(&router, &detector_id, &request).await;
        assert!(
            !ids.contains(&id),
            "case \"list {position} of the quota\": every create must mint a fresh id, got \
             {id:?} again"
        );
        ids.push(id);
    }

    // The rows differ along what the over-quota create carries: a fresh token, no
    // token at all, and a body that differs in every other member. None of them is
    // accepted.
    for (case, request) in [
        (
            "a fresh client token",
            json!({"name": "over-quota", "format": "TXT", "location": "s3://threat-lists/over.txt", "activate": false, "clientToken": "token-past-the-quota"}),
        ),
        (
            "no client token",
            json!({"name": "over-quota-2", "format": "STIX", "location": "s3://threat-lists/over2.txt", "activate": true}),
        ),
    ] {
        let (status, refused) = create_set(&router, &detector_id, &request).await;
        assert_rejection(
            case,
            status,
            &refused,
            &format!(
                "The request is rejected because the account has reached the limit of {quota} \
                 threat lists."
            ),
        );
        assert!(
            refused.get("threatIntelSetId").is_none(),
            "case {case:?}: a refused create must mint nothing, got {refused:?}"
        );
    }

    // A replayed token consumes none of the quota: it is still answered with the
    // list the first create minted, even with the quota full.
    let (status, replayed) = create_set(
        &router,
        &detector_id,
        &json!({
            "name": "list-0",
            "format": FORMATS[0],
            "location": "s3://threat-lists/0.txt",
            "activate": false,
            "clientToken": "token-0",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a replayed token at the quota\": expected 200, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed["threatIntelSetId"].as_str(),
        ids.first().map(String::as_str),
        "case \"a replayed token at the quota\": expected the first list, got {replayed:?}"
    );

    // Every stored list survived the refusals, with the members its create stored.
    for (position, id) in ids.iter().enumerate() {
        let (status, got) = get_set(&router, &detector_id, id).await;
        assert_eq!(
            status, 200,
            "case \"list {position} after the refusals\": expected 200, got {status} {got:?}"
        );
        assert_eq!(
            got["name"].as_str(),
            Some(format!("list-{position}").as_str()),
            "case \"list {position} after the refusals\": got {got:?}"
        );
    }
}

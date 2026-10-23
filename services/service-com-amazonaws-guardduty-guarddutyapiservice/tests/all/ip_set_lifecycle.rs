// SPDX-License-Identifier: CC0-1.0

//! GuardDuty IPSets (trusted IP lists), end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `CreateIPSet` PERSISTS the IPSet under the request's detector, and `GetIPSet`
//!    reports every member back as it was stored — the name verbatim, the format,
//!    the location, the tags and `ExpectedBucketOwner`, which appears "only if it
//!    was provided during IPSet creation or update".
//! 2. `Activate` decides the status: a list the caller asked GuardDuty to start
//!    using is transitional (`ACTIVATING`) until the CLOCK owes it `ACTIVE`
//!    (`state::IP_SET_TRANSITION_MILLIS`), one it did not is stored unused
//!    (`INACTIVE`). A read never moves the status early and never changes what a
//!    later read sees.
//! 3. `ClientToken` is `@idempotencyToken`: the same token returns the IPSet the
//!    first call minted, a different token (or none) mints another.
//! 4. An id the store does not resolve is refused with HTTP 400 under the
//!    `InvalidInputException` class — a value that cannot be an id at all as a
//!    parameter failure, a well-formed one the account/detector does not hold as an
//!    ownership failure — and the refusal names the MEMBER, never the caller's
//!    value. A create past `state::IP_SETS_PER_DETECTOR` is refused the same way,
//!    which is also what bounds what one caller can make the service retain.
//! 5. An IPSet is a sub-resource of a REGIONAL detector, so it is served only
//!    through the Region's endpoint that created it.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, IP_SETS_PER_DETECTOR, IP_SET_ACTIVATING, IP_SET_ACTIVE, IP_SET_ID_MEMBER,
    IP_SET_INACTIVE, IP_SET_TRANSITION_MILLIS,
};

/// Every member of `IpSetFormat` — "the format of the file that contains the IPSet"
/// (`smithy_query(shape="IpSetFormat")` lists exactly these six values).
const IP_SET_FORMATS: [&str; 6] = [
    "TXT",
    "STIX",
    "OTX_CSV",
    "ALIEN_VAULT",
    "PROOF_POINT",
    "FIRE_EYE",
];

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed `IpSetId` (32 lowercase hex characters, the form this service
/// mints) that no create ever minted.
const ABSENT_IP_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// Long enough for the clock to owe the settled status, plus slack.
fn past_the_transition() -> std::time::Duration {
    std::time::Duration::from_millis(IP_SET_TRANSITION_MILLIS as u64 + 250)
}

async fn create_ip_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/ipset"),
        Some(body),
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

// ---------------------------------------------------------------------------
// Rule 1: what a create stored is what a read reports.
// ---------------------------------------------------------------------------

/// One create request and the case it stands for. The rows span the classes the
/// members admit — `Name` and `Location` are `len 1..300` with no pattern and
/// `Tags`/`ExpectedBucketOwner` are optional
/// (`smithy_query(operation="CreateIPSet")`) — differing in SIZE (both ends of the
/// modelled range), ALPHABET (ASCII, non-ASCII, the documented name punctuation),
/// PRESENCE (the optional members absent and present) and COUNT (one tag, several).
fn round_trip_rows() -> Vec<(&'static str, Value)> {
    let longest_name = "n".repeat(300);
    let longest_location = format!("s3://bucket/{}", "p".repeat(287));
    vec![
        (
            "the shortest name and location the model accepts",
            json!({"name": "a", "format": "TXT", "location": "s", "activate": true}),
        ),
        (
            "the documented name alphabet: alphanumeric, whitespace, dash, underscore",
            json!({
                "name": "trusted ip-list_02",
                "format": "TXT",
                "location": "s3://my-bucket/lists/trusted.txt",
                "activate": true
            }),
        ),
        (
            "a non-ASCII name round-trips byte for byte",
            json!({
                "name": "リスト – トラスト",
                "format": "STIX",
                "location": "https://example.com/lists/トラスト.xml",
                "activate": false
            }),
        ),
        (
            "the longest name and location the model accepts",
            json!({
                "name": longest_name,
                "format": "ALIEN_VAULT",
                "location": longest_location,
                "activate": false
            }),
        ),
        (
            "the optional members present: tags and expectedBucketOwner",
            json!({
                "name": "tagged list",
                "format": "PROOF_POINT",
                "location": "s3://my-bucket/lists/proof.txt",
                "activate": true,
                "expectedBucketOwner": "123456789012",
                "tags": {"Env": "prod", "Team/Owner": "security", "cost-center": "4711"}
            }),
        ),
        (
            "a single tag whose value is the empty string the model allows",
            json!({
                "name": "one tag",
                "format": "FIRE_EYE",
                "location": "s3://my-bucket/lists/fire.csv",
                "activate": false,
                "tags": {"Reviewed": ""}
            }),
        ),
    ]
}

#[tokio::test]
async fn an_ip_set_read_reports_what_the_create_stored() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for (case, request) in round_trip_rows() {
        let id = created(&router, &detector, case, request.clone()).await;
        let body = read(&router, &detector, &id, case).await;

        for member in ["name", "format", "location"] {
            let expected = request[member].as_str().unwrap_or_default();
            let actual = body[member].as_str().unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: {member} must round-trip, expected {expected:?}, got {actual:?}"
            );
        }

        // `Tags` is always reported — empty when nothing tagged the IPSet — so a
        // client iterates it without a null guard.
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            body["tags"], expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            body["tags"]
        );

        // "This field appears in the response only if it was provided during IPSet
        // creation or update" (`smithy_query(operation="GetIPSet")`).
        let expected_owner = request.get("expectedBucketOwner").cloned();
        match expected_owner {
            Some(expected) => assert_eq!(
                body["expectedBucketOwner"], expected,
                "case {case:?}: expected expectedBucketOwner {expected:?}, got {:?}",
                body["expectedBucketOwner"]
            ),
            None => assert!(
                body.get("expectedBucketOwner").is_none_or(Value::is_null),
                "case {case:?}: expectedBucketOwner must be absent when the create did not carry it, got {:?}",
                body["expectedBucketOwner"]
            ),
        }
    }
}

/// Every `IpSetFormat` the model declares is stored and reported as given, not only
/// the ones a recorded call used.
#[tokio::test]
async fn every_modelled_format_round_trips() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for format in IP_SET_FORMATS {
        let id = created(
            &router,
            &detector,
            format,
            json!({
                "name": format!("list-{format}"),
                "format": format,
                "location": format!("s3://my-bucket/{format}.list"),
                "activate": true
            }),
        )
        .await;
        let body = read(&router, &detector, &id, format).await;
        let actual = body["format"].as_str().unwrap_or_default();
        assert_eq!(
            actual, format,
            "case {format:?}: expected format {format:?}, got {actual:?}"
        );
    }
}

/// Two creates under one detector are two distinct IPSets, each readable on its own
/// id: nothing about one read is a function of the other.
#[tokio::test]
async fn separate_creates_are_separate_ip_sets() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    let mut ids = Vec::new();
    for (case, request) in round_trip_rows() {
        ids.push((case, created(&router, &detector, case, request).await));
    }
    for (case, id) in &ids {
        let matching = ids.iter().filter(|(_, other)| other == id).count();
        assert_eq!(
            matching, 1,
            "case {case:?}: every create must mint its own id, {id:?} appears {matching} times"
        );
    }
    for (case, id) in &ids {
        let body = read(&router, &detector, id, case).await;
        assert!(
            body["name"].is_string(),
            "case {case:?}: every minted IPSet must still be readable, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: Activate decides the status, and only the clock settles it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activation_status_settles_only_when_the_clock_owes_it() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // Each row: what the create asked for, the status it must report while the
    // transition is owed, and the status the clock settles it to.
    let rows: [(bool, &str, &str); 2] = [
        (true, IP_SET_ACTIVATING, IP_SET_ACTIVE),
        (false, IP_SET_INACTIVE, IP_SET_INACTIVE),
    ];

    for (activate, transient, settled) in rows {
        let case = format!("activate={activate}");
        let id = created(
            &router,
            &detector,
            &case,
            json!({
                "name": format!("clock-{activate}"),
                "format": "TXT",
                "location": "s3://my-bucket/clock.txt",
                "activate": activate
            }),
        )
        .await;

        // The span is never zero, so a caller reading straight after the create
        // still observes the transient status.
        let first = status_of(&router, &detector, &id, &case).await;
        assert_eq!(
            first, transient,
            "case {case:?}: expected {transient} straight after the create, got {first:?}"
        );
        // A read does not advance the clock: the second read sees what the first
        // one saw.
        let second = status_of(&router, &detector, &id, &case).await;
        assert_eq!(
            second, transient,
            "case {case:?}: a read must not settle the status early, got {second:?}"
        );

        tokio::time::sleep(past_the_transition()).await;

        let after = status_of(&router, &detector, &id, &case).await;
        assert_eq!(
            after, settled,
            "case {case:?}: expected {settled} once the clock owes it, got {after:?}"
        );
        // A settled status never moves again on its own.
        let again = status_of(&router, &detector, &id, &case).await;
        assert_eq!(
            again, settled,
            "case {case:?}: a settled status must stay put, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: ClientToken is an idempotency token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_returns_the_first_ip_set() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    let base = json!({
        "name": "token list",
        "format": "TXT",
        "location": "s3://my-bucket/token.txt",
        "activate": true
    });

    // The tokens span the class `ClientToken` admits — `len 0..64`
    // (`smithy_query(operation="CreateIPSet")`) — including the empty string, which
    // carries no token at all.
    let tokens = [
        ("a short token", "t1"),
        ("a uuid-shaped token", "7f1d2c3b-4a59-4e6f-8a0b-1c2d3e4f5a6b"),
        ("the longest token the model accepts", &"z".repeat(64)[..]),
    ];

    let mut minted = Vec::new();
    for (case, token) in tokens {
        let mut request = base.clone();
        request["clientToken"] = json!(token);
        let first = created(&router, &detector, case, request.clone()).await;
        let replay = created(&router, &detector, case, request).await;
        assert_eq!(
            replay, first,
            "case {case:?}: a replayed clientToken must return the first IPSet, got {replay:?} after {first:?}"
        );
        minted.push((case, first));
    }

    for (case, id) in &minted {
        let distinct = minted.iter().filter(|(_, other)| other == id).count();
        assert_eq!(
            distinct, 1,
            "case {case:?}: a different token must mint a different IPSet, {id:?} appears {distinct} times"
        );
    }

    // No token, and the empty token the model allows, carry no idempotency: each
    // such create mints its own IPSet.
    for (case, request) in [
        ("no clientToken", base.clone()),
        ("an empty clientToken", {
            let mut request = base.clone();
            request["clientToken"] = json!("");
            request
        }),
    ] {
        let first = created(&router, &detector, case, request.clone()).await;
        let second = created(&router, &detector, case, request).await;
        assert_ne!(
            first, second,
            "case {case:?}: an untokened create must mint a new IPSet, got {second:?} twice"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an id the store does not resolve is a classified HTTP 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_id_is_refused_without_echoing_it() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let ip_set = created(
        &router,
        &detector,
        "the stored IPSet",
        json!({
            "name": "refusal list",
            "format": "TXT",
            "location": "s3://my-bucket/refusal.txt",
            "activate": true
        }),
    )
    .await;

    // Each row: the case, the detector id and IPSet id the read uses, and the
    // member the refusal must name.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a well-formed detectorId no create minted",
            ABSENT_DETECTOR_ID.to_string(),
            ip_set.clone(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a value that cannot be a detectorId at all",
            "not-a-detector".to_string(),
            ip_set.clone(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "an empty-ish detectorId at the modelled minimum length",
            "x".to_string(),
            ip_set.clone(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a well-formed ipSetId this detector does not hold",
            detector.clone(),
            ABSENT_IP_SET_ID.to_string(),
            IP_SET_ID_MEMBER,
        ),
        (
            "a value that cannot be an ipSetId at all",
            detector.clone(),
            "not-an-ip-set".to_string(),
            IP_SET_ID_MEMBER,
        ),
        (
            "an ipSetId a delete-shaped typo produced",
            detector.clone(),
            format!("{}0", &ip_set[..ip_set.len() - 1]),
            IP_SET_ID_MEMBER,
        ),
    ];

    for (case, detector_id, ip_set_id, member) in rows {
        let (status, body) = get_ip_set(&router, &detector_id, &ip_set_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        let class = error_code(&body);
        assert_eq!(
            class, INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name {member}, got {message:?}"
        );
        for echoed in [detector_id.as_str(), ip_set_id.as_str()] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
            );
        }
    }

    // A create under an unresolvable detector is refused the same way, and
    // persists nothing.
    let (status, body) = create_ip_set(
        &router,
        ABSENT_DETECTOR_ID,
        json!({
            "name": "orphan",
            "format": "TXT",
            "location": "s3://my-bucket/orphan.txt",
            "activate": true
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"create under an absent detector\": expected 400, got {status} {body:?}"
    );
    let class = error_code(&body);
    assert_eq!(
        class, INVALID_INPUT,
        "case \"create under an absent detector\": expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4b: the per-detector maximum is enforced at its published bound.
// ---------------------------------------------------------------------------

/// A detector holds IPSets up to `IP_SETS_PER_DETECTOR` and refuses the create
/// past it — the bound itself, and one past it, are both asserted.
#[tokio::test]
async fn the_per_detector_maximum_is_enforced_at_its_bound() {
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
                    "name": format!("bounded-{index}"),
                    "format": "TXT",
                    "location": format!("s3://my-bucket/bounded-{index}.txt"),
                    "activate": false
                }),
            )
            .await,
        );
    }

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
    let class = error_code(&body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );

    // The refusal persisted nothing and dropped nothing: every IPSet created up to
    // the bound still reads back.
    for (index, id) in ids.iter().enumerate() {
        let body = read(&router, &detector, id, &format!("ip set {index}")).await;
        let name = body["name"].as_str().unwrap_or_default();
        let expected = format!("bounded-{index}");
        assert_eq!(
            name, expected,
            "case \"ip set {index}\": expected the stored name {expected:?}, got {name:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: an IPSet is served only through the Region that created it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_ip_set_is_regional() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let ip_set = created(
        &router,
        &detector,
        "the us-east-1 IPSet",
        json!({
            "name": "regional list",
            "format": "TXT",
            "location": "s3://my-bucket/regional.txt",
            "activate": true
        }),
    )
    .await;

    for region in ["eu-west-1", "ap-northeast-1", "us-west-2"] {
        let (status, body) = call_in(
            &router,
            region,
            "GET",
            &format!("/detector/{detector}/ipset/{ip_set}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region:?}: another Region's endpoint must not serve the IPSet, got {status} {body:?}"
        );
    }

    // The Region that created it still serves it.
    let body = read(&router, &detector, &ip_set, "the owning Region").await;
    let name = body["name"].as_str().unwrap_or_default();
    assert_eq!(
        name, "regional list",
        "case \"the owning Region\": expected the stored name, got {name:?}"
    );
}

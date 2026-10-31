// SPDX-License-Identifier: CC0-1.0

//! The account's allowance for the trusted IP lists GuardDuty USES, end to end over
//! the wire.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//!
//! > An account may have GuardDuty use at most
//! > `state::ACTIVATED_IP_SETS_PER_ACCOUNT` trusted IP list(s) in a Region, and the
//! > allowance is taken up by the lists the account already holds: a `CreateIPSet`
//! > or `UpdateIPSet` that asks GuardDuty to start using a list past the allowance is
//! > refused with the modelled `InternalServerErrorException` (HTTP 500) and its
//! > account-limit sentence, before anything is minted or written, while the
//! > identical request that does not ask for the list to be used is served and the
//! > list is uploaded unused.
//!
//! Corollaries asserted below, because they are what make the rule state-driven
//! rather than a count of calls: the allowance is released when the list holding it
//! is deleted, it is per Region (a detector is the account's one resource in a
//! Region), and a refused request leaves no list behind.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, so these
//! tests exercise routing, restJson1 (de)serialization, modelled-constraint
//! validation and the handlers together.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, call_in, create_detector, error_code, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ACCOUNT_LIMIT_MESSAGE, ACTIVATED_IP_SETS_PER_ACCOUNT, IP_SET_ACTIVATING, IP_SET_INACTIVE,
};

/// The error class a server-side allowance failure is named by on the wire: the
/// modelled `InternalServerErrorException` (`httpError` 500,
/// `smithy_query(shape="InternalServerErrorException")`).
const SERVER_ERROR: &str = "InternalServerErrorException";

async fn create_ip_set(router: &Router, region: &str, detector: &str, body: Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector}/ipset"),
        Some(body),
    )
    .await
}

async fn update_ip_set(
    router: &Router,
    detector: &str,
    id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector}/ipset/{id}"),
        Some(body),
    )
    .await
}

async fn read_ip_set(router: &Router, detector: &str, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/detector/{detector}/ipset/{id}"), None).await
}

async fn list_ip_set_ids(router: &Router, region: &str, detector: &str) -> Vec<String> {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector}/ipset"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list-ip-sets in {region}: expected 200, got {status} {body:?}"
    );
    body["ipSetIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// Create a detector in `region` and return its id.
async fn detector_in(router: &Router, region: &str) -> String {
    let (status, body) = call_in(router, region, "POST", "/detector", Some(json!({"enable": true}))).await;
    assert_eq!(
        status, 200,
        "create-detector in {region}: expected 200, got {status} {body:?}"
    );
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a detectorId, got {body:?}"))
        .to_string()
}

/// Create an IPSet and return its id, failing the test if the create did not succeed.
async fn created(router: &Router, detector: &str, case: &str, body: Value) -> String {
    let (status, response) = create_ip_set(router, "us-east-1", detector, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-ip-set expected 200, got {status} {response:?}"
    );
    response["ipSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an ipSetId, got {response:?}"))
        .to_string()
}

/// Assert a response is the account-limit refusal: HTTP 500, the modelled server
/// error class, and the fixed sentence, which echoes nothing of the request.
fn assert_account_limit(case: &str, status: u16, body: &Value, echoed: &[&str]) {
    assert_eq!(
        status, 500,
        "case {case:?}: expected HTTP 500, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, SERVER_ERROR,
        "case {case:?}: expected the {SERVER_ERROR} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, ACCOUNT_LIMIT_MESSAGE,
        "case {case:?}: expected the account-limit sentence, got {message:?}"
    );
    for value in echoed {
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not echo {value:?}, got {message:?}"
        );
    }
}

/// One create body. The rows differ in the members they carry, the format and the
/// location spelling, so the refusal is a function of the ALLOWANCE and not of what
/// else the request holds.
fn request_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the minimal create",
            json!({"name": "minimal", "format": "TXT", "location": "s3://b/k"}),
        ),
        (
            "another format and a path-style endpoint",
            json!({
                "name": "path style",
                "format": "STIX",
                "location": "https://s3.eu-west-1.amazonaws.com/my-bucket/lists/x.xml"
            }),
        ),
        (
            "the optional members present",
            json!({
                "name": "tagged_list-01",
                "format": "ALIEN_VAULT",
                "location": "s3://my-bucket/lists/tagged.csv",
                "expectedBucketOwner": "123456789012",
                "tags": {"Env": "prod"},
                "clientToken": "allowance-token"
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Past the allowance, a create that asks for the list to be used is refused —
// and the same create that does not ask is served.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_past_the_allowance_is_refused_only_when_it_asks_for_the_list_to_be_used() {
    for (case, body) in request_rows() {
        let router = new_router();
        let detector = detector_in(&router, "us-east-1").await;

        // Take the allowance up: the lists the account holds are what consume it,
        // so this many creates leave no room for a further activation.
        let mut held = Vec::new();
        for index in 0..ACTIVATED_IP_SETS_PER_ACCOUNT {
            let case = format!("{case}: the list holding allowance {index}");
            held.push(
                created(
                    &router,
                    &detector,
                    &case,
                    json!({
                        "name": format!("holder-{index}"),
                        "format": "TXT",
                        "location": format!("s3://my-bucket/holder-{index}.txt"),
                        "activate": true
                    }),
                )
                .await,
            );
        }

        // The same request, twice: asking for the list to be used is refused, and
        // not asking is served.
        let mut activating = body.clone();
        activating["activate"] = json!(true);
        let (status, refused) =
            create_ip_set(&router, "us-east-1", &detector, activating).await;
        assert_account_limit(
            &format!("{case}: activate=true past the allowance"),
            status,
            &refused,
            &[&detector],
        );

        // The refusal minted nothing: the collection is exactly the lists that hold
        // the allowance.
        let listed = list_ip_set_ids(&router, "us-east-1", &detector).await;
        assert_eq!(
            listed.len(),
            held.len(),
            "case {case:?}: a refused create must mint no IPSet, got {listed:?}"
        );

        let mut inactive = body.clone();
        inactive["activate"] = json!(false);
        // A second create under the token the refused one carried would otherwise
        // replay it; the refusal stored nothing, so the token is free.
        let (status, served) = create_ip_set(&router, "us-east-1", &detector, inactive).await;
        assert_eq!(
            status, 200,
            "case {case:?}: activate=false past the allowance must be served, got {status} {served:?}"
        );
        let id = served["ipSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ipSetId, got {served:?}"))
            .to_string();

        // And it is readable, stored unused.
        let (status, stored) = read_ip_set(&router, &detector, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the uploaded list must read back, got {status} {stored:?}"
        );
        let reported = stored["status"].as_str().unwrap_or_default();
        assert_eq!(
            reported, IP_SET_INACTIVE,
            "case {case:?}: a list nobody asked GuardDuty to use is stored unused, got {reported:?}"
        );
        let location = stored["location"].as_str().unwrap_or_default();
        let expected = body["location"].as_str().unwrap_or_default();
        assert_eq!(
            location, expected,
            "case {case:?}: expected the stored location {expected:?}, got {location:?}"
        );
    }
}

/// Up to the allowance a create that asks for its list to be used is SERVED: the
/// bound itself is asserted, not only one past it.
#[tokio::test]
async fn a_create_within_the_allowance_is_served_and_the_list_is_in_use() {
    let router = new_router();
    let detector = detector_in(&router, "us-east-1").await;

    for index in 0..ACTIVATED_IP_SETS_PER_ACCOUNT {
        let case = format!("activation {index} of {ACTIVATED_IP_SETS_PER_ACCOUNT}");
        let id = created(
            &router,
            &detector,
            &case,
            json!({
                "name": format!("within-{index}"),
                "format": "TXT",
                "location": format!("s3://my-bucket/within-{index}.txt"),
                "activate": true
            }),
        )
        .await;
        let (status, stored) = read_ip_set(&router, &detector, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the list must read back, got {status} {stored:?}"
        );
        let reported = stored["status"].as_str().unwrap_or_default();
        assert_eq!(
            reported, IP_SET_ACTIVATING,
            "case {case:?}: a list the create activated is on its way into use, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The update that asks for a list to be used is held to the same allowance.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_that_asks_for_a_list_to_be_used_is_held_to_the_allowance() {
    let router = new_router();
    let detector = detector_in(&router, "us-east-1").await;

    let holder = created(
        &router,
        &detector,
        "the list holding the allowance",
        json!({
            "name": "holder",
            "format": "TXT",
            "location": "s3://my-bucket/holder.txt",
            "activate": true
        }),
    )
    .await;
    let other = created(
        &router,
        &detector,
        "the list uploaded unused",
        json!({
            "name": "unused",
            "format": "TXT",
            "location": "s3://my-bucket/unused.txt",
            "activate": false
        }),
    )
    .await;

    // Asking for the unused list to be used is refused — alone and beside every
    // other updatable member, since the refusal is about the allowance.
    for (case, extra) in [
        ("activate alone", json!({})),
        ("beside a name", json!({"name": "renamed"})),
        ("beside a location", json!({"location": "s3://my-bucket/moved.txt"})),
        (
            "beside an expectedBucketOwner",
            json!({"expectedBucketOwner": "123456789012"}),
        ),
    ] {
        let mut request = json!({"activate": true});
        if let Some(members) = extra.as_object() {
            for (key, value) in members {
                request[key] = value.clone();
            }
        }
        let (status, body) = update_ip_set(&router, &detector, &other, request).await;
        assert_account_limit(case, status, &body, &[&detector, &other]);

        // Nothing was written: the list is still unused and still carries what the
        // create stored.
        let (status, stored) = read_ip_set(&router, &detector, &other).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the list must still read back, got {status} {stored:?}"
        );
        let reported = stored["status"].as_str().unwrap_or_default();
        assert_eq!(
            reported, IP_SET_INACTIVE,
            "case {case:?}: a refused update must leave the list unused, got {reported:?}"
        );
        for (member, expected) in [("name", "unused"), ("location", "s3://my-bucket/unused.txt")] {
            let actual = stored[member].as_str().unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: a refused update must keep {member} {expected:?}, got {actual:?}"
            );
        }
    }

    // An update that does NOT ask for the list to be used is served, past the
    // allowance, because it puts nothing into use.
    let (status, body) = update_ip_set(
        &router,
        &detector,
        &other,
        json!({"name": "renamed_unused", "activate": false}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an update that asks for nothing to be used\": expected 200, got {status} {body:?}"
    );

    // Releasing the allowance — deleting the list that holds it — makes the same
    // update succeed: the refusal is read off the account's STATE, not off how many
    // times it was asked.
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/detector/{detector}/ipset/{holder}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete the holder\": expected 200, got {status} {body:?}"
    );
    let (status, body) = update_ip_set(&router, &detector, &other, json!({"activate": true})).await;
    assert_eq!(
        status, 200,
        "case \"activate once the allowance is released\": expected 200, got {status} {body:?}"
    );
    let (status, stored) = read_ip_set(&router, &detector, &other).await;
    assert_eq!(
        status, 200,
        "case \"the activated list\": expected 200, got {status} {stored:?}"
    );
    let reported = stored["status"].as_str().unwrap_or_default();
    assert_eq!(
        reported, IP_SET_ACTIVATING,
        "case \"the activated list\": expected the list to be on its way into use, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The allowance is released by a delete, and it is per Region.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_releases_the_allowance() {
    let router = new_router();
    let detector = detector_in(&router, "us-east-1").await;

    // Create, activate, delete, activate again — several times, so the release is a
    // property of the state and not of the first round.
    for round in 0..3 {
        let case = format!("round {round}");
        let id = created(
            &router,
            &detector,
            &case,
            json!({
                "name": format!("cycled-{round}"),
                "format": "TXT",
                "location": format!("s3://my-bucket/cycled-{round}.txt"),
                "activate": true
            }),
        )
        .await;
        // While it is held, a second activation is refused.
        let (status, body) = create_ip_set(
            &router,
            "us-east-1",
            &detector,
            json!({
                "name": format!("blocked-{round}"),
                "format": "TXT",
                "location": format!("s3://my-bucket/blocked-{round}.txt"),
                "activate": true
            }),
        )
        .await;
        assert_account_limit(&case, status, &body, &[&detector]);

        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/detector/{detector}/ipset/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete-ip-set expected 200, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn the_allowance_is_per_region() {
    let router = new_router();

    // Each Region is its own account resource, so each has its own allowance: the
    // list in use in one Region does not decide what another Region answers.
    let regions = ["us-east-1", "eu-west-1", "ap-northeast-1"];
    let mut detectors = Vec::new();
    for region in regions {
        detectors.push((region, detector_in(&router, region).await));
    }

    for (region, detector) in &detectors {
        let (status, body) = create_ip_set(
            &router,
            region,
            detector,
            json!({
                "name": format!("regional-{region}"),
                "format": "TXT",
                "location": format!("s3://my-bucket/{region}.txt"),
                "activate": true
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: each Region has its own allowance, got {status} {body:?}"
        );
    }

    // And within one Region the allowance is still spent.
    let (region, detector) = &detectors[0];
    let (status, body) = create_ip_set(
        &router,
        region,
        detector,
        json!({
            "name": "second-in-region",
            "format": "TXT",
            "location": "s3://my-bucket/second.txt",
            "activate": true
        }),
    )
    .await;
    assert_account_limit(region, status, &body, &[detector]);
}

/// The allowance is the ACCOUNT's, so it is not reset by creating the detector
/// again: `CreateDetector` on a Region that already has one does not mint a second.
#[tokio::test]
async fn a_repeated_create_detector_does_not_reset_the_allowance() {
    let router = new_router();
    let detector = detector_in(&router, "us-east-1").await;
    let _ = created(
        &router,
        &detector,
        "the list holding the allowance",
        json!({
            "name": "holder",
            "format": "TXT",
            "location": "s3://my-bucket/holder.txt",
            "activate": true
        }),
    )
    .await;

    // A second create for the same account and Region is refused ("you can have only
    // one detector per account per Region"), so the detector that holds the lists is
    // still the one above.
    let (status, body) = create_detector(&router, json!({"enable": true})).await;
    assert_eq!(
        status, 400,
        "case \"a second detector\": expected the one-per-Region refusal, got {status} {body:?}"
    );

    let (status, body) = create_ip_set(
        &router,
        "us-east-1",
        &detector,
        json!({
            "name": "after-the-retry",
            "format": "TXT",
            "location": "s3://my-bucket/after.txt",
            "activate": true
        }),
    )
    .await;
    assert_account_limit("after a repeated create-detector", status, &body, &[&detector]);
}

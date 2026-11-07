// SPDX-License-Identifier: CC0-1.0

//! The network-server half of a service profile, its data-rate window rule, and the
//! observability of its deletion — end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * a created service profile is provisioned with the network server's settings, so a
//!   read reports a value for every member of the reported shape the request could not
//!   name, whatever the request did name — including a request that named no `LoRaWAN`
//!   block at all;
//! * a create whose data-rate window `DrMin..DrMax` is inverted — each member's
//!   effective value being the one the request named or the modelled default — is
//!   refused and provisions nothing;
//! * a profile the store does not hold, because it never existed or because a delete
//!   removed it, is refused with the sentence this service words for a missing
//!   resource.
//!
//! The wire harness is the one the sibling suites already have: `tests/all/*.rs` are
//! modules of ONE test target, so `router`, `call` and `error_code` are shared from
//! there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE wire harness this crate's suites share.
use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// The settings AWS's LoRaWAN network server provisions a service profile with: the
/// account's service-profile defaults, which no member of `CreateServiceProfile`'s
/// `LoRaWAN` block can set, so they are the same for every created profile.
///
/// Source: the members `LoRaWANGetServiceProfileInfo` declares and
/// `LoRaWANServiceProfile` does not (`smithy_query(query="shape",
/// shape="LoRaWANGetServiceProfileInfo,LoRaWANServiceProfile")`), with the value the
/// service reports for a profile created without settings. Each numeric value sits
/// inside the member's published range, which
/// [`a_provisioned_default_sits_inside_its_published_range`] re-checks against those
/// same traits.
fn network_server_defaults() -> Value {
    json!({
        "UlRate": 60,
        "UlBucketSize": 4096,
        "DlRate": 60,
        "DlBucketSize": 4096,
        "DevStatusReqFreq": 24,
        "MinGwDiversity": 1,
        "TargetPer": 5,
        "AdrAlgorithm": "ORIGINAL",
        "UplinkHistoryLen": 20,
    })
}

/// The published range of each member [`network_server_defaults`] carries a number for
/// and the model constrains.
/// Source: `smithy_query(query="shape", shape="LoRaWANGetServiceProfileInfo")` —
/// `TargetPer` `@range` 0..100, `MinGwDiversity` `@range` 1..100, and the rate, bucket
/// and device-status members `@range` 0..2147483647.
const REPORTED_RANGES: &[(&str, i64, i64)] = &[
    ("UlRate", 0, 2_147_483_647),
    ("UlBucketSize", 0, 2_147_483_647),
    ("DlRate", 0, 2_147_483_647),
    ("DlBucketSize", 0, 2_147_483_647),
    ("DevStatusReqFreq", 0, 2_147_483_647),
    ("MinGwDiversity", 1, 100),
    ("TargetPer", 0, 100),
];

/// The top of `DrMin`/`DrMax`'s published range.
/// Source: `smithy_query(query="shape", shape="LoRaWANServiceProfile")` — both `@range`
/// 0..15.
const DR_MAX_BOUND: i64 = 15;

/// The `DrMax` the service applies when a request omits it.
/// Source: `LoRaWANGetServiceProfileInfo.DrMax`'s modelled `@default`.
const DR_DEFAULT: i64 = 0;

async fn create_profile(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/service-profiles", Some(body)).await
}

async fn get_profile(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(router, Method::GET, &format!("/service-profiles/{id}"), None).await
}

async fn delete_profile(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::DELETE,
        &format!("/service-profiles/{id}"),
        None,
    )
    .await
}

/// Whatever a create names — nothing, one member, every member, either end of each
/// range — the profile it mints carries the network server's settings, and a read
/// reports every one of them.
#[tokio::test]
async fn a_created_profile_carries_the_network_server_defaults() {
    let router = router();

    // The cases differ in WHICH create-settable members the request names and at what
    // value — including naming no `LoRaWAN` block at all — which is what the rule
    // generalises over: the provisioned settings do not depend on them.
    let cases: &[(&str, Option<Value>)] = &[
        ("no-lorawan-member", None),
        ("no-settings", Some(json!({}))),
        ("one-boolean", Some(json!({ "AddGwMetadata": true }))),
        (
            "window-at-the-top-of-its-range",
            Some(json!({ "DrMin": DR_MAX_BOUND, "DrMax": DR_MAX_BOUND })),
        ),
        (
            "every-settable-member",
            Some(json!({
                "AddGwMetadata": true,
                "DrMin": 2,
                "DrMax": 9,
                "PrAllowed": true,
                "RaAllowed": true,
                "TxPowerIndexMin": 1,
                "TxPowerIndexMax": 14,
                "NbTransMin": 1,
                "NbTransMax": 2,
            })),
        ),
        (
            "only-the-transmission-counters",
            Some(json!({ "NbTransMin": 0, "NbTransMax": 15 })),
        ),
    ];

    for (case, lorawan) in cases {
        let mut body = json!({ "Name": case });
        if let Some(lorawan) = lorawan {
            body["LoRaWAN"] = lorawan.clone();
        }
        let (status, created) = create_profile(&router, body).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: expected 201, got {status} {created}"
        );
        let id = created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {created}"))
            .to_string();

        let (status, fetched) = get_profile(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read, got {status} {fetched}"
        );

        for (member, expected) in network_server_defaults()
            .as_object()
            .expect("an object of settings")
        {
            assert_eq!(
                fetched["LoRaWAN"].get(member),
                Some(expected),
                "case {case:?}: expected LoRaWAN.{member} = {expected}, got {:?}",
                fetched["LoRaWAN"].get(member)
            );
        }

        // What the request DID name is still the stored value, so the provisioning
        // defaults never overwrite a caller's setting.
        for (member, value) in lorawan
            .as_ref()
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            assert_eq!(
                fetched["LoRaWAN"].get(member),
                Some(value),
                "case {case:?}: expected the requested LoRaWAN.{member} = {value}, got {:?}",
                fetched["LoRaWAN"].get(member)
            );
        }
    }
}

/// Every provisioned default lies inside the range its member publishes, so a client
/// that validates what it reads against the model accepts it.
#[tokio::test]
async fn a_provisioned_default_sits_inside_its_published_range() {
    let router = router();
    let (_, created) = create_profile(&router, json!({ "LoRaWAN": {} })).await;
    let id = created["Id"].as_str().expect("an Id").to_string();
    let (_, fetched) = get_profile(&router, &id).await;

    for (member, min, max) in REPORTED_RANGES {
        let reported = fetched["LoRaWAN"]
            .get(member)
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {member:?}: nothing reported in {fetched}"));
        assert!(
            (*min..=*max).contains(&reported),
            "case {member:?}: expected a value in {min}..={max}, got {reported}"
        );
    }
}

/// A create whose effective `DrMin` exceeds its effective `DrMax` is refused with the
/// modelled `ValidationException` and mints nothing; a window that is not inverted is
/// accepted and reads back.
#[tokio::test]
async fn an_inverted_data_rate_window_is_refused() {
    let router = router();

    // (case, DrMin, DrMax) — `None` means the request omits the member, so the
    // effective value is the modelled default.
    let cases: &[(&str, Option<i64>, Option<i64>)] = &[
        ("min-at-the-top-max-omitted", Some(DR_MAX_BOUND), None),
        ("min-one-above-the-default-max-omitted", Some(1), None),
        ("min-above-a-named-max", Some(5), Some(3)),
        ("min-one-above-a-named-max", Some(DR_MAX_BOUND), Some(14)),
        ("equal-window-at-the-top", Some(DR_MAX_BOUND), Some(DR_MAX_BOUND)),
        ("equal-window-at-the-bottom", Some(0), Some(0)),
        ("ascending-window", Some(3), Some(7)),
        ("min-omitted-max-named", None, Some(DR_MAX_BOUND)),
        ("both-omitted", None, None),
        ("min-at-the-default-max-omitted", Some(0), None),
    ];

    for (case, dr_min, dr_max) in cases {
        let mut lorawan = json!({});
        if let Some(value) = dr_min {
            lorawan["DrMin"] = json!(value);
        }
        if let Some(value) = dr_max {
            lorawan["DrMax"] = json!(value);
        }
        let (status, body) = create_profile(&router, json!({ "LoRaWAN": lorawan })).await;

        let effective_min = dr_min.unwrap_or(DR_DEFAULT);
        let effective_max = dr_max.unwrap_or(DR_DEFAULT);
        if effective_min > effective_max {
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {case:?}: expected 400 for DrMin {effective_min} over DrMax \
                 {effective_max}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["Message"],
                json!(format!(
                    "DrMin {effective_min} should not be larger than DrMax {effective_max}"
                )),
                "case {case:?}: unexpected refusal message, got {:?}",
                body["Message"]
            );
            assert!(
                body.get("Id").is_none() && body.get("Arn").is_none(),
                "case {case:?}: a refused create must mint nothing, got {body}"
            );
        } else {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {case:?}: expected 201 for DrMin {effective_min} under DrMax \
                 {effective_max}, got {status} {body}"
            );
            let id = body["Id"].as_str().expect("an Id").to_string();
            let (status, fetched) = get_profile(&router, &id).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from the read, got {status} {fetched}"
            );
            assert_eq!(
                (fetched["LoRaWAN"]["DrMin"].clone(), fetched["LoRaWAN"]["DrMax"].clone()),
                (json!(effective_min), json!(effective_max)),
                "case {case:?}: expected the window {effective_min}..{effective_max} read \
                 back, got {:?}..{:?}",
                fetched["LoRaWAN"]["DrMin"],
                fetched["LoRaWAN"]["DrMax"]
            );
        }
    }
}

/// The refusal body for a service profile the store does not hold: AWS's sentence for a
/// missing resource, with the requested id and the resource kind.
fn expected_not_found(id: &str) -> Value {
    json!({
        "Message": format!(
            "Cannot complete the step because resource is not found, with resource type \
             ServiceProfile, resource Id {id}"
        ),
        "ResourceId": id,
        "ResourceType": "ServiceProfile",
    })
}

/// A delete is observable by the next caller: the profile stops resolving, the second
/// delete is a 404, and every other profile is untouched. An id that never existed is
/// refused the same way.
#[tokio::test]
async fn a_deleted_service_profile_is_not_found() {
    let router = router();

    // Profiles differing in what their create named, so the delete is exercised over
    // more than one kind of stored record.
    let bodies = [
        json!({ "LoRaWAN": {} }),
        json!({ "Name": "Deleted", "LoRaWAN": { "AddGwMetadata": true, "DrMax": 7 } }),
        json!({ "Name": "Kept", "LoRaWAN": { "PrAllowed": true }, "Tags": [{ "Key": "k", "Value": "v" }] }),
    ];
    let mut ids = Vec::new();
    for body in &bodies {
        let (status, created) = create_profile(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {body}: expected 201, got {status} {created}"
        );
        ids.push(created["Id"].as_str().expect("an Id").to_string());
    }

    for (index, id) in ids.iter().enumerate() {
        let (status, body) = delete_profile(&router, id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {index}: expected 204 from the delete, got {status} {body}"
        );

        let (status, read) = get_profile(&router, id).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {index}: a deleted profile must not resolve, got {status} {read}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {index}: expected ResourceNotFoundException, got {read}"
        );
        for (member, expected) in expected_not_found(id).as_object().expect("a body") {
            assert_eq!(
                read.get(member),
                Some(expected),
                "case {index}: expected {member} = {expected}, got {:?}",
                read.get(member)
            );
        }

        // The second delete has nothing to remove, and says so the same way.
        let (status, again) = delete_profile(&router, id).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {index}: deleting twice must be a 404, got {status} {again}"
        );
        assert_eq!(
            again.get("Message"),
            expected_not_found(id).get("Message"),
            "case {index}: expected the missing-resource sentence, got {:?}",
            again.get("Message")
        );

        // Every profile the delete did not name still reads back.
        for later in &ids[index + 1..] {
            let (status, still) = get_profile(&router, later).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {index}: profile {later} must be untouched, got {status} {still}"
            );
        }
    }
}

/// An `Id` that never existed is refused with the same sentence as a deleted one —
/// whatever the id looks like.
#[tokio::test]
async fn an_unknown_service_profile_id_carries_the_missing_resource_sentence() {
    let router = router();

    for id in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-uuid",
        "A",
        "service-profile",
    ] {
        let (status, body) = get_profile(&router, id).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {id:?}: expected 404, got {status} {body}"
        );
        for (member, expected) in expected_not_found(id).as_object().expect("a body") {
            assert_eq!(
                body.get(member),
                Some(expected),
                "case {id:?}: expected {member} = {expected}, got {:?}",
                body.get(member)
            );
        }
    }
}

/// A `ClientRequestToken` does not outlive the profile it minted: after the delete, the
/// same token mints a NEW profile rather than replaying the removed one.
#[tokio::test]
async fn a_token_whose_profile_was_deleted_mints_a_new_profile() {
    let router = router();

    for token in ["token-one", "token_two", "TOKEN-3"] {
        let body = json!({ "ClientRequestToken": token, "LoRaWAN": { "AddGwMetadata": true } });
        let (status, first) = create_profile(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {token:?}: expected 201, got {status} {first}"
        );
        let first_id = first["Id"].as_str().expect("an Id").to_string();

        let (status, deleted) = delete_profile(&router, &first_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {token:?}: expected 204 from the delete, got {status} {deleted}"
        );

        let (status, second) = create_profile(&router, body).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {token:?}: expected 201 from the replay, got {status} {second}"
        );
        assert_ne!(
            second["Id"], first["Id"],
            "case {token:?}: the replay must mint a new profile, got {:?} again",
            second["Id"]
        );

        let (status, fetched) = get_profile(&router, second["Id"].as_str().expect("an Id")).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {token:?}: the new profile must read back, got {status} {fetched}"
        );
    }
}

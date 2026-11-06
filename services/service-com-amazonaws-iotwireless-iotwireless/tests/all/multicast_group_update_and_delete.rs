// SPDX-License-Identifier: CC0-1.0

//! Patching and deleting a multicast group, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * a multicast group's mutable members live on ONE stored record, so
//!   `UpdateMulticastGroup` merges the members the request carried into it — leaving
//!   every member it omitted as it was, inside the `LoRaWAN` block too — and the next
//!   `GetMulticastGroup` reports the merged record;
//! * `Status` is service-assigned lifecycle state: it is stamped on the record at create
//!   time, reported by the reader, and a patch (which carries no value for it) does not
//!   move it;
//! * `DefaultSessionParameters` is the group's downlink session plan, which the service
//!   assigns rather than merely echoing: a member the write supplied is kept as
//!   submitted, and a member it left out takes the value of the group's RF region's
//!   downlink channel, so the reader never answers half a plan;
//! * `DeleteMulticastGroup` removes the record, so the group's reader stops resolving
//!   the id and the delete is observable: the next read, patch or delete of that id is
//!   the modelled 404, and what the group held — the memberships devices had in it, the
//!   idempotency token that minted it — goes with it;
//! * an `Id` the account does not hold is refused identically by the read, the patch and
//!   the delete, with the service's not-found sentence for the kind and id addressed.
//!
//! The wire harness is the one the sibling suites already have: `tests/all/*.rs` are
//! modules of ONE test target, so `call`, `error_code`, `request_signed_for`,
//! `seed_group`, `seed_device`, `associate` and `member_ids` are shared from there
//! rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotwireless_iotwireless::state::MULTICAST_GROUP_STATUS_PENDING;

use super::destination_and_wireless_device_lifecycle::{
    call, error_code, request_signed_for, router, REGION,
};
use super::multicast_group_device_association::{
    associate, member_ids, seed_destination, seed_device, seed_group,
};

/// `Description`'s published length ceiling.
/// Source: `smithy_query(query="operation", operation="UpdateMulticastGroup")`.
const DESCRIPTION_MAX_LEN: usize = 2048;

/// `MulticastGroupStatus`'s published length ceiling.
/// Source: `smithy_query(query="shape", shape="MulticastGroupStatus")`.
const STATUS_MAX_LEN: usize = 256;

/// `DefaultSessionParametersMulticast`'s published ranges.
/// Source: `smithy_query(query="shape", shape="DefaultSessionParametersMulticast")`.
const DL_DR_MIN: i64 = 0;
const DL_DR_MAX: i64 = 15;
const DL_FREQ_MIN: i64 = 100_000_000;
const DL_FREQ_MAX: i64 = 1_000_000_000;

/// Every value of `SupportedRfRegion` — the model-generated constant, so the suite and
/// the service cannot drift apart.
/// Source: `smithy_query(query="shape", shape="SupportedRfRegion")`.
const RF_REGIONS: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::SUPPORTED_RF_REGION_VALUES;

/// The downlink channel each regional plan reserves for class B/C downlinks, as
/// `(SupportedRfRegion, DlDr, DlFreq)`: the plan's second receive window, which is the
/// channel a multicast downlink goes out on, with the AS923 plans' published frequency
/// offsets and the data rate the service assigns on the 500 kHz plans' shared channel.
/// One row per entry of the modelled catalog above, so the table is asserted on regions
/// the issue's units never name.
const REGION_DOWNLINK_CHANNEL: &[(&str, i64, i64)] = &[
    ("EU868", 0, 869_525_000),
    ("US915", 9, 923_300_000),
    ("AU915", 9, 923_300_000),
    ("AS923-1", 2, 923_200_000),
    ("AS923-2", 2, 921_400_000),
    ("AS923-3", 2, 916_600_000),
    ("AS923-4", 2, 917_300_000),
    ("EU433", 0, 434_665_000),
    ("CN470", 0, 505_300_000),
    ("CN779", 0, 786_000_000),
    ("RU864", 0, 869_100_000),
    ("KR920", 0, 921_900_000),
    ("IN865", 2, 866_550_000),
];

/// One answer from the router: the status, the `x-amzn-errortype` header the wire
/// protocol names an error shape in, and the parsed body.
struct Answer {
    status: StatusCode,
    error_type: String,
    body: Value,
}

/// Dispatch one call and keep its headers as well as its body, which [`call`] does not.
async fn call_with_headers(
    router: &MultiServiceRouter,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let response = router
        .dispatch(request_signed_for(Some(REGION), method, uri, body))
        .await;
    let status = response.status();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

async fn get_group(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(router, Method::GET, &format!("/multicast-groups/{id}"), None).await
}

async fn patch_group(
    router: &MultiServiceRouter,
    id: &str,
    body: Value,
) -> (StatusCode, Value) {
    call(
        router,
        Method::PATCH,
        &format!("/multicast-groups/{id}"),
        Some(body),
    )
    .await
}

async fn delete_group(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::DELETE,
        &format!("/multicast-groups/{id}"),
        None,
    )
    .await
}

/// The body every patch case starts from: every mutable member populated, so a case can
/// assert that what it did NOT send survived.
fn seed_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "the description the create carried",
        "LoRaWAN": {
            "RfRegion": "US915",
            "DlClass": "ClassB",
            "ParticipatingGateways": {
                "GatewayList": ["gw-seed"],
                "TransmissionInterval": 1_000,
            },
            "DefaultSessionParameters": { "DlDr": 9, "DlFreq": 923_300_000 },
        },
    })
}

/// The members a read must report, as the patch left them: the seeded body with the
/// patch merged in, member by member, one level into `LoRaWAN`.
fn merged(seed: &Value, patch: &Value) -> Value {
    let mut expected = seed.clone();
    for (key, value) in patch.as_object().expect("the patch is an object") {
        if key == "LoRaWAN" {
            for (member, value) in value.as_object().expect("LoRaWAN is an object") {
                expected["LoRaWAN"][member] = value.clone();
            }
        } else {
            expected[key] = value.clone();
        }
    }
    expected
}

/// One patch applied to a freshly seeded group.
struct PatchCase {
    case: &'static str,
    patch: Value,
}

fn patch_cases() -> Vec<PatchCase> {
    vec![
        // The member the fixture patches, on its own: nothing else may move.
        PatchCase {
            case: "description-only",
            patch: json!({ "Description": "an updated description" }),
        },
        // The same member emptied, which is inside its published length range.
        PatchCase {
            case: "description-emptied",
            patch: json!({ "Description": "" }),
        },
        // And at its published ceiling.
        PatchCase {
            case: "description-at-ceiling",
            patch: json!({ "Description": "d".repeat(DESCRIPTION_MAX_LEN) }),
        },
        PatchCase {
            case: "name-only",
            patch: json!({ "Name": "renamed group" }),
        },
        PatchCase {
            case: "name-emptied",
            patch: json!({ "Name": "" }),
        },
        PatchCase {
            case: "name-and-description",
            patch: json!({ "Name": "both", "Description": "both" }),
        },
        // One member INSIDE the LoRaWAN block: the rest of the block, the session
        // parameters above all, must survive.
        PatchCase {
            case: "lorawan-rf-region-only",
            patch: json!({ "LoRaWAN": { "RfRegion": "EU868" } }),
        },
        PatchCase {
            case: "lorawan-dl-class-only",
            patch: json!({ "LoRaWAN": { "DlClass": "ClassC" } }),
        },
        // The session plan itself, at both ends of both published ranges.
        PatchCase {
            case: "lorawan-session-parameters-low",
            patch: json!({
                "LoRaWAN": {
                    "DefaultSessionParameters": { "DlDr": DL_DR_MIN, "DlFreq": DL_FREQ_MIN },
                }
            }),
        },
        PatchCase {
            case: "lorawan-session-parameters-high",
            patch: json!({
                "LoRaWAN": {
                    "DefaultSessionParameters": { "DlDr": DL_DR_MAX, "DlFreq": DL_FREQ_MAX },
                }
            }),
        },
        PatchCase {
            case: "lorawan-participating-gateways",
            patch: json!({
                "LoRaWAN": {
                    "ParticipatingGateways": {
                        "GatewayList": ["gw-patched-a", "gw-patched-b"],
                        "TransmissionInterval": 0,
                    },
                }
            }),
        },
        // Every mutable member at once.
        PatchCase {
            case: "everything",
            patch: json!({
                "Name": "fully patched",
                "Description": "fully patched",
                "LoRaWAN": {
                    "RfRegion": "AS923-4",
                    "DlClass": "ClassC",
                    "ParticipatingGateways": { "GatewayList": [], "TransmissionInterval": 60_000 },
                    "DefaultSessionParameters": { "DlDr": 1, "DlFreq": 869_525_000 },
                },
            }),
        },
        // A patch that carries nothing: a no-op that must not blank the record.
        PatchCase {
            case: "empty-patch",
            patch: json!({}),
        },
        PatchCase {
            case: "lorawan-empty-block",
            patch: json!({ "LoRaWAN": {} }),
        },
    ]
}

/// A patch merges into the stored group and the next read reports the merged record:
/// the members it carried are the ones it sent, the members it omitted are the ones the
/// create stored, and the group's identity and lifecycle status are untouched.
#[tokio::test]
async fn a_patch_merges_into_the_stored_group_and_the_read_reflects_it() {
    let router = router();

    for PatchCase { case, patch } in patch_cases() {
        let seed = seed_body(case);
        let id = seed_group(&router, seed.clone()).await;
        let (_, before) = get_group(&router, &id).await;

        let (status, answered) = patch_group(&router, &id, patch.clone()).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from UpdateMulticastGroup, got {status} {answered}"
        );

        let (status, fetched) = get_group(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read after the patch, got {status} {fetched}"
        );

        let expected = merged(&seed, &patch);
        for member in ["Name", "Description"] {
            assert_eq!(
                fetched.get(member),
                expected.get(member),
                "case {case:?}: expected {member} {:?}, got {:?}",
                expected.get(member),
                fetched.get(member)
            );
        }
        for member in [
            "RfRegion",
            "DlClass",
            "ParticipatingGateways",
            "DefaultSessionParameters",
        ] {
            assert_eq!(
                fetched["LoRaWAN"].get(member),
                expected["LoRaWAN"].get(member),
                "case {case:?}: expected LoRaWAN.{member} {:?}, got {:?}",
                expected["LoRaWAN"].get(member),
                fetched["LoRaWAN"].get(member)
            );
        }
        // A patch names no identity and no lifecycle member, so none of them moves.
        for member in ["Id", "Arn", "CreatedAt", "Status"] {
            assert_eq!(
                fetched.get(member),
                before.get(member),
                "case {case:?}: a patch must leave {member} at {:?}, got {:?}",
                before.get(member),
                fetched.get(member)
            );
        }
    }
}

/// `Status` is assigned at create time, reported by every read, and moved by neither a
/// read nor a patch — over groups created with different LoRaWAN settings.
#[tokio::test]
async fn a_created_group_reports_a_lifecycle_status_no_read_or_patch_moves() {
    let router = router();

    let creates = [
        ("empty-lorawan", json!({ "LoRaWAN": {} })),
        (
            "region-only",
            json!({ "Name": "status-region", "LoRaWAN": { "RfRegion": "EU868" } }),
        ),
        (
            "full",
            json!({
                "Name": "status-full",
                "Description": "status",
                "LoRaWAN": {
                    "RfRegion": "US915",
                    "DlClass": "ClassB",
                    "DefaultSessionParameters": { "DlDr": 9, "DlFreq": 923_300_000 },
                },
            }),
        ),
    ];

    for (case, body) in creates {
        let id = seed_group(&router, body).await;
        let (_, fetched) = get_group(&router, &id).await;
        let status_member = fetched.get("Status").cloned().unwrap_or(Value::Null);
        assert_eq!(
            status_member,
            json!(MULTICAST_GROUP_STATUS_PENDING),
            "case {case:?}: expected the initial lifecycle status, got {status_member:?}"
        );
        let text = status_member.as_str().unwrap_or_default();
        assert!(
            !text.is_empty() && text.len() <= STATUS_MAX_LEN,
            "case {case:?}: Status must be a non-empty string within its published length, \
             got {text:?}"
        );

        // Reading is not a mutation: the second read answers exactly what the first did.
        let (_, again) = get_group(&router, &id).await;
        assert_eq!(
            again.get("Status"),
            fetched.get("Status"),
            "case {case:?}: a second read must report the same Status, got {:?}",
            again.get("Status")
        );

        let (status, answered) =
            patch_group(&router, &id, json!({ "Description": "patched" })).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from the patch, got {status} {answered}"
        );
        let (_, after) = get_group(&router, &id).await;
        assert_eq!(
            after.get("Status"),
            fetched.get("Status"),
            "case {case:?}: a patch carries no Status, so it must not move it, got {:?}",
            after.get("Status")
        );
    }
}

/// The refusal every multicast-group-addressing operation gives for an `Id` the account
/// does not hold: 404 `ResourceNotFoundException`, the kind and the requested id on the
/// shape's own members, and the service's sentence for this error class.
fn assert_not_found(case: &str, operation: &str, id: &str, answered: &Answer) {
    assert_eq!(
        answered.status,
        StatusCode::NOT_FOUND,
        "case {case:?}: expected 404 from {operation}, got {} {}",
        answered.status,
        answered.body
    );
    assert_eq!(
        error_code(&answered.body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException from {operation}, got {}",
        answered.body
    );
    assert_eq!(
        answered.error_type, "ResourceNotFoundException",
        "case {case:?}: expected {operation} to name the error shape in x-amzn-errortype, \
         got {:?}",
        answered.error_type
    );
    assert_eq!(
        answered.body["ResourceType"].as_str(),
        Some("MulticastGroup"),
        "case {case:?}: expected ResourceType MulticastGroup from {operation}, got {}",
        answered.body
    );
    assert_eq!(
        answered.body["ResourceId"].as_str(),
        Some(id),
        "case {case:?}: expected ResourceId {id:?} from {operation}, got {}",
        answered.body
    );
    let expected_message = format!(
        "Cannot complete the step because resource is not found, with resource type \
         MulticastGroup, resource Id {id}"
    );
    assert_eq!(
        answered.body["Message"].as_str(),
        Some(expected_message.as_str()),
        "case {case:?}: expected the not-found sentence from {operation}, got {}",
        answered.body
    );
}

/// Deleting a group is observable through every operation that addresses it: the read,
/// the patch and a second delete are all the modelled 404, while the groups that were
/// not deleted keep resolving unchanged.
#[tokio::test]
async fn deleting_a_group_is_observable_by_the_next_read() {
    let router = router();

    // The group bodies the table deletes, so the delete is exercised over a group with
    // every member set, one with only the required member, and one that was patched
    // first.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("full", seed_body("delete-full"), false),
        ("minimal", json!({ "LoRaWAN": {} }), false),
        ("patched-first", seed_body("delete-patched"), true),
    ];

    // One group the table never deletes: nothing about a delete may reach it.
    let survivor = seed_group(&router, seed_body("survivor")).await;
    let (_, survivor_before) = get_group(&router, &survivor).await;

    for (case, body, patch_first) in cases {
        let id = seed_group(&router, body).await;
        if patch_first {
            let (status, answered) =
                patch_group(&router, &id, json!({ "Description": "patched then deleted" })).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: expected 204 from the patch, got {status} {answered}"
            );
        }

        let (status, answered) = delete_group(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from DeleteMulticastGroup, got {status} {answered}"
        );
        assert!(
            answered.as_object().is_some_and(|body| body.is_empty())
                || answered == Value::Null,
            "case {case:?}: a 204 carries no members, got {answered}"
        );

        let read = call_with_headers(
            &router,
            Method::GET,
            &format!("/multicast-groups/{id}"),
            None,
        )
        .await;
        assert_not_found(case, "GetMulticastGroup", &id, &read);

        let patched = call_with_headers(
            &router,
            Method::PATCH,
            &format!("/multicast-groups/{id}"),
            Some(json!({ "Description": "after the delete" })),
        )
        .await;
        assert_not_found(case, "UpdateMulticastGroup", &id, &patched);

        let deleted_again = call_with_headers(
            &router,
            Method::DELETE,
            &format!("/multicast-groups/{id}"),
            None,
        )
        .await;
        assert_not_found(case, "DeleteMulticastGroup", &id, &deleted_again);
    }

    let (status, survivor_after) = get_group(&router, &survivor).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the group nothing deleted must still resolve, got {status} {survivor_after}"
    );
    assert_eq!(
        survivor_after, survivor_before,
        "the group nothing deleted must read back unchanged, got {survivor_after}"
    );
}

/// An `Id` the store does not hold — whatever it looks like — is refused by the patch
/// and the delete exactly as the read refuses it, and the refusal creates nothing.
#[tokio::test]
async fn an_unknown_id_is_refused_by_the_read_the_patch_and_the_delete() {
    let router = router();
    let present = seed_group(&router, seed_body("present")).await;
    let (_, before) = get_group(&router, &present).await;

    for id in [
        "00000000-0000-4000-8000-000000000000".to_string(),
        "not-a-uuid".to_string(),
        "A".to_string(),
        present.to_uppercase(),
        format!("{present}x"),
    ] {
        for (operation, method, body) in [
            ("GetMulticastGroup", Method::GET, None),
            (
                "UpdateMulticastGroup",
                Method::PATCH,
                Some(json!({ "Description": "nothing to patch" })),
            ),
            ("DeleteMulticastGroup", Method::DELETE, None),
        ] {
            let answered = call_with_headers(
                &router,
                method,
                &format!("/multicast-groups/{id}"),
                body,
            )
            .await;
            assert_not_found(&id, operation, &id, &answered);
        }
    }

    let (status, after) = get_group(&router, &present).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored group must still resolve after the refusals, got {status} {after}"
    );
    assert_eq!(
        after, before,
        "a refused call must change nothing, got {after}"
    );
}

/// What the group held goes with it: the memberships devices had in the deleted group
/// are released, so no filtered list still selects them for it, and the idempotency
/// token that minted it is free again, so replaying that token mints a NEW group rather
/// than replaying one the account no longer holds.
#[tokio::test]
async fn a_deleted_group_releases_its_devices_and_its_idempotency_token() {
    let router = router();
    seed_destination(&router, "MulticastTeardown").await;

    let token = "teardown-token";
    let mut create = seed_body("teardown");
    create["ClientRequestToken"] = json!(token);
    let id = seed_group(&router, create.clone()).await;
    let kept = seed_group(&router, seed_body("teardown-kept")).await;

    let mut devices = Vec::new();
    for index in 0..3 {
        let device = seed_device(
            &router,
            json!({
                "Type": "LoRaWAN",
                "Name": format!("TeardownDevice{index}"),
                "DestinationName": "MulticastTeardown",
            }),
        )
        .await;
        // Two devices join the group that is deleted, one joins the group that stays.
        let group = if index < 2 { &id } else { &kept };
        let (status, answered) = associate(&router, group, &device).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {index}: expected 204 from the association, got {status} {answered}"
        );
        devices.push(device);
    }

    let mut joined = vec![devices[0].clone(), devices[1].clone()];
    joined.sort();
    assert_eq!(
        member_ids(&router, &id, 0).await,
        joined,
        "the two joined devices must be selected for the group before it is deleted"
    );

    let (status, answered) = delete_group(&router, &id).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "expected 204 from DeleteMulticastGroup, got {status} {answered}"
    );

    assert!(
        member_ids(&router, &id, 0).await.is_empty(),
        "a device cannot stay a member of a group the account no longer holds, got {:?}",
        member_ids(&router, &id, 0).await
    );
    assert_eq!(
        member_ids(&router, &kept, 0).await,
        vec![devices[2].clone()],
        "the other group's membership must be untouched, got {:?}",
        member_ids(&router, &kept, 0).await
    );
    // The devices themselves survive: only their membership in the deleted group went.
    let (status, listed) = call(&router, Method::GET, "/wireless-devices?maxResults=0", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "expected 200 from ListWirelessDevices, got {status} {listed}"
    );
    let mut listed_ids: Vec<String> = listed["WirelessDeviceList"]
        .as_array()
        .unwrap_or_else(|| panic!("WirelessDeviceList must be a list, got {listed}"))
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    listed_ids.sort();
    let mut expected_ids = devices.clone();
    expected_ids.sort();
    assert_eq!(
        listed_ids, expected_ids,
        "every device must survive the group's delete, got {listed_ids:?}"
    );

    // The token no longer names a stored group, so the same create is a fresh one.
    let (status, replayed) = call(
        &router,
        Method::POST,
        "/multicast-groups",
        Some(create.clone()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "replaying the token of a deleted group must create a new group, got {status} {replayed}"
    );
    let replayed_id = replayed["Id"].as_str().expect("an Id");
    assert_ne!(
        replayed_id, id,
        "the replay must mint a NEW Id, got the deleted group's {id}"
    );
    let (status, fetched) = get_group(&router, replayed_id).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the group the replay minted must read back, got {status} {fetched}"
    );
    assert!(
        member_ids(&router, replayed_id, 0).await.is_empty(),
        "a freshly minted group inherits no membership, got {:?}",
        member_ids(&router, replayed_id, 0).await
    );
}

/// The group's downlink session plan is COMPLETED from the region it names: a member the
/// write supplied is kept as submitted, a member it left out takes the region's downlink
/// channel, and a group that names no region has no channel to take and gets nothing
/// invented for it. Asserted over every modelled region and both presence states of
/// both members.
#[tokio::test]
async fn the_session_plan_is_completed_from_the_group_rf_region() {
    let router = router();

    // The table below answers for the WHOLE modelled catalog, not a few rows of it.
    for region in RF_REGIONS {
        assert!(
            REGION_DOWNLINK_CHANNEL
                .iter()
                .any(|(name, _, _)| name == region),
            "case {region:?}: the modelled region has no downlink channel in this table"
        );
    }

    for (region, default_dr, default_freq) in REGION_DOWNLINK_CHANNEL {
        assert!(
            RF_REGIONS.contains(region),
            "case {region:?}: not in the modelled catalog {RF_REGIONS:?}"
        );

        // (case, the DefaultSessionParameters the create carries, the plan it must read
        // back as)
        let cases: Vec<(&str, Option<Value>, (i64, i64))> = vec![
            ("no session parameters", None, (*default_dr, *default_freq)),
            (
                "an empty session plan",
                Some(json!({})),
                (*default_dr, *default_freq),
            ),
            (
                "only the data rate",
                Some(json!({ "DlDr": DL_DR_MAX })),
                (DL_DR_MAX, *default_freq),
            ),
            (
                "only the frequency",
                Some(json!({ "DlFreq": DL_FREQ_MIN })),
                (*default_dr, DL_FREQ_MIN),
            ),
            (
                "the whole plan",
                Some(json!({ "DlDr": DL_DR_MIN, "DlFreq": DL_FREQ_MAX })),
                (DL_DR_MIN, DL_FREQ_MAX),
            ),
        ];

        for (case, supplied, (expected_dr, expected_freq)) in cases {
            let case = format!("{region}/{case}");
            let mut lorawan = json!({ "RfRegion": region });
            if let Some(supplied) = &supplied {
                lorawan["DefaultSessionParameters"] = supplied.clone();
            }
            let id = seed_group(&router, json!({ "LoRaWAN": lorawan })).await;
            let (status, fetched) = get_group(&router, &id).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from the read, got {status} {fetched}"
            );
            let plan = fetched["LoRaWAN"]["DefaultSessionParameters"].clone();
            assert_eq!(
                plan["DlDr"].as_i64(),
                Some(expected_dr),
                "case {case:?}: expected DlDr {expected_dr}, got {plan}"
            );
            assert_eq!(
                plan["DlFreq"].as_i64(),
                Some(expected_freq),
                "case {case:?}: expected DlFreq {expected_freq}, got {plan}"
            );
            // Whatever the plan ends up being, it is inside the published ranges.
            assert!(
                (DL_DR_MIN..=DL_DR_MAX).contains(&expected_dr)
                    && (DL_FREQ_MIN..=DL_FREQ_MAX).contains(&expected_freq),
                "case {case:?}: the plan {plan} is outside the published ranges"
            );
            // Reading again answers the same plan: it was assigned by the write, not
            // computed afresh for each reader.
            let (_, again) = get_group(&router, &id).await;
            assert_eq!(
                again["LoRaWAN"]["DefaultSessionParameters"], plan,
                "case {case:?}: a second read must answer the same plan, got {:?}",
                again["LoRaWAN"]["DefaultSessionParameters"]
            );
        }
    }

    // No region named, so there is no regional channel to complete from: what the
    // request carried is all the group has.
    for (case, lorawan, expected) in [
        ("no region, no plan", json!({}), Value::Null),
        (
            "no region, half a plan",
            json!({ "DefaultSessionParameters": { "DlDr": 3 } }),
            json!({ "DlDr": 3 }),
        ),
    ] {
        let id = seed_group(&router, json!({ "LoRaWAN": lorawan })).await;
        let (_, fetched) = get_group(&router, &id).await;
        let plan = fetched["LoRaWAN"]
            .get("DefaultSessionParameters")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            plan, expected,
            "case {case:?}: expected {expected}, got {plan}"
        );
    }
}

/// A patch is completed by the same rule: supplying half a session plan leaves the group
/// with a whole one, and the members the patch supplied are the ones it sent.
#[tokio::test]
async fn a_patch_that_half_supplies_the_session_plan_is_completed_too() {
    let router = router();

    for (region, default_dr, default_freq) in REGION_DOWNLINK_CHANNEL {
        // (case, the LoRaWAN block the patch carries, the plan the group must read back)
        let cases: Vec<(&str, Value, (i64, i64))> = vec![
            (
                "only the data rate",
                json!({ "DefaultSessionParameters": { "DlDr": DL_DR_MAX } }),
                (DL_DR_MAX, *default_freq),
            ),
            (
                "only the frequency",
                json!({ "DefaultSessionParameters": { "DlFreq": DL_FREQ_MAX } }),
                (*default_dr, DL_FREQ_MAX),
            ),
            (
                "an emptied plan",
                json!({ "DefaultSessionParameters": {} }),
                (*default_dr, *default_freq),
            ),
            (
                "the region alone",
                json!({ "RfRegion": region }),
                (DL_DR_MIN, DL_FREQ_MIN),
            ),
        ];

        for (case, patch, (expected_dr, expected_freq)) in cases {
            let case = format!("{region}/{case}");
            // The group starts with a plan at the bottom of both published ranges, so a
            // completed member is distinguishable from the stored one.
            let id = seed_group(
                &router,
                json!({
                    "LoRaWAN": {
                        "RfRegion": region,
                        "DefaultSessionParameters": { "DlDr": DL_DR_MIN, "DlFreq": DL_FREQ_MIN },
                    }
                }),
            )
            .await;
            let (status, answered) = patch_group(&router, &id, json!({ "LoRaWAN": patch })).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: expected 204 from the patch, got {status} {answered}"
            );
            let (_, fetched) = get_group(&router, &id).await;
            let plan = fetched["LoRaWAN"]["DefaultSessionParameters"].clone();
            assert_eq!(
                (plan["DlDr"].as_i64(), plan["DlFreq"].as_i64()),
                (Some(expected_dr), Some(expected_freq)),
                "case {case:?}: expected the plan ({expected_dr}, {expected_freq}), got {plan}"
            );
        }
    }
}


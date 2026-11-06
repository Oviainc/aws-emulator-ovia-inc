// SPDX-License-Identifier: CC0-1.0

//! Removing wireless devices from multicast groups, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded request:
//! `DisassociateWirelessDeviceFromMulticastGroup` releases the membership the request
//! names — and only that one — so every reader of the CURRENT membership answers from
//! the store afterwards: the group's `NumberOfDevicesInGroup` drops to exactly the
//! devices still joined, `ListWirelessDevices?multicastGroupId=` stops selecting the
//! device, and the device is free to join a group again. Releasing a membership does not
//! unmake the request that created it, so `NumberOfDevicesRequested` counts the devices
//! whose association with the group was asked for and does not fall when one leaves; it
//! falls only when the device itself is gone. Removal is idempotent, a membership the
//! request does not name is left exactly as it stands, and either end of the pair the
//! store does not hold is a 404 that persists nothing.
//!
//! The cases vary the device type and its optional technology record, how many devices a
//! group holds (one, several), which member of a group is removed and in what order, the
//! page size the filtered list is walked at, and how many times the removal is repeated.
//!
//! The wire harness and the multicast seeding helpers are the ones the sibling
//! association suite already has: `tests/all/*.rs` are modules of ONE test target, so
//! `router`, `call`, `error_code` and the `seed_*` helpers are shared from there rather
//! than copied.

use http::{Method, StatusCode};
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};
use super::multicast_group_device_association::{
    associate, device_bodies, group_counts, member_ids, seed_destination, seed_device, seed_group,
};

/// `SupportedRfRegion`'s modelled values — the model-generated constant, so the suite
/// and the service cannot drift apart.
const RF_REGIONS: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::SUPPORTED_RF_REGION_VALUES;

/// `DlClass`'s modelled values, from the same generated source.
const DL_CLASSES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::DL_CLASS_VALUES;

/// `DisassociateWirelessDeviceFromMulticastGroup` over the wire: the modelled route is
/// `DELETE /multicast-groups/{Id}/wireless-devices/{WirelessDeviceId}`.
async fn disassociate(
    router: &MultiServiceRouter,
    group_id: &str,
    device_id: &str,
) -> (StatusCode, serde_json::Value) {
    call(
        router,
        Method::DELETE,
        &format!("/multicast-groups/{group_id}/wireless-devices/{device_id}"),
        None,
    )
    .await
}

/// `DeleteWirelessDevice` over the wire, so a case can check what the group reports once
/// the device itself is gone.
async fn delete_device(
    router: &MultiServiceRouter,
    device_id: &str,
) -> (StatusCode, serde_json::Value) {
    call(
        router,
        Method::DELETE,
        &format!("/wireless-devices/{device_id}"),
        None,
    )
    .await
}

/// One group in the plan: how it is created, which seeded devices join it, and the
/// order its members are removed in.
struct GroupCase {
    case: &'static str,
    body: serde_json::Value,
    /// Indexes into the seeded device table.
    members: &'static [usize],
    /// The members to remove, in the order they are removed.
    remove_order: &'static [usize],
}

/// Releasing a membership is what the group's `NumberOfDevicesInGroup` and the filtered
/// list report next, while the number requested stands, over a table of groups holding
/// one and several devices of both modelled types, with the members removed in different
/// orders and the list walked at every page size.
#[tokio::test]
async fn a_disassociated_device_leaves_its_group_membership_and_filtered_list() {
    let router = router();
    seed_destination(&router, "McDisassocDest").await;

    let devices = device_bodies("McDisassocDest");
    let mut device_ids = Vec::new();
    for (case, body) in &devices {
        device_ids.push((*case, seed_device(&router, body.clone()).await));
    }

    let groups = [
        GroupCase {
            case: "one-member-removed",
            body: json!({
                "Name": "McDisassocOne",
                "LoRaWAN": { "RfRegion": RF_REGIONS[0], "DlClass": DL_CLASSES[0] },
            }),
            members: &[0],
            remove_order: &[0],
        },
        GroupCase {
            case: "three-members-removed-out-of-order",
            body: json!({
                "Name": "McDisassocThree",
                "Description": "two LoRaWAN devices and a Sidewalk device",
                "LoRaWAN": {
                    "RfRegion": RF_REGIONS[RF_REGIONS.len() - 1],
                    "DlClass": DL_CLASSES[DL_CLASSES.len() - 1],
                },
            }),
            members: &[1, 2, 3],
            remove_order: &[3, 1, 2],
        },
    ];

    for group in &groups {
        let group_id = seed_group(&router, group.body.clone()).await;
        for member in group.members {
            let (status, body) = associate(&router, &group_id, &device_ids[*member].1).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {:?} device {:?}: seeding the membership expected 204, got {status} {body}",
                group.case,
                device_ids[*member].0
            );
        }
        // Every member was requested AND is joined, so both counts start at the size of
        // the group.
        let requested = group.members.len() as i64;
        assert_eq!(
            group_counts(&router, &group_id).await,
            (requested, requested),
            "case {:?}: the group must count every device joined to it",
            group.case
        );

        let mut still_in: Vec<usize> = group.members.to_vec();
        for (removal, member) in group.remove_order.iter().enumerate() {
            let (status, body) = disassociate(&router, &group_id, &device_ids[*member].1).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {:?} device {:?}: expected 204, got {status} {body}",
                group.case,
                device_ids[*member].0
            );
            assert!(
                body.as_object().is_none_or(|members| members.is_empty()),
                "case {:?}: a 204 carries no response members, got {body}",
                group.case
            );

            still_in.retain(|held| held != member);
            let expected = {
                let mut ids: Vec<String> = still_in
                    .iter()
                    .map(|held| device_ids[*held].1.clone())
                    .collect();
                ids.sort();
                ids
            };
            let in_group = expected.len() as i64;
            assert_eq!(
                group_counts(&router, &group_id).await,
                (requested, in_group),
                "case {:?} after {} removal(s): expected {requested} requested and {in_group} \
                 in the group",
                group.case,
                removal + 1
            );
            // Page size 0 is "no caller-supplied page size"; 1 and 2 force the filter to
            // be applied before paging.
            for max_results in [0, 1, 2, 250] {
                let selected = member_ids(&router, &group_id, max_results).await;
                assert_eq!(
                    selected, expected,
                    "case {:?} after {} removal(s) maxResults={max_results}: wrong selection, \
                     got {selected:?}",
                    group.case,
                    removal + 1
                );
            }

            // The device itself is still stored — only its membership was released — so
            // the unfiltered list still reports it.
            let (status, all) = call(&router, Method::GET, "/wireless-devices", None).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {:?}: listing every device expected 200, got {status} {all}",
                group.case
            );
            let listed: Vec<String> = all["WirelessDeviceList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                listed.contains(&device_ids[*member].1),
                "case {:?} device {:?}: the removal must not delete the device, got {listed:?}",
                group.case,
                device_ids[*member].0
            );
        }

        assert_eq!(
            group_counts(&router, &group_id).await,
            (requested, 0),
            "case {:?}: a group every member left must hold no device and still report \
             {requested} requested",
            group.case
        );
    }
}

/// A released membership frees the device: the single-valued membership is gone, so the
/// device joins another group, and releasing that one lets it re-join the first.
#[tokio::test]
async fn a_disassociated_device_can_join_another_group() {
    let router = router();
    seed_destination(&router, "McRejoinDest").await;
    let first = seed_group(
        &router,
        json!({ "Name": "McRejoinFirst", "LoRaWAN": { "RfRegion": RF_REGIONS[0] } }),
    )
    .await;
    let second = seed_group(&router, json!({ "LoRaWAN": { "DlClass": DL_CLASSES[0] } })).await;

    let mut seen = 0i64;
    for (case, body) in device_bodies("McRejoinDest") {
        let device_id = seed_device(&router, body).await;
        seen += 1;

        // Walking first -> second -> first proves the membership is really released,
        // rather than the second association being accepted by accident.
        for (from, to) in [(&first, &second), (&second, &first)] {
            let (status, answered) = associate(&router, from, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: joining {from} expected 204, got {status} {answered}"
            );
            let (status, answered) = disassociate(&router, from, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: leaving {from} expected 204, got {status} {answered}"
            );
            assert!(
                !member_ids(&router, from, 0).await.contains(&device_id),
                "case {case:?}: group {from} must no longer hold the device"
            );
            let (status, answered) = associate(&router, to, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: joining {to} after leaving {from} expected 204, got {status} \
                 {answered}"
            );
            assert!(
                member_ids(&router, to, 0).await.contains(&device_id),
                "case {case:?}: group {to} must now hold the device"
            );
            let (status, answered) = disassociate(&router, to, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: leaving {to} expected 204, got {status} {answered}"
            );
        }

        // Each group was asked for every device seen so far, and holds none of them.
        for (label, group_id) in [("first", &first), ("second", &second)] {
            assert_eq!(
                group_counts(&router, group_id).await,
                (seen, 0),
                "case {case:?}: the {label} group must report {seen} requested and hold none"
            );
        }
    }
}

/// Releasing a membership is idempotent, and it releases only the membership the request
/// names: a device that never joined, and a device that belongs to another group, are
/// both the same empty success with the stored memberships untouched.
#[tokio::test]
async fn removing_a_membership_that_is_not_held_changes_nothing() {
    let router = router();
    seed_destination(&router, "McIdemDisassocDest").await;
    let held = seed_group(
        &router,
        json!({ "Name": "McHeld", "LoRaWAN": { "RfRegion": RF_REGIONS[0] } }),
    )
    .await;
    let other = seed_group(&router, json!({ "LoRaWAN": {} })).await;

    let mut requested = 0i64;
    for (case, body) in device_bodies("McIdemDisassocDest") {
        let device_id = seed_device(&router, body).await;

        // (1) A device in no group at all: nothing to release, for either group, and
        // nothing was ever requested of either.
        for group_id in [&held, &other] {
            for attempt in 1..=3 {
                let (status, answered) = disassociate(&router, group_id, &device_id).await;
                assert_eq!(
                    status,
                    StatusCode::NO_CONTENT,
                    "case {case:?} unjoined attempt {attempt}: expected 204, got {status} \
                     {answered}"
                );
            }
        }
        assert_eq!(
            group_counts(&router, &other).await,
            (0, 0),
            "case {case:?}: releasing a membership nobody asked for must count nothing"
        );

        let (status, answered) = associate(&router, &held, &device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: the membership must be established, got {status} {answered}"
        );
        requested += 1;

        // (2) A membership the request does not name: releasing the device from the OTHER
        // group leaves the one it really holds in place.
        for attempt in 1..=3 {
            let (status, answered) = disassociate(&router, &other, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?} other-group attempt {attempt}: expected 204, got {status} \
                 {answered}"
            );
        }
        assert!(
            member_ids(&router, &held, 0).await.contains(&device_id),
            "case {case:?}: the membership the request did not name must stay in place"
        );
        assert_eq!(
            group_counts(&router, &held).await,
            (requested, 1),
            "case {case:?}: the group the device holds must still hold exactly it"
        );
        assert_eq!(
            group_counts(&router, &other).await,
            (0, 0),
            "case {case:?}: the group the device never joined must count nothing"
        );

        // (3) Repeating the release of a membership that WAS held: the first call releases
        // it, every later one is the same success and nothing moves.
        for attempt in 1..=3 {
            let (status, answered) = disassociate(&router, &held, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?} repeat attempt {attempt}: expected 204, got {status} {answered}"
            );
            assert!(
                !member_ids(&router, &held, 0).await.contains(&device_id),
                "case {case:?} repeat attempt {attempt}: the membership must stay released"
            );
            assert_eq!(
                group_counts(&router, &held).await,
                (requested, 0),
                "case {case:?} repeat attempt {attempt}: repeated releases must not move the \
                 counts"
            );
        }
    }
}

/// The requested count follows the devices the store holds: a device that is deleted was
/// not requested to join anything, so the group stops counting it either way.
#[tokio::test]
async fn deleting_a_device_drops_it_from_the_requested_count() {
    let router = router();
    seed_destination(&router, "McDeleteDest").await;
    let group_id = seed_group(
        &router,
        json!({ "Name": "McDeleted", "LoRaWAN": { "RfRegion": RF_REGIONS[0] } }),
    )
    .await;

    // Each case varies whether the membership is released before the device is deleted:
    // either way the deleted device is counted by nothing afterwards.
    for (index, (case, body)) in device_bodies("McDeleteDest").into_iter().enumerate() {
        let release_first = index % 2 == 0;
        let device_id = seed_device(&router, body).await;
        let (status, answered) = associate(&router, &group_id, &device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: the membership must be established, got {status} {answered}"
        );
        assert_eq!(
            group_counts(&router, &group_id).await,
            (1, 1),
            "case {case:?}: the group must count the device it was asked for and holds"
        );

        if release_first {
            let (status, answered) = disassociate(&router, &group_id, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: releasing the membership expected 204, got {status} {answered}"
            );
            assert_eq!(
                group_counts(&router, &group_id).await,
                (1, 0),
                "case {case:?}: a released membership leaves the request counted"
            );
        }

        let (status, answered) = delete_device(&router, &device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: deleting the device expected 204, got {status} {answered}"
        );
        assert_eq!(
            group_counts(&router, &group_id).await,
            (0, 0),
            "case {case:?}: a deleted device must be counted by no group"
        );
        assert!(
            member_ids(&router, &group_id, 0).await.is_empty(),
            "case {case:?}: a deleted device must be selected by no filtered list"
        );
    }
}

/// Both ends of the membership are resolved against real state: an `Id` or a
/// `WirelessDeviceId` the store does not hold is a 404 that persists nothing.
#[tokio::test]
async fn an_absent_group_or_device_is_not_found_on_removal() {
    let router = router();
    seed_destination(&router, "McMissingDisassocDest").await;
    let group_id = seed_group(&router, json!({ "LoRaWAN": {} })).await;
    let device_id = seed_device(
        &router,
        json!({ "Type": "LoRaWAN", "DestinationName": "McMissingDisassocDest" }),
    )
    .await;
    let (status, answered) = associate(&router, &group_id, &device_id).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "seeding the membership expected 204, got {status} {answered}"
    );

    // (case, the group the call names, the device it names)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "absent group, real device",
            "00000000-0000-4000-8000-000000000000".to_string(),
            device_id.clone(),
        ),
        (
            "group id in the wrong case",
            group_id.to_uppercase(),
            device_id.clone(),
        ),
        (
            "real group, absent device",
            group_id.clone(),
            "11111111-1111-4111-8111-111111111111".to_string(),
        ),
        (
            "real group, device id in the wrong case",
            group_id.clone(),
            device_id.to_uppercase(),
        ),
        (
            "real group, device id that is not a uuid",
            group_id.clone(),
            "not-a-device".to_string(),
        ),
        (
            "neither end exists",
            "mc-nope".to_string(),
            "wd-nope".to_string(),
        ),
    ];

    for (case, group, device) in cases {
        let (status, answered) = disassociate(&router, &group, &device).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {case:?}: expected 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered}"
        );
        assert_eq!(
            group_counts(&router, &group_id).await,
            (1, 1),
            "case {case:?}: a refused removal must persist nothing"
        );
        assert_eq!(
            member_ids(&router, &group_id, 0).await,
            vec![device_id.clone()],
            "case {case:?}: a refused removal must leave the membership in place"
        );
    }

    // The real pair still disassociates after every refusal.
    let (status, answered) = disassociate(&router, &group_id, &device_id).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the real pair must still disassociate, got {status} {answered}"
    );
    assert!(
        member_ids(&router, &group_id, 0).await.is_empty(),
        "the group must no longer hold the device"
    );
    assert_eq!(
        group_counts(&router, &group_id).await,
        (1, 0),
        "the group must still report the request it served and hold no device"
    );
}

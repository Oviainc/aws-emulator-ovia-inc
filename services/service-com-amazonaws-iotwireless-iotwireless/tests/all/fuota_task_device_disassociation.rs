// SPDX-License-Identifier: CC0-1.0

//! Removing a wireless device from a FUOTA task, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded request:
//! `DisassociateWirelessDeviceFromFuotaTask` drops exactly the stored membership it
//! names, so every reader that selects on that membership
//! (`ListWirelessDevices?fuotaTaskId=`) stops reporting the device and stops reporting
//! its `FuotaDeviceStatus`, while every other device's membership, and a membership the
//! call does not name, is left as it stands. The cases vary the device type, how many
//! devices the task holds, which member is removed, the page size the list is walked
//! at, and how many times the removal is repeated.
//!
//! The refusals and the idempotence belong to the same rule: both ends are resolved
//! against real state (an absent task or device is a 404 that persists nothing), and a
//! membership that is already gone — or that was never there — is the same empty
//! success again.
//!
//! The wire harness is the one the sibling destination/device suite already has:
//! `tests/all/*.rs` are modules of ONE test target, so `router`, `call` and
//! `error_code` are shared from there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The FUOTA device status the service publishes for a device that belongs to a task,
/// so this suite asserts what the implementation declares rather than a string copied
/// by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::FUOTA_DEVICE_STATUS_INITIAL;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// The two required members of a FUOTA task, reused by every case.
const IMAGE: &str = "s3://fuota-images/disassociation.bin";
const ROLE: &str = "arn:aws:iam::000000000000:role/iotwireless-fuota";

/// Create one destination, so the devices below have something to register against.
async fn seed_destination(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        Method::POST,
        "/destinations",
        Some(json!({
            "Name": name,
            "ExpressionType": "RuleName",
            "Expression": format!("rule-for-{name}"),
            "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding destination {name}: expected 201, got {status} {body}"
    );
}

/// Create one FUOTA task and return its `Id`.
async fn seed_task(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        Method::POST,
        "/fuota-tasks",
        Some(json!({
            "Name": name,
            "FirmwareUpdateImage": IMAGE,
            "FirmwareUpdateRole": ROLE,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding FUOTA task {name}: expected 201, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("seeding FUOTA task {name}: no Id in {created}"))
        .to_string()
}

/// Create one wireless device and return its `Id`.
async fn seed_device(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, Method::POST, "/wireless-devices", Some(body)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a wireless device: expected 201, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("seeding a wireless device: no Id in {created}"))
        .to_string()
}

/// `AssociateWirelessDeviceWithFuotaTask` over the wire.
async fn associate(
    router: &MultiServiceRouter,
    task_id: &str,
    device_id: &str,
) -> (StatusCode, Value) {
    call(
        router,
        Method::PUT,
        &format!("/fuota-tasks/{task_id}/wireless-device"),
        Some(json!({ "WirelessDeviceId": device_id })),
    )
    .await
}

/// `DisassociateWirelessDeviceFromFuotaTask` over the wire.
/// Route: `DELETE /fuota-tasks/{Id}/wireless-devices/{WirelessDeviceId}`.
async fn disassociate(
    router: &MultiServiceRouter,
    task_id: &str,
    device_id: &str,
) -> (StatusCode, Value) {
    call(
        router,
        Method::DELETE,
        &format!("/fuota-tasks/{task_id}/wireless-devices/{device_id}"),
        None,
    )
    .await
}

/// Walk every page of `ListWirelessDevices?fuotaTaskId=<task_id>` at `max_results` and
/// return the entries it served, in order.
async fn list_for_task(router: &MultiServiceRouter, task_id: &str, max_results: i64) -> Vec<Value> {
    let mut entries: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut uri = format!("/wireless-devices?fuotaTaskId={task_id}&maxResults={max_results}");
        if let Some(token) = &token {
            uri.push_str(&format!("&nextToken={token}"));
        }
        let (status, body) = call(router, Method::GET, &uri, None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case fuotaTaskId={task_id} maxResults={max_results}: expected 200, got {status} \
             {body}"
        );
        let page = body["WirelessDeviceList"]
            .as_array()
            .unwrap_or_else(|| panic!("WirelessDeviceList must be a list, got {body}"))
            .clone();
        entries.extend(page);
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    entries
}

/// The `Id`s of a list page, sorted, so a selection is compared as a set.
fn sorted_ids(entries: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = entries
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// The devices every case shares: both modelled `WirelessDeviceType`s, with and without
/// the optional LoRaWAN record, so the membership never depends on which optional record
/// a device carries.
fn device_bodies(destination: &str, prefix: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "lorawan-with-dev-eui",
            json!({
                "Type": "LoRaWAN",
                "Name": format!("{prefix}-lorawan-1"),
                "DestinationName": destination,
                "LoRaWAN": { "DevEui": format!("{:016x}", 0xd1_u64), "DeviceProfileId": "dp-dis" },
            }),
        ),
        (
            "lorawan-bare",
            json!({ "Type": "LoRaWAN", "DestinationName": destination }),
        ),
        (
            "sidewalk",
            json!({
                "Type": "Sidewalk",
                "Name": format!("{prefix}-sidewalk"),
                "DestinationName": destination,
                "Sidewalk": { "DeviceProfileId": "dp-sidewalk" },
            }),
        ),
        (
            "lorawan-named-only",
            json!({
                "Type": "LoRaWAN",
                "Name": format!("{prefix}-lorawan-2"),
                "DestinationName": destination,
                "LoRaWAN": { "DevEui": format!("{:016x}", 0xd2_u64) },
            }),
        ),
    ]
}

/// One case of the removal table: how many of the seeded devices the task holds, and
/// which of those the call removes.
struct RemovalCase {
    case: &'static str,
    /// Indexes into the seeded device table.
    members: &'static [usize],
    /// Indexes (into the same table) the disassociate calls name, in order.
    removed: &'static [usize],
}

/// A removal is observable through the filtered list, and only the named membership
/// goes: over tasks holding one, two and four devices of both modelled types, removing
/// the first, the last or every member, read back at every page size.
#[tokio::test]
async fn a_disassociated_device_leaves_the_filtered_list() {
    let cases = [
        RemovalCase {
            case: "the only member",
            members: &[0],
            removed: &[0],
        },
        RemovalCase {
            case: "the first of two",
            members: &[1, 2],
            removed: &[1],
        },
        RemovalCase {
            case: "the last of two",
            members: &[2, 3],
            removed: &[3],
        },
        RemovalCase {
            case: "every member of four",
            members: &[0, 1, 2, 3],
            removed: &[0, 1, 2, 3],
        },
        RemovalCase {
            case: "nothing removed",
            members: &[0, 3],
            removed: &[],
        },
    ];

    for case in &cases {
        // One store per case, so each case's membership table stands on its own.
        let router = router();
        seed_destination(&router, "DisDest").await;
        let task_id = seed_task(&router, "Removal").await;
        let mut device_ids = Vec::new();
        for (name, body) in device_bodies("DisDest", "dis") {
            device_ids.push((name, seed_device(&router, body).await));
        }

        for member in case.members {
            let (status, body) = associate(&router, &task_id, &device_ids[*member].1).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {:?} device {:?}: seeding the membership expected 204, got {status} {body}",
                case.case,
                device_ids[*member].0
            );
        }

        for member in case.removed {
            let (status, body) = disassociate(&router, &task_id, &device_ids[*member].1).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {:?} device {:?}: expected 204, got {status} {body}",
                case.case,
                device_ids[*member].0
            );
            assert!(
                body.as_object().is_none_or(|members| members.is_empty()),
                "case {:?}: a 204 carries no response members, got {body}",
                case.case
            );
        }

        let expected: Vec<String> = {
            let mut ids: Vec<String> = case
                .members
                .iter()
                .filter(|member| !case.removed.contains(member))
                .map(|member| device_ids[*member].1.clone())
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };
        // Page size 0 is "no caller-supplied page size"; 1 and 2 force the filter to be
        // applied before paging; 250 is the published maximum.
        for max_results in [0, 1, 2, 250] {
            let entries = list_for_task(&router, &task_id, max_results).await;
            assert_eq!(
                sorted_ids(&entries),
                expected,
                "case {:?} maxResults={max_results}: wrong selection, got {:?}",
                case.case,
                sorted_ids(&entries)
            );
            for entry in &entries {
                assert_eq!(
                    entry["FuotaDeviceStatus"].as_str(),
                    Some(FUOTA_DEVICE_STATUS_INITIAL),
                    "case {:?}: a device still in the task must report \
                     {FUOTA_DEVICE_STATUS_INITIAL}, got {:?}",
                    case.case,
                    entry.get("FuotaDeviceStatus")
                );
            }
        }

        // A removed device is still a device: the unfiltered list reports it, now with
        // no FUOTA status at all.
        let (_, all) = call(&router, Method::GET, "/wireless-devices", None).await;
        let listed = all["WirelessDeviceList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            sorted_ids(&listed),
            {
                let mut ids: Vec<String> = device_ids.iter().map(|(_, id)| id.clone()).collect();
                ids.sort();
                ids
            },
            "case {:?}: the unfiltered list must still report every device, got {:?}",
            case.case,
            sorted_ids(&listed)
        );
        for member in case.removed {
            let entry = listed
                .iter()
                .find(|entry| entry["Id"].as_str() == Some(device_ids[*member].1.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {:?}: the removed device must still be listed", case.case)
                });
            assert!(
                entry.get("FuotaDeviceStatus").is_none(),
                "case {:?} device {:?}: a device in no task has no FuotaDeviceStatus, got {:?}",
                case.case,
                device_ids[*member].0,
                entry.get("FuotaDeviceStatus")
            );
        }
    }
}

/// Removing a membership that is already gone — or that was never written — is the same
/// empty success again, for every device shape.
#[tokio::test]
async fn disassociating_a_device_that_is_not_a_member_is_idempotent() {
    let router = router();
    seed_destination(&router, "IdemDisDest").await;
    let task_id = seed_task(&router, "IdemRemoval").await;

    for (case, body) in device_bodies("IdemDisDest", "idem") {
        let device_id = seed_device(&router, body).await;

        // (case, how many memberships were written before the removals)
        for associations in [0_usize, 1] {
            for _ in 0..associations {
                let (status, answered) = associate(&router, &task_id, &device_id).await;
                assert_eq!(
                    status,
                    StatusCode::NO_CONTENT,
                    "case {case:?}: seeding the membership expected 204, got {status} {answered}"
                );
            }
            for attempt in 1..=3 {
                let (status, answered) = disassociate(&router, &task_id, &device_id).await;
                assert_eq!(
                    status,
                    StatusCode::NO_CONTENT,
                    "case {case:?} associations={associations} attempt {attempt}: expected 204, \
                     got {status} {answered}"
                );
            }
            let ids = sorted_ids(&list_for_task(&router, &task_id, 0).await);
            assert!(
                !ids.contains(&device_id),
                "case {case:?} associations={associations}: the device must be out of the task, \
                 got {ids:?}"
            );
        }

        // The membership can be written again after it was removed: the removal cleared
        // state, it did not retire the device.
        let (status, answered) = associate(&router, &task_id, &device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: re-associating after a removal expected 204, got {status} {answered}"
        );
        assert!(
            sorted_ids(&list_for_task(&router, &task_id, 0).await).contains(&device_id),
            "case {case:?}: the re-associated device must be back in the task"
        );
        let (status, answered) = disassociate(&router, &task_id, &device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: tidying the membership expected 204, got {status} {answered}"
        );
    }
}

/// A call removes only the membership it names: a device held by another task keeps that
/// task, and so does every other member of it.
#[tokio::test]
async fn disassociating_from_another_task_leaves_the_stored_membership() {
    let router = router();
    seed_destination(&router, "OtherDest").await;
    let holder = seed_task(&router, "Holder").await;
    let other = seed_task(&router, "Other").await;

    let mut device_ids = Vec::new();
    for (case, body) in device_bodies("OtherDest", "other") {
        device_ids.push((case, seed_device(&router, body).await));
    }
    for (case, device_id) in &device_ids {
        let (status, answered) = associate(&router, &holder, device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: seeding the membership expected 204, got {status} {answered}"
        );
    }

    for (case, device_id) in &device_ids {
        let (status, answered) = disassociate(&router, &other, device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: removing an unnamed membership expected 204, got {status} {answered}"
        );
        assert!(
            sorted_ids(&list_for_task(&router, &holder, 0).await).contains(device_id),
            "case {case:?}: the device must still be held by the task it joined"
        );
    }

    // Every membership survived, and the task the calls named holds nothing.
    let held = sorted_ids(&list_for_task(&router, &holder, 0).await);
    assert_eq!(
        held,
        {
            let mut ids: Vec<String> = device_ids.iter().map(|(_, id)| id.clone()).collect();
            ids.sort();
            ids
        },
        "every membership must survive a removal naming another task, got {held:?}"
    );
    let unnamed = sorted_ids(&list_for_task(&router, &other, 0).await);
    assert!(
        unnamed.is_empty(),
        "the task the removals named must hold nothing, got {unnamed:?}"
    );
}

/// Both ends are resolved against real state: an `Id` or a `WirelessDeviceId` the store
/// does not hold is a 404 that persists nothing.
#[tokio::test]
async fn an_absent_task_or_device_is_not_found() {
    let router = router();
    seed_destination(&router, "MissingDisDest").await;
    let task_id = seed_task(&router, "MissingRemoval").await;
    let device_id = seed_device(
        &router,
        json!({ "Type": "LoRaWAN", "DestinationName": "MissingDisDest" }),
    )
    .await;
    let (status, answered) = associate(&router, &task_id, &device_id).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "seeding the membership expected 204, got {status} {answered}"
    );

    // (case, the task the call names, the device it names)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "absent task, real device",
            "00000000-0000-4000-8000-000000000000".to_string(),
            device_id.clone(),
        ),
        (
            "task id in the wrong case",
            task_id.to_uppercase(),
            device_id.clone(),
        ),
        (
            "real task, absent device",
            task_id.clone(),
            "11111111-1111-4111-8111-111111111111".to_string(),
        ),
        (
            "real task, device id in the wrong case",
            task_id.clone(),
            device_id.to_uppercase(),
        ),
        (
            "real task, device id that is not a uuid",
            task_id.clone(),
            "not-a-device".to_string(),
        ),
        (
            "neither end exists",
            "ft-nope".to_string(),
            "wd-nope".to_string(),
        ),
    ];

    for (case, task, device) in cases {
        let (status, answered) = disassociate(&router, &task, &device).await;
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
        // A refused removal persists nothing: the membership it could not address is
        // still there.
        let ids = sorted_ids(&list_for_task(&router, &task_id, 0).await);
        assert_eq!(
            ids,
            vec![device_id.clone()],
            "case {case:?}: a refused removal must persist nothing, got {ids:?}"
        );
    }

    // The real pair still disassociates after every refusal.
    let (status, answered) = disassociate(&router, &task_id, &device_id).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the real pair must still disassociate, got {status} {answered}"
    );
    let ids = sorted_ids(&list_for_task(&router, &task_id, 0).await);
    assert!(
        ids.is_empty(),
        "the task must now hold nothing, got {ids:?}"
    );
}

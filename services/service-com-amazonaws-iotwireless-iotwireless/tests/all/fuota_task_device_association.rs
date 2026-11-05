// SPDX-License-Identifier: CC0-1.0

//! Joining wireless devices to FUOTA tasks, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded request:
//! `AssociateWirelessDeviceWithFuotaTask` persists the membership on the device it
//! names, so every reader that selects on that membership
//! (`ListWirelessDevices?fuotaTaskId=`) reports exactly the devices joined to that task
//! and the FUOTA status each one carries, while a device joined to no task stays out of
//! every filtered list. The cases vary the device type, how many devices a task holds
//! (none, one, several), the page size the list is walked at, and the order the calls
//! arrive in.
//!
//! The refusals belong to the same rule: both ends of the association are resolved
//! against real state (an absent task or device is a 404), the membership is
//! single-valued (a second task for the same device is a 409), and asking for a
//! membership that already holds is the same success again. A refused call persists
//! nothing.
//!
//! The wire harness is the one the sibling destination/device suite already has:
//! `tests/all/*.rs` are modules of ONE test target, so `router`, `call` and
//! `error_code` are shared from there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The FUOTA device status the service publishes for a device that has just joined a
/// task, so this suite asserts what the implementation declares rather than a string
/// copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::FUOTA_DEVICE_STATUS_INITIAL;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// `FuotaDeviceStatus`'s modelled values — the model-generated constant, so the suite
/// and the model cannot drift apart.
const FUOTA_DEVICE_STATUSES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::FUOTA_DEVICE_STATUS_VALUES;

/// The two required members of a FUOTA task, reused by every case.
const IMAGE: &str = "s3://fuota-images/association.bin";
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
async fn seed_task(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, Method::POST, "/fuota-tasks", Some(body)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a FUOTA task: expected 201, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("seeding a FUOTA task: no Id in {created}"))
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

/// Walk every page of `ListWirelessDevices?fuotaTaskId=<task_id>` at `max_results` and
/// return the entries it served, in order.
async fn list_for_task(
    router: &MultiServiceRouter,
    task_id: &str,
    max_results: i64,
) -> Vec<Value> {
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
        if max_results > 0 {
            assert!(
                page.len() as i64 <= max_results,
                "case fuotaTaskId={task_id} maxResults={max_results}: a page served {} \
                 entries",
                page.len()
            );
        }
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

/// One task in the plan: the body it is created with, and which of the seeded devices
/// are joined to it.
struct TaskCase {
    case: &'static str,
    body: Value,
    /// Indexes into the seeded device table.
    members: &'static [usize],
}

/// The devices every case shares: both modelled `WirelessDeviceType`s, with and without
/// the optional LoRaWAN record.
fn device_bodies(destination: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "lorawan-with-dev-eui",
            json!({
                "Type": "LoRaWAN",
                "Name": "assoc-lorawan-1",
                "DestinationName": destination,
                "LoRaWAN": { "DevEui": "0000000000000f01", "DeviceProfileId": "dp-assoc" },
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
                "Name": "assoc-sidewalk",
                "DestinationName": destination,
                "Sidewalk": { "DeviceProfileId": "dp-sidewalk" },
            }),
        ),
        (
            "lorawan-named-only",
            json!({
                "Type": "LoRaWAN",
                "Name": "assoc-lorawan-2",
                "DestinationName": destination,
                "LoRaWAN": { "DevEui": "0000000000000f02" },
            }),
        ),
    ]
}

/// A membership written by the associate call is what the filtered list reports, over a
/// table of tasks holding none, one and several devices of both modelled types, and at
/// every page size.
#[tokio::test]
async fn an_associated_device_is_reported_by_the_filtered_list() {
    let router = router();
    seed_destination(&router, "AssocDest").await;

    let devices = device_bodies("AssocDest");
    let mut device_ids = Vec::new();
    for (case, body) in &devices {
        device_ids.push((case, seed_device(&router, body.clone()).await));
    }

    let tasks = [
        TaskCase {
            case: "one-lorawan-member",
            body: json!({
                "Name": "OneMember",
                "LoRaWAN": { "RfRegion": "US915" },
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
            }),
            members: &[0],
        },
        TaskCase {
            case: "two-members-mixed-types",
            body: json!({
                "Name": "TwoMembers",
                "LoRaWAN": { "RfRegion": "EU868" },
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
            }),
            // A bare LoRaWAN device and a Sidewalk device, so the membership does not
            // depend on which optional record the device carries.
            members: &[1, 2],
        },
        TaskCase {
            case: "no-members",
            body: json!({
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
            }),
            members: &[],
        },
    ];

    let mut task_ids = Vec::new();
    for task in &tasks {
        let task_id = seed_task(&router, task.body.clone()).await;
        // Before anything joins it, a task selects nothing: the filter answers from the
        // devices' own records, not from the task's existence.
        let entries = list_for_task(&router, &task_id, 0).await;
        assert!(
            entries.is_empty(),
            "case {:?}: a task nothing joined must select no device, got {entries:?}",
            task.case
        );
        task_ids.push(task_id);
    }

    for (task, task_id) in tasks.iter().zip(&task_ids) {
        for member in task.members {
            let (status, body) = associate(&router, task_id, &device_ids[*member].1).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {:?} device {:?}: expected 204, got {status} {body}",
                task.case,
                device_ids[*member].0
            );
            assert!(
                body.as_object().is_none_or(|members| members.is_empty()),
                "case {:?}: a 204 carries no response members, got {body}",
                task.case
            );
        }
    }

    for (task, task_id) in tasks.iter().zip(&task_ids) {
        let expected: Vec<String> = {
            let mut ids: Vec<String> = task
                .members
                .iter()
                .map(|member| device_ids[*member].1.clone())
                .collect();
            ids.sort();
            ids
        };
        // Page size 0 is "no caller-supplied page size"; 1 and 2 force the filter to be
        // applied before paging.
        for max_results in [0, 1, 2, 250] {
            let entries = list_for_task(&router, task_id, max_results).await;
            assert_eq!(
                sorted_ids(&entries),
                expected,
                "case {:?} maxResults={max_results}: wrong selection, got {:?}",
                task.case,
                sorted_ids(&entries)
            );
            for entry in &entries {
                let status = entry["FuotaDeviceStatus"].as_str().unwrap_or_default();
                assert!(
                    FUOTA_DEVICE_STATUSES.contains(&status),
                    "case {:?}: {status:?} is not in the modelled catalog \
                     {FUOTA_DEVICE_STATUSES:?}",
                    task.case
                );
                assert_eq!(
                    status, FUOTA_DEVICE_STATUS_INITIAL,
                    "case {:?}: a device that just joined must report \
                     {FUOTA_DEVICE_STATUS_INITIAL}, got {status:?}",
                    task.case
                );
            }
        }
    }

    // A device joined to nothing stays out of every filtered list, and the unfiltered
    // list still reports every device exactly once.
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
        "the unfiltered list must still report every device, got {:?}",
        sorted_ids(&listed)
    );
    let unjoined = &device_ids[3];
    for entry in &listed {
        if entry["Id"].as_str() == Some(unjoined.1.as_str()) {
            assert!(
                entry.get("FuotaDeviceStatus").is_none(),
                "case {:?}: a device in no task has no FuotaDeviceStatus, got {:?}",
                unjoined.0,
                entry.get("FuotaDeviceStatus")
            );
        }
    }
}

/// Asking for a membership that already holds is the same success again, and leaves the
/// task holding the device exactly once.
#[tokio::test]
async fn re_associating_the_same_device_is_idempotent() {
    let router = router();
    seed_destination(&router, "IdemDest").await;
    let task_id = seed_task(
        &router,
        json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE }),
    )
    .await;

    for (case, body) in device_bodies("IdemDest") {
        let device_id = seed_device(&router, body).await;
        for attempt in 1..=3 {
            let (status, answered) = associate(&router, &task_id, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?} attempt {attempt}: expected 204, got {status} {answered}"
            );
        }
        let entries = list_for_task(&router, &task_id, 0).await;
        let ids = sorted_ids(&entries);
        assert_eq!(
            ids.iter().filter(|id| *id == &device_id).count(),
            1,
            "case {case:?}: the task must hold the device exactly once, got {ids:?}"
        );
    }
}

/// A device's FUOTA membership is single-valued: joining it to a second task is refused
/// and the membership already stored is left as it stands.
#[tokio::test]
async fn a_device_already_in_a_task_cannot_join_a_second_one() {
    let router = router();
    seed_destination(&router, "ConflictDest").await;

    let first = seed_task(
        &router,
        json!({ "Name": "First", "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE }),
    )
    .await;
    let second = seed_task(
        &router,
        json!({ "Name": "Second", "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE }),
    )
    .await;

    for (case, body) in device_bodies("ConflictDest") {
        let device_id = seed_device(&router, body).await;
        let (status, answered) = associate(&router, &first, &device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: the first association must succeed, got {status} {answered}"
        );

        let (status, answered) = associate(&router, &second, &device_id).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {case:?}: a second task for the same device must be 409, got {status} \
             {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {answered}"
        );

        // The refusal changed nothing: the device is still in the first task and in no
        // other.
        assert!(
            sorted_ids(&list_for_task(&router, &first, 0).await).contains(&device_id),
            "case {case:?}: the refused call must leave the first membership in place"
        );
        assert!(
            !sorted_ids(&list_for_task(&router, &second, 0).await).contains(&device_id),
            "case {case:?}: the refused call must not have joined the second task"
        );
    }
}

/// Both ends of the association are resolved against real state: an `Id` or a
/// `WirelessDeviceId` the store does not hold is a 404 that persists nothing.
#[tokio::test]
async fn an_absent_task_or_device_is_not_found() {
    let router = router();
    seed_destination(&router, "MissingDest").await;
    let task_id = seed_task(
        &router,
        json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE }),
    )
    .await;
    let device_id = seed_device(
        &router,
        json!({ "Type": "LoRaWAN", "DestinationName": "MissingDest" }),
    )
    .await;

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
        let (status, answered) = associate(&router, &task, &device).await;
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
        let entries = list_for_task(&router, &task_id, 0).await;
        assert!(
            entries.is_empty(),
            "case {case:?}: a refused association must persist nothing, got {entries:?}"
        );
    }

    // The real pair still associates after every refusal.
    let (status, answered) = associate(&router, &task_id, &device_id).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the real pair must still associate, got {status} {answered}"
    );
    assert_eq!(
        sorted_ids(&list_for_task(&router, &task_id, 0).await),
        vec![device_id],
        "the task must now hold the device"
    );
}

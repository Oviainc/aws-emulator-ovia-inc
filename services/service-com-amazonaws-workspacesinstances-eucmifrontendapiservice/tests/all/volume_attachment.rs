// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the volume-attachment slice: `DisassociateVolume` and the
//! `AssociateVolume` that gives it something to detach, driven over the wire this
//! service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`).
//!
//! The rule under test: an attachment is a stored relation between ONE volume and ONE
//! WorkSpace Instance at one device path — `AssociateVolume` creates it and
//! `DisassociateVolume` removes the one the request names, each resolving both ids
//! against stored state. So a detach answers `ResourceNotFoundException` for an id
//! that names no stored resource, `ConflictException` for a volume that holds no such
//! attachment (none at all, one to another instance, or one at another device), and on
//! success leaves the volume attached to nothing: free to be attached again at any
//! device and free to be deleted, with its old device path free for another volume.
//!
//! Nothing here can pass against a canned answer: this service models no operation
//! that REPORTS an attachment, so every assertion reads the stored relation back
//! through the operations it changes — `DeleteVolume` refuses a volume that holds one,
//! a second `AssociateVolume` refuses a volume that already holds one, and a second
//! `DisassociateVolume` refuses a volume that no longer does.
//!
//! The wire helpers come from the `workspace_instance_lifecycle` suite and the volume
//! helpers from `volume_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::volume_lifecycle::{create_volume, simple_create};
use crate::workspace_instance_lifecycle::{
    call, call_in_region, create, error_code, minimal_create, ok, router, DEFAULT_TEST_REGION,
};

/// The `Message` member of an error envelope, which the JSON protocols render with the
/// modelled casing and some clients read in lower case.
fn message(payload: &Value) -> Option<&str> {
    payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
}

/// An instance and a volume, both freshly created through the service.
async fn instance_and_volume(router: &MultiServiceRouter) -> (String, String) {
    let instance = create(router, minimal_create()).await;
    let volume = create_volume(router, simple_create("us-east-1a")).await;
    (instance, volume)
}

/// An `AssociateVolume` body.
fn attach_body(instance: &str, volume: &str, device: &str) -> Value {
    json!({ "WorkspaceInstanceId": instance, "VolumeId": volume, "Device": device })
}

/// A `DisassociateVolume` body: `Device` and `DisassociateMode` are optional, so each
/// is sent only when the case asks for it.
fn detach_body(
    instance: &str,
    volume: &str,
    device: Option<&str>,
    mode: Option<&str>,
) -> Value {
    let mut body = json!({ "WorkspaceInstanceId": instance, "VolumeId": volume });
    if let Some(device) = device {
        body["Device"] = json!(device);
    }
    if let Some(mode) = mode {
        body["DisassociateMode"] = json!(mode);
    }
    body
}

/// Assert `operation` is refused with the modelled `ConflictException` (HTTP 409)
/// naming `resource_id` as the conflicting resource, `resource_type` as its kind and
/// `expected_message` as the service's fixed wording for the conflict.
async fn assert_conflict(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    resource_id: &str,
    resource_type: &str,
    expected_message: &str,
) {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected HTTP 409 from {operation}, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ConflictException",
        "case {case:?}: expected ConflictException from {operation}, got {payload}"
    );
    assert_eq!(
        payload["ResourceId"].as_str(),
        Some(resource_id),
        "case {case:?}: expected the conflict to name {resource_id:?}, got {payload}"
    );
    assert_eq!(
        payload["ResourceType"].as_str(),
        Some(resource_type),
        "case {case:?}: expected the resource type {resource_type:?}, got {payload}"
    );
    assert_eq!(
        message(&payload),
        Some(expected_message),
        "case {case:?}: expected the service's own wording, got {payload}"
    );
}

/// Assert `operation` is refused as not found, with the status and the resource members
/// this service answers that shape with.
async fn assert_not_found(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    resource_id: &str,
    resource_type: &str,
    expected_message: &str,
) {
    let (status, payload) = call(router, operation, body).await;
    let expected_status = wsinst::state::RESOURCE_NOT_FOUND_STATUS;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status} from {operation}, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException from {operation}, got {payload}"
    );
    assert_eq!(
        payload["ResourceId"].as_str(),
        Some(resource_id),
        "case {case:?}: expected the refusal to name {resource_id:?}, got {payload}"
    );
    assert_eq!(
        payload["ResourceType"].as_str(),
        Some(resource_type),
        "case {case:?}: expected the resource type {resource_type:?}, got {payload}"
    );
    assert_eq!(
        message(&payload),
        Some(expected_message),
        "case {case:?}: expected the service's own wording, got {payload}"
    );
}

/// Assert the volume still holds an attachment: `DeleteVolume` refuses it as in use,
/// which is the only read of the stored relation this service's surface offers.
async fn assert_attached(router: &MultiServiceRouter, case: &str, volume: &str) {
    assert_conflict(
        router,
        case,
        "DeleteVolume",
        json!({ "VolumeId": volume }),
        volume,
        wsinst::state::VOLUME_RESOURCE_TYPE,
        wsinst::state::VOLUME_IN_USE_MESSAGE,
    )
    .await;
}

/// One detach case: the device path the volume is attached at, whether the detach
/// echoes that device, and the `DisassociateMode` it asks for.
struct DetachRow {
    case: &'static str,
    device: String,
    /// `Device` on the detach request: the attachment's own device, or absent.
    echo_device: bool,
    /// `DisassociateMode`, or absent.
    mode: Option<&'static str>,
}

/// The detach cases, one row per dimension the rule generalises over: the device path
/// (its published forms, and both ends of what the modelled `DeviceName` admits),
/// whether `Device` is sent at all, and each value of `DisassociateModeEnum`.
///
/// The device paths are the forms AWS publishes for an EBS attachment point (the
/// Amazon EC2 device-naming reference: `/dev/sd[f-p]` on Linux, `xvd…` on the
/// paravirtual and Windows names); the lengths are the modelled `DeviceName`'s own
/// `len 0..32` (`smithy_query(query="shape",shape="DeviceName")`), so the one-character
/// and 32-character rows sit on its bounds.
fn detach_rows() -> Vec<DetachRow> {
    // Every value of the modelled enum, from the generated list, so no mode is left
    // untested the day the model adds one.
    let modes = wsinst::ops::types::DISASSOCIATE_MODE_ENUM_VALUES;
    assert_eq!(
        modes,
        &["FORCE", "NO_FORCE"],
        "case catalog: the generated DisassociateModeEnum list is the model's, got {modes:?}"
    );

    let mut rows = vec![
        DetachRow {
            case: "a Linux device path, device echoed, no mode",
            device: "/dev/sdf".to_string(),
            echo_device: true,
            mode: None,
        },
        DetachRow {
            case: "a paravirtual device path, no device, no mode",
            device: "/dev/xvdb".to_string(),
            echo_device: false,
            mode: None,
        },
        DetachRow {
            case: "a Windows device name, device echoed",
            device: "xvdf".to_string(),
            echo_device: true,
            mode: None,
        },
        DetachRow {
            case: "the shortest device the model admits",
            device: "a".to_string(),
            echo_device: false,
            mode: None,
        },
        DetachRow {
            case: "a device of the modelled maximum length",
            device: format!("/dev/{}", "d".repeat(27)),
            echo_device: true,
            mode: None,
        },
    ];
    assert_eq!(
        rows[4].device.len(),
        32,
        "case bounds: the longest row must sit on the modelled maximum, got {:?}",
        rows[4].device
    );

    for mode in modes {
        rows.push(DetachRow {
            case: "a modelled DisassociateMode",
            device: format!("/dev/sd{}", mode.to_lowercase().chars().next().unwrap()),
            echo_device: false,
            mode: Some(mode),
        });
    }

    rows
}

#[tokio::test]
async fn a_detach_removes_the_attachment_and_frees_the_volume_and_its_device() {
    let router = router();

    for row in detach_rows() {
        let case = row.case;
        let (instance, volume) = instance_and_volume(&router).await;

        let attached = ok(
            &router,
            "AssociateVolume",
            attach_body(&instance, &volume, &row.device),
        )
        .await;
        assert!(
            attached.is_null() || attached.as_object().is_some_and(|body| body.is_empty()),
            "case {case:?}: AssociateVolumeResponse models no members, got {attached}"
        );

        // The attach persisted a relation, not a canned 200: the volume is in use and
        // holds exactly ONE attachment, so neither a delete nor a second attach is
        // accepted.
        assert_attached(&router, case, &volume).await;
        assert_conflict(
            &router,
            case,
            "AssociateVolume",
            attach_body(&instance, &volume, "/dev/sdp"),
            &volume,
            wsinst::state::VOLUME_RESOURCE_TYPE,
            wsinst::state::VOLUME_ALREADY_ATTACHED_MESSAGE,
        )
        .await;

        // The device path is occupied by that volume, so another volume cannot take it.
        let other_volume = create_volume(&router, simple_create("us-east-1b")).await;
        assert_conflict(
            &router,
            case,
            "AssociateVolume",
            attach_body(&instance, &other_volume, &row.device),
            &volume,
            wsinst::state::VOLUME_RESOURCE_TYPE,
            wsinst::state::DEVICE_ALREADY_IN_USE_MESSAGE,
        )
        .await;

        let detach = detach_body(
            &instance,
            &volume,
            row.echo_device.then_some(row.device.as_str()),
            row.mode,
        );
        let detached = ok(&router, "DisassociateVolume", detach.clone()).await;
        assert!(
            detached.is_null() || detached.as_object().is_some_and(|body| body.is_empty()),
            "case {case:?}: DisassociateVolumeResponse models no members, got {detached}"
        );

        // The attachment is gone: a second detach of the same volume has nothing to
        // remove, the freed device path takes another volume, and the volume itself is
        // attachable again — at a different device from the one it held.
        assert_conflict(
            &router,
            case,
            "DisassociateVolume",
            detach,
            &volume,
            wsinst::state::VOLUME_RESOURCE_TYPE,
            wsinst::state::VOLUME_NOT_ATTACHED_MESSAGE,
        )
        .await;
        ok(
            &router,
            "AssociateVolume",
            attach_body(&instance, &other_volume, &row.device),
        )
        .await;
        ok(
            &router,
            "AssociateVolume",
            attach_body(&instance, &volume, "/dev/sdp"),
        )
        .await;
        ok(
            &router,
            "DisassociateVolume",
            detach_body(&instance, &volume, None, None),
        )
        .await;

        // A detached volume is deletable, which is the stored relation read back one
        // last time: the refusal above and this success differ only by the detach.
        ok(&router, "DeleteVolume", json!({ "VolumeId": &volume })).await;
    }
}

#[tokio::test]
async fn a_detach_of_an_attachment_the_volume_does_not_hold_is_a_conflict() {
    let router = router();
    let (instance, volume) = instance_and_volume(&router).await;
    let other_instance = create(&router, minimal_create()).await;
    let unattached = create_volume(&router, simple_create("us-east-1c")).await;
    let device = "/dev/sdh";
    ok(
        &router,
        "AssociateVolume",
        attach_body(&instance, &volume, device),
    )
    .await;

    // Each row names an attachment the volume does not hold: no attachment at all, one
    // to another instance, or one at another device. The device rows differ from the
    // stored device in form as well as value.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a volume that was never attached",
            detach_body(&instance, &unattached, None, None),
            wsinst::state::VOLUME_NOT_ATTACHED_MESSAGE,
        ),
        (
            "a volume attached to another instance",
            detach_body(&other_instance, &volume, None, None),
            wsinst::state::VOLUME_NOT_ATTACHED_MESSAGE,
        ),
        (
            "another instance, with the right device",
            detach_body(&other_instance, &volume, Some(device), None),
            wsinst::state::VOLUME_NOT_ATTACHED_MESSAGE,
        ),
        (
            "a neighbouring device path",
            detach_body(&instance, &volume, Some("/dev/sdg"), None),
            wsinst::state::VOLUME_DEVICE_MISMATCH_MESSAGE,
        ),
        (
            "the same device in another form",
            detach_body(&instance, &volume, Some("xvdh"), None),
            wsinst::state::VOLUME_DEVICE_MISMATCH_MESSAGE,
        ),
        (
            "a device of the modelled maximum length",
            detach_body(&instance, &volume, Some(&"d".repeat(32)), None),
            wsinst::state::VOLUME_DEVICE_MISMATCH_MESSAGE,
        ),
    ];

    for (case, body, expected_message) in rows {
        let volume_id = body["VolumeId"].as_str().unwrap_or_default().to_string();
        assert_conflict(
            &router,
            case,
            "DisassociateVolume",
            body,
            &volume_id,
            wsinst::state::VOLUME_RESOURCE_TYPE,
            expected_message,
        )
        .await;
    }

    // None of those refusals touched the attachment: the volume is still in use, and
    // the detach that names the attachment it really holds is accepted.
    assert_attached(&router, "after the refusals", &volume).await;
    ok(
        &router,
        "DisassociateVolume",
        detach_body(&instance, &volume, Some(device), None),
    )
    .await;
    ok(&router, "DeleteVolume", json!({ "VolumeId": &volume })).await;
    ok(&router, "DeleteVolume", json!({ "VolumeId": &unattached })).await;
}

#[tokio::test]
async fn a_detach_resolves_both_ids_against_the_store() {
    let router = router();
    let (instance, volume) = instance_and_volume(&router).await;
    ok(
        &router,
        "AssociateVolume",
        attach_body(&instance, &volume, "/dev/sdf"),
    )
    .await;

    // Well-formed per their modelled shapes, stored by nobody: the shortest and
    // longest legal ids, a mixed-case one and an all-digit one.
    let unknown_instances = [
        ("the shortest legal instance id", format!("wsinst-{}", "a".repeat(8))),
        ("the longest legal instance id", format!("wsinst-{}", "z".repeat(62))),
        ("a mixed-case instance id", "wsinst-AbCdEf0123".to_string()),
        ("an all-digit instance id", "wsinst-012345678".to_string()),
    ];
    for (case, unknown) in &unknown_instances {
        assert_ne!(
            unknown, &instance,
            "case {case:?}: the test's unknown id must not be the stored one"
        );
        // The instance is the resource the volume is detached FROM, so it is resolved
        // first: an unknown instance is refused even beside a stored volume, and beside
        // an unknown volume it is the id the refusal names.
        for (suffix, volume_id) in [("", volume.as_str()), (" with an unknown volume", "vol-nope")] {
            assert_not_found(
                &router,
                &format!("{case}{suffix}"),
                "DisassociateVolume",
                detach_body(unknown, volume_id, None, None),
                unknown,
                wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
                wsinst::state::INSTANCE_NOT_FOUND_MESSAGE,
            )
            .await;
        }
    }

    let unknown_volumes = [
        ("the shortest legal volume id", "vol-a".to_string()),
        ("the longest legal volume id", format!("vol-{}", "z".repeat(63))),
        ("a mixed-case volume id", "vol-AbCdEf0123".to_string()),
        ("an all-digit volume id", "vol-98765432109876543".to_string()),
    ];
    for (case, unknown) in &unknown_volumes {
        assert_ne!(
            unknown, &volume,
            "case {case:?}: the test's unknown id must not be the stored one"
        );
        assert_not_found(
            &router,
            case,
            "DisassociateVolume",
            detach_body(&instance, unknown, None, None),
            unknown,
            wsinst::state::VOLUME_RESOURCE_TYPE,
            wsinst::state::VOLUME_NOT_FOUND_MESSAGE,
        )
        .await;
        // The attach path resolves the same two ids the same way.
        assert_not_found(
            &router,
            case,
            "AssociateVolume",
            attach_body(&instance, unknown, "/dev/sdp"),
            unknown,
            wsinst::state::VOLUME_RESOURCE_TYPE,
            wsinst::state::VOLUME_NOT_FOUND_MESSAGE,
        )
        .await;
    }

    // The attachment lives in the ENGINE the service was built from, not in the
    // request: a second engine, with the same two ids minted the same way and no
    // attach call, answers the identical detach request as "not attached".
    let elsewhere = crate::workspace_instance_lifecycle::router();
    let (other_instance, other_volume) = instance_and_volume(&elsewhere).await;
    assert_eq!(
        (other_instance.as_str(), other_volume.as_str()),
        (instance.as_str(), volume.as_str()),
        "case the second engine: the ids are minted from the store, so an identically \
         built engine mints the identical pair — got {other_instance:?}/{other_volume:?}"
    );
    assert_conflict(
        &elsewhere,
        "the same ids in an engine that never attached them",
        "DisassociateVolume",
        detach_body(&instance, &volume, None, None),
        &volume,
        wsinst::state::VOLUME_RESOURCE_TYPE,
        wsinst::state::VOLUME_NOT_ATTACHED_MESSAGE,
    )
    .await;

    // The attachment this engine holds survived every refusal above.
    assert_attached(&router, "after the refusals", &volume).await;
}

#[tokio::test]
async fn a_device_that_names_no_device_path_is_refused() {
    let router = router();
    let (instance, volume) = instance_and_volume(&router).await;

    // The modelled `DeviceName` is `len 0..32`, so the empty string reaches the
    // handler: it names no device path on either call. One past the maximum is refused
    // by the boundary validator, with the same status and code.
    let too_long = "d".repeat(33);
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "an empty Device on an attach",
            "AssociateVolume",
            attach_body(&instance, &volume, ""),
        ),
        (
            "an empty Device on a detach",
            "DisassociateVolume",
            detach_body(&instance, &volume, Some(""), None),
        ),
        (
            "a Device one past the modelled maximum",
            "AssociateVolume",
            attach_body(&instance, &volume, &too_long),
        ),
        (
            "an attach with no Device at all, which the model requires",
            "AssociateVolume",
            json!({ "WorkspaceInstanceId": &instance, "VolumeId": &volume }),
        ),
        (
            "a DisassociateMode outside the modelled enum",
            "DisassociateVolume",
            detach_body(&instance, &volume, None, Some("force")),
        ),
    ];

    for (case, operation, body) in rows {
        let (status, payload) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException from {operation}, got {payload}"
        );
    }

    // Nothing above attached anything, so the volume is still free to delete.
    ok(&router, "DeleteVolume", json!({ "VolumeId": &volume })).await;
}

#[tokio::test]
async fn a_teardown_releases_the_attachments_of_the_instance_it_tears_down() {
    let router = router();
    let (instance, volume) = instance_and_volume(&router).await;
    ok(
        &router,
        "AssociateVolume",
        attach_body(&instance, &volume, "/dev/sdf"),
    )
    .await;
    assert_attached(&router, "before the delete", &volume).await;

    // Deleting the instance starts its teardown, which takes the volumes attached to it
    // with it: the volume holds no attachment afterwards, so a detach has nothing to
    // remove and the volume is deletable with no detach call at all.
    ok(
        &router,
        "DeleteWorkspaceInstance",
        json!({ "WorkspaceInstanceId": &instance }),
    )
    .await;
    assert_conflict(
        &router,
        "after the instance teardown started",
        "DisassociateVolume",
        detach_body(&instance, &volume, None, None),
        &volume,
        wsinst::state::VOLUME_RESOURCE_TYPE,
        wsinst::state::VOLUME_NOT_ATTACHED_MESSAGE,
    )
    .await;

    // And no new volume can be attached to an instance that is tearing down: here the
    // conflicting resource is the INSTANCE, so the refusal names it and its kind.
    let fresh = create_volume(&router, simple_create("us-east-1a")).await;
    assert_conflict(
        &router,
        "an attach to a tearing-down instance",
        "AssociateVolume",
        attach_body(&instance, &fresh, "/dev/sdg"),
        &instance,
        wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
        wsinst::state::INSTANCE_TEARING_DOWN_MESSAGE,
    )
    .await;

    ok(&router, "DeleteVolume", json!({ "VolumeId": &volume })).await;

    // Past the teardown and the retention the instance is not reported at all, so both
    // calls answer not-found for it rather than a conflict.
    tokio::time::sleep(
        wsinst::state::DEALLOCATION_SPAN
            + wsinst::state::DEALLOCATED_RETENTION
            + std::time::Duration::from_millis(250),
    )
    .await;
    assert_not_found(
        &router,
        "an attach past the retention",
        "AssociateVolume",
        attach_body(&instance, &fresh, "/dev/sdg"),
        &instance,
        wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
        wsinst::state::INSTANCE_NOT_FOUND_MESSAGE,
    )
    .await;
    assert_not_found(
        &router,
        "a detach past the retention",
        "DisassociateVolume",
        detach_body(&instance, &fresh, None, None),
        &instance,
        wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
        wsinst::state::INSTANCE_NOT_FOUND_MESSAGE,
    )
    .await;
}

#[tokio::test]
async fn one_instance_carries_many_volumes_one_per_device() {    let router = router();
    let instance = create(&router, minimal_create()).await;
    let second_instance = create(&router, minimal_create()).await;

    // One device per volume, several volumes per instance: the attachments are a
    // relation, not a single slot on the instance.
    let devices = ["/dev/sdf", "/dev/sdg", "/dev/sdh", "xvdi"];
    let mut volumes = Vec::new();
    for (n, device) in devices.iter().enumerate() {
        let volume = create_volume(&router, simple_create("us-east-1a")).await;
        ok(
            &router,
            "AssociateVolume",
            attach_body(&instance, &volume, device),
        )
        .await;
        assert_attached(&router, &format!("volume {n} at {device}"), &volume).await;
        volumes.push(volume);
    }

    // The same device path on a DIFFERENT instance is free, so a device is scoped to
    // the instance it is on.
    let on_second = create_volume(&router, simple_create("us-east-1b")).await;
    ok(
        &router,
        "AssociateVolume",
        attach_body(&second_instance, &on_second, devices[0]),
    )
    .await;

    // Detaching one volume leaves every other attachment where it was.
    ok(
        &router,
        "DisassociateVolume",
        detach_body(&instance, &volumes[1], Some(devices[1]), None),
    )
    .await;
    ok(&router, "DeleteVolume", json!({ "VolumeId": &volumes[1] })).await;
    for (n, volume) in volumes.iter().enumerate() {
        if n == 1 {
            continue;
        }
        assert_attached(&router, &format!("volume {n} after a sibling detach"), volume).await;
    }
    assert_attached(&router, "the volume on the second instance", &on_second).await;

    // The device the detached volume held is free for another volume on that instance.
    let replacement = create_volume(&router, simple_create("us-east-1a")).await;
    ok(
        &router,
        "AssociateVolume",
        attach_body(&instance, &replacement, devices[1]),
    )
    .await;
}

#[tokio::test]
async fn an_attachment_is_made_and_removed_only_in_the_region_that_holds_the_resources() {
    // Foreign Regions on other continents, so no Region string here shares a prefix with
    // the signing Region of the helpers (`DEFAULT_TEST_REGION`) and none can be matched
    // by shape. Membership is checked against the service's own availability set, so a
    // row that drifts out of it fails rather than testing an unsupported Region.
    let foreign_regions = ["eu-west-3", "ap-northeast-2"];
    for region in foreign_regions.iter().chain([&DEFAULT_TEST_REGION]) {
        assert!(
            wsinst::state::SUPPORTED_REGIONS.contains(region),
            "case {region:?}: this suite's Regions must be ones the service is available \
             in, which are {:?}",
            wsinst::state::SUPPORTED_REGIONS
        );
    }

    for foreign in foreign_regions {
        let router = router();
        let (instance, volume) = instance_and_volume(&router).await;
        let device = "/dev/sdf";
        ok(
            &router,
            "AssociateVolume",
            attach_body(&instance, &volume, device),
        )
        .await;

        // Both resources belong to the Region their create calls were signed for, so to
        // a caller signed for another Region they do not exist: an attach or a detach
        // there answers the refusal this service gives an id it never minted, naming the
        // instance it resolves first.
        for operation in ["AssociateVolume", "DisassociateVolume"] {
            let body = attach_body(&instance, &volume, device);
            let (status, payload) = call_in_region(&router, foreign, operation, body).await;
            let case = format!("{operation} from {foreign}");
            let expected_status = wsinst::state::RESOURCE_NOT_FOUND_STATUS;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {payload}"
            );
            assert_eq!(
                payload["ResourceId"].as_str(),
                Some(instance.as_str()),
                "case {case:?}: expected the refusal to name the instance, got {payload}"
            );
        }

        // A volume of ANOTHER Region cannot be attached to this Region's instance
        // either: the instance resolves, the volume does not, so the volume is the id
        // the refusal names.
        let foreign_volume = {
            let body = json!({ "AvailabilityZone": format!("{foreign}a"), "SizeInGB": 4 });
            let (status, payload) = call_in_region(&router, foreign, "CreateVolume", body).await;
            assert_eq!(
                status, 200,
                "case a volume in {foreign}: expected HTTP 200, got {status} with {payload}"
            );
            payload["VolumeId"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateVolume in {foreign} returned no VolumeId"))
                .to_string()
        };
        assert_not_found(
            &router,
            &format!("an attach of a {foreign} volume to a {DEFAULT_TEST_REGION} instance"),
            "AssociateVolume",
            attach_body(&instance, &foreign_volume, "/dev/sdg"),
            &foreign_volume,
            wsinst::state::VOLUME_RESOURCE_TYPE,
            wsinst::state::VOLUME_NOT_FOUND_MESSAGE,
        )
        .await;

        // None of those refusals touched the attachment, and the Region that holds both
        // resources can still detach it.
        assert_attached(&router, &format!("after the calls from {foreign}"), &volume).await;
        ok(
            &router,
            "DisassociateVolume",
            detach_body(&instance, &volume, Some(device), None),
        )
        .await;
        ok(&router, "DeleteVolume", json!({ "VolumeId": &volume })).await;
    }
}

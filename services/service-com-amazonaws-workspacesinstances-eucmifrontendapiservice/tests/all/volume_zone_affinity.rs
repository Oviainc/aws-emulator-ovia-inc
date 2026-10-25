// SPDX-License-Identifier: CC0-1.0

//! The Availability Zone rule `AssociateVolume` applies, driven over the wire this
//! service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule under test, stated once and asserted over a SET of inputs rather than one
//! recorded request:
//!
//! > a volume and the WorkSpace Instance it is attached to live in ONE Availability
//! > Zone, so an `AssociateVolume` whose volume sits in a different zone from the one
//! > the instance was placed in is refused with the modelled `ValidationException`
//! > (HTTP 400) naming `VolumeId` and stores no attachment, while an attach inside one
//! > zone is performed.
//!
//! The rows below differ along what that rule generalises over: which zone of the
//! Region the instance is placed in and which the volume is created in (every pairing
//! of the Region's zones, not only the cited `us-east-1b` / `us-east-1a` one), whether
//! the instance names a placement zone at all, the device path, and the Region (the
//! zone set is derived per Region, so the rule is asserted in a second one too).
//!
//! Nothing here can pass against a canned answer: this service models no operation
//! that REPORTS an attachment, so every accepted attach is read back through the
//! operations the stored relation changes (`DeleteVolume` refuses an attached volume,
//! `DisassociateVolume` frees it), and every refusal is checked against the store the
//! service itself was built on.

use serde_json::json;
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{call_in_region, error_code, harness, DEFAULT_TEST_REGION};

/// The `ValidationExceptionReason` a single bad member is reported with, as the
/// modelled enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// Device paths an attach may name, so no row's outcome can come from the device the
/// request happens to carry. Inside the modelled `DeviceName` (`len 0..32`).
const DEVICES: [&str; 3] = ["/dev/sdf", "/dev/xvdb", "xvdh"];

/// `CreateWorkspaceInstance` in `region`, placed in `zone` when one is given, returning
/// the minted id.
async fn create_instance(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    zone: Option<&str>,
) -> String {
    let mut managed_instance =
        json!({ "ImageId": "ami-0123456789abcdef0", "InstanceType": "t3.medium" });
    if let Some(zone) = zone {
        managed_instance["Placement"] = json!({ "AvailabilityZone": zone });
    }
    let (status, payload) = call_in_region(
        router,
        region,
        "CreateWorkspaceInstance",
        json!({ "ManagedInstance": managed_instance }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} in {region}: expected the create to be accepted, got {status} with {payload}"
    );
    payload["WorkspaceInstanceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?} in {region}: no WorkspaceInstanceId in {payload}"))
        .to_string()
}

/// `CreateVolume` in `region` and `zone`, returning the minted id.
async fn create_volume(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    zone: &str,
) -> String {
    let (status, payload) = call_in_region(
        router,
        region,
        "CreateVolume",
        json!({ "AvailabilityZone": zone, "SizeInGB": 4 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} in {region}: expected the volume create to be accepted, got {status} with \
         {payload}"
    );
    payload["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?} in {region}: no VolumeId in {payload}"))
        .to_string()
}

/// Whether the service's own store holds an attachment for `volume`, read the one legal
/// way: through the engine the router was built from.
fn attachment_stored(engine: &engine::InMemoryEngine, volume: &str) -> bool {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .volumes
        .get(volume)
        .is_some_and(|record| record.attachment.is_some())
}

/// Assert the attach of `volume` to `instance` at `device` is refused for a zone
/// mismatch, and that the refusal stored nothing.
async fn assert_zone_mismatch(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    case: &str,
    instance: &str,
    volume: &str,
    device: &str,
) {
    let (status, payload) = call_in_region(
        router,
        region,
        "AssociateVolume",
        json!({ "WorkspaceInstanceId": instance, "VolumeId": volume, "Device": device }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?} in {region}: expected HTTP 400 for an attach across Availability Zones, \
         got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case {case:?} in {region}: expected ValidationException, got {payload}"
    );
    assert_eq!(
        payload["Reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case:?} in {region}: expected the single-member reason, got {payload}"
    );
    let named: Vec<&str> = payload["FieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?} in {region}: no FieldList in {payload}"))
        .iter()
        .map(|field| field["Name"].as_str().unwrap_or("<no Name>"))
        .collect();
    assert_eq!(
        named,
        vec![wsinst::state::VOLUME_ID_MEMBER],
        "case {case:?} in {region}: expected the refusal to name the volume member, got {payload}"
    );
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?} in {region}: no Message in {payload}"));
    assert_eq!(
        message,
        wsinst::state::VOLUME_ZONE_MISMATCH_MESSAGE,
        "case {case:?} in {region}: expected the service's own wording, got {message:?}"
    );
    assert!(
        !attachment_stored(engine, volume),
        "case {case:?} in {region}: a refused attach must store no attachment for {volume:?}"
    );
    // The refusal is observable through the operations too: a volume that holds no
    // attachment is deletable, where an attached one answers the in-use conflict.
    let (status, payload) =
        call_in_region(router, region, "DeleteVolume", json!({ "VolumeId": volume })).await;
    assert_eq!(
        status, 200,
        "case {case:?} in {region}: a volume no attach stored must be deletable, got {status} \
         with {payload}"
    );
}

/// Assert the attach of `volume` to `instance` at `device` is performed, reading the
/// stored attachment back through `DeleteVolume` (refused while it holds one) and
/// `DisassociateVolume` (which frees it), and leaving the volume gone.
async fn assert_attached(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    case: &str,
    instance: &str,
    volume: &str,
    device: &str,
) {
    let (status, payload) = call_in_region(
        router,
        region,
        "AssociateVolume",
        json!({ "WorkspaceInstanceId": instance, "VolumeId": volume, "Device": device }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} in {region}: expected an attach inside one zone to be performed, got \
         {status} with {payload}"
    );
    assert!(
        attachment_stored(engine, volume),
        "case {case:?} in {region}: an accepted attach must store the attachment for {volume:?}"
    );
    let (status, payload) =
        call_in_region(router, region, "DeleteVolume", json!({ "VolumeId": volume })).await;
    assert_eq!(
        status, 409,
        "case {case:?} in {region}: an attached volume must answer the in-use conflict, got \
         {status} with {payload}"
    );
    let (status, payload) = call_in_region(
        router,
        region,
        "DisassociateVolume",
        json!({ "WorkspaceInstanceId": instance, "VolumeId": volume, "Device": device }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} in {region}: the stored attachment must be the one a detach removes, got \
         {status} with {payload}"
    );
    let (status, payload) =
        call_in_region(router, region, "DeleteVolume", json!({ "VolumeId": volume })).await;
    assert_eq!(
        status, 200,
        "case {case:?} in {region}: a detached volume must be deletable, got {status} with \
         {payload}"
    );
}

/// Every pairing of an instance placement and a volume zone in one Region: an attach
/// inside one zone is performed, an attach across zones is refused, and an instance
/// whose create named no zone takes a volume from any zone of its Region.
async fn zone_pairings_in(region: &str) {
    let (router, engine) = harness();
    // The zones of the Region, from the service's own catalog rather than a list
    // written out here (`wsinst::state::availability_zones_in_region`).
    let zones = wsinst::state::availability_zones_in_region(region);
    assert!(
        zones.len() > 1,
        "case zones in {region}: the rule needs a Region with more than one zone, got {zones:?}"
    );

    let mut placements: Vec<Option<&str>> = vec![None];
    placements.extend(zones.iter().map(|zone| Some(zone.as_str())));

    for (row, placement) in placements.iter().enumerate() {
        for (column, volume_zone) in zones.iter().enumerate() {
            let case = format!(
                "instance placed in {placement:?}, volume in {volume_zone:?}",
                placement = placement.unwrap_or("<no placement>")
            );
            let device = DEVICES[(row + column) % DEVICES.len()];
            let instance = create_instance(&router, region, &case, *placement).await;
            let volume = create_volume(&router, region, &case, volume_zone).await;

            if wsinst::state::zones_allow_attachment(*placement, volume_zone) {
                assert_attached(
                    &router, &engine, region, &case, &instance, &volume, device,
                )
                .await;
            } else {
                assert_zone_mismatch(
                    &router, &engine, region, &case, &instance, &volume, device,
                )
                .await;
                // The refusal left the device path free as well as the volume: a volume
                // in the instance's OWN zone still attaches there.
                let same_zone = create_volume(
                    &router,
                    region,
                    &case,
                    placement.expect("a mismatch means the instance named a zone"),
                )
                .await;
                assert_attached(
                    &router, &engine, region, &case, &instance, &same_zone, device,
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn an_attach_is_refused_across_availability_zones_and_performed_inside_one() {
    zone_pairings_in(DEFAULT_TEST_REGION).await;
}

/// The same rule in another Region, whose zones are other names: the zone pair is what
/// decides the attach, not one Region's zone names.
#[tokio::test]
async fn the_zone_rule_holds_in_every_region_this_service_serves() {
    zone_pairings_in("eu-west-2").await;
}

/// A cross-zone attach is refused whatever else is true of the pair: the volume the
/// instance would have taken is still free afterwards, and the refusal does not depend
/// on the instance having any other attachment.
#[tokio::test]
async fn a_cross_zone_attach_leaves_the_volume_free_to_attach_in_its_own_zone() {
    let (router, engine) = harness();
    let region = DEFAULT_TEST_REGION;
    let zones = wsinst::state::availability_zones_in_region(region);
    let (first, second) = (zones[0].as_str(), zones[1].as_str());

    let case = "a volume refused by one instance attaches to another in its own zone";
    let elsewhere = create_instance(&router, region, case, Some(second)).await;
    let home = create_instance(&router, region, case, Some(first)).await;
    let volume = create_volume(&router, region, case, first).await;

    let (status, payload) = call_in_region(
        &router,
        region,
        "AssociateVolume",
        json!({ "WorkspaceInstanceId": &elsewhere, "VolumeId": &volume, "Device": DEVICES[0] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the cross-zone attach to be refused, got {status} with {payload}"
    );
    assert!(
        !attachment_stored(&engine, &volume),
        "case {case:?}: the refused attach must have stored nothing"
    );
    assert_attached(
        &router, &engine, region, case, &home, &volume, DEVICES[0],
    )
    .await;
}

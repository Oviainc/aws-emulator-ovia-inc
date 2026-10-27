// SPDX-License-Identifier: CC0-1.0

//! The Availability Zone rule `CreateWorkspaceInstance` applies to a placement, driven
//! over the wire this service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule under test, stated once and asserted over a SET of inputs rather than one
//! request:
//!
//! > a resource of this service belongs to the Region its create call was signed for,
//! > so an Availability Zone the request names must be one THAT Region publishes: a
//! > create whose `ManagedInstance.Placement.AvailabilityZone` names a published zone
//! > is launched and placed in it, and one naming any other zone — another Region's
//! > zone, a letter this Region does not publish, or a zone name of a form this Region
//! > publishes none of — is refused with the modelled `ValidationException` (HTTP 400)
//! > naming `ManagedInstance.Placement.AvailabilityZone` and stores nothing.
//!
//! The rows differ along what the rule generalises over: the Region (every Region
//! `ListRegions` reports), the zone (every zone each of those Regions publishes, which
//! for the six-zone Region reaches `f`), the way a zone misses — a foreign Region's
//! zone, the letters past a Region's last one, the gaps inside its run, and a
//! local-zone-shaped name the modelled pattern admits — and how much other placement
//! configuration surrounds the zone, so a refusal turns on the zone and not on the
//! request's shape. The absent placement and a placement that names no zone are rows
//! too: the service places such an instance itself.
//!
//! Nothing here can pass against a canned answer: an accepted placement is read back
//! out of the store the service itself was built on AND through the call the placement
//! decides (`AssociateVolume`, which attaches only a volume of the instance's zone),
//! and a refusal is only counted once the store is seen to hold exactly what it held
//! before.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{
    call_in_region, error_code, harness, TEST_IMAGE_ID, TEST_INSTANCE_TYPE,
};

/// The `ValidationExceptionReason` a single bad member is reported with, as the modelled
/// enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The member a bad placement zone is reported against.
const ZONE_MEMBER: &str = "ManagedInstance.Placement.AvailabilityZone";

/// A device path an attach may name, inside the modelled `DeviceName` (`len 0..32`).
const DEVICE: &str = "/dev/sdf";

/// A create body placing the smallest launchable instance in `zone`, with `extra`
/// merged onto its `Placement` so a row can surround the zone with more configuration.
fn create_body(zone: Option<&str>, extra: Value) -> Value {
    let mut placement = json!({});
    if let Some(zone) = zone {
        placement["AvailabilityZone"] = json!(zone);
    }
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        placement[member] = value;
    }
    let mut managed_instance =
        json!({ "ImageId": TEST_IMAGE_ID, "InstanceType": TEST_INSTANCE_TYPE });
    if placement.as_object().is_some_and(|members| !members.is_empty()) {
        managed_instance["Placement"] = placement;
    }
    json!({ "ManagedInstance": managed_instance })
}

/// How many instances the service's own store holds, read the one legal way: through
/// the engine the router was built from.
fn stored_instances(engine: &engine::InMemoryEngine) -> usize {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .instances
        .len()
}

/// The Availability Zone the service stored for `instance`, as the attach path reads it.
fn stored_zone(engine: &engine::InMemoryEngine, instance: &str) -> Option<String> {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .instances
        .get(instance)
        .and_then(|record| record.placement_availability_zone().map(ToString::to_string))
}

/// Assert the create is accepted, that one instance reached the store and that the
/// placement the store kept is the one asked for. Returns the minted id.
async fn assert_created(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    region: &str,
    expected_zone: Option<&str>,
    body: Value,
) -> String {
    let held = stored_instances(engine);
    let (status, payload) = call_in_region(router, region, "CreateWorkspaceInstance", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 in {region}, got {status} with {payload}"
    );
    let id = payload["WorkspaceInstanceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstanceId in {payload}"))
        .to_string();
    assert_eq!(
        stored_instances(engine),
        held + 1,
        "case {case:?}: an accepted create must store one instance"
    );
    let stored = stored_zone(engine, &id);
    assert_eq!(
        stored.as_deref(),
        expected_zone,
        "case {case:?}: expected the stored placement to be {expected_zone:?}, got {stored:?}"
    );

    let (status, got) = call_in_region(
        router,
        region,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": &id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the minted {id:?} must resolve in {region}, got {status} with {got}"
    );
    id
}

/// Assert the create is refused with the modelled `ValidationException` naming the
/// placement zone, and that it stored nothing.
async fn assert_refused(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    region: &str,
    body: Value,
) {
    let held = stored_instances(engine);
    let (status, payload) = call_in_region(router, region, "CreateWorkspaceInstance", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 in {region}, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case {case:?}: expected ValidationException in {region}, got {payload}"
    );
    assert_eq!(
        payload["Reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case:?}: expected the single-member reason, got {payload}"
    );
    let named: Vec<&str> = payload["FieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no FieldList in {payload}"))
        .iter()
        .map(|field| field["Name"].as_str().unwrap_or("<no Name>"))
        .collect();
    assert_eq!(
        named,
        vec![ZONE_MEMBER],
        "case {case:?}: expected the refusal to name {ZONE_MEMBER}, got {payload}"
    );
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: no Message in {payload}"));
    assert!(
        message.contains(ZONE_MEMBER),
        "case {case:?}: expected the wording to name {ZONE_MEMBER}, got {message:?}"
    );
    assert_eq!(
        stored_instances(engine),
        held,
        "case {case:?}: a refused create must store nothing"
    );
}

#[tokio::test]
async fn every_zone_a_region_publishes_places_an_instance() {
    let (router, engine) = harness();

    // Every Region `ListRegions` reports, and every zone that Region publishes: the rows
    // reach `d`, `e` and `f` where a Region publishes them, and skip the letters it does
    // not.
    for region in wsinst::state::SUPPORTED_REGIONS {
        for zone in wsinst::state::availability_zones_in_region(region) {
            let case = format!("{zone} is a zone of {region}");
            assert_created(
                &router,
                &engine,
                &case,
                region,
                Some(&zone),
                create_body(Some(&zone), json!({})),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_zone_the_signed_region_does_not_publish_is_refused() {
    let (router, engine) = harness();

    // For every Region: a zone of ANOTHER Region, which the modelled pattern admits and
    // this Region publishes none of.
    let regions = wsinst::state::SUPPORTED_REGIONS;
    for (index, region) in regions.iter().enumerate() {
        let foreign = regions[(index + 1) % regions.len()];
        let foreign_zone = wsinst::state::availability_zones_in_region(foreign)
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("{foreign} publishes at least one zone"));
        let case = format!("{foreign_zone} is a zone of {foreign}, not of {region}");
        assert_refused(
            &router,
            &engine,
            &case,
            region,
            create_body(Some(&foreign_zone), json!({})),
        )
        .await;
    }

    // And, for Regions whose published letters differ in shape — six zones, four zones,
    // a run with a gap inside it, and a plain three — every letter of the Latin alphabet
    // the Region does NOT publish: the letters past its last one as well as the gaps.
    for region in ["us-east-1", "us-west-2", "ap-northeast-1", "ca-central-1", "eu-west-1"] {
        let letters = wsinst::state::availability_zone_letters(region);
        for letter in 'a'..='z' {
            if letters.contains(letter) {
                continue;
            }
            let case = format!("{region} publishes no zone {letter}");
            assert_refused(
                &router,
                &engine,
                &case,
                region,
                create_body(Some(&format!("{region}{letter}")), json!({})),
            )
            .await;
        }
    }

    // Zone names of the OTHER form the modelled pattern admits
    // (`^[a-z]{2}-[a-z]+-\d(?:[a-z]|-[a-z]+-\d[a-z])$`, so a Local Zone name like
    // `us-east-1-bos-1a`): this service publishes none of them anywhere, so they name no
    // zone of the signed Region either.
    for (region, zone) in [
        ("us-east-1", "us-east-1-bos-1a"),
        ("us-west-2", "us-west-2-lax-1a"),
        ("eu-west-1", "eu-west-1-cgn-1a"),
    ] {
        let case = format!("{zone} is no zone {region} publishes");
        assert_refused(
            &router,
            &engine,
            &case,
            region,
            create_body(Some(zone), json!({})),
        )
        .await;
    }
}

#[tokio::test]
async fn the_refusal_turns_on_the_zone_and_not_on_the_rest_of_the_placement() {
    let (router, engine) = harness();
    let region = "us-east-1";
    let published = wsinst::state::availability_zones_in_region(region);
    let good = published
        .last()
        .cloned()
        .expect("the Region publishes at least one zone");

    // The same placement members around a published zone and around one the Region does
    // not publish: the accepted and refused sides differ only in the zone.
    let surroundings = [
        ("a tenancy beside the zone", json!({ "Tenancy": "default" })),
        (
            "a placement group beside the zone",
            json!({ "GroupName": "my-group", "PartitionNumber": 1 }),
        ),
        (
            "every other placement member beside the zone",
            json!({
                "Affinity": "host",
                "GroupId": "pg-0123456789",
                "HostId": "h-0123456789",
                "HostResourceGroupArn": "arn:aws:resource-groups:us-east-1:111111111111:group/hosts",
                "Tenancy": "default",
            }),
        ),
    ];
    for (case, extra) in surroundings {
        assert_created(
            &router,
            &engine,
            &format!("{case}, published"),
            region,
            Some(&good),
            create_body(Some(&good), extra.clone()),
        )
        .await;
        assert_refused(
            &router,
            &engine,
            &format!("{case}, unpublished"),
            region,
            create_body(Some("eu-west-1a"), extra),
        )
        .await;
    }
}

#[tokio::test]
async fn a_create_that_names_no_zone_leaves_the_placement_to_the_service() {
    let (router, engine) = harness();
    let region = "ap-northeast-1";

    // No placement at all, and a placement carrying everything EXCEPT a zone: neither
    // names a zone, so the service places the instance and the store keeps none.
    for (case, extra) in [
        ("no placement member at all", json!({})),
        ("a placement without a zone", json!({ "Tenancy": "default" })),
        (
            "a placement group without a zone",
            json!({ "GroupName": "my-group", "PartitionNumber": 0 }),
        ),
    ] {
        assert_created(
            &router,
            &engine,
            case,
            region,
            None,
            create_body(None, extra),
        )
        .await;
    }
}

#[tokio::test]
async fn an_accepted_placement_is_one_a_volume_of_the_region_can_be_attached_in() {
    let (router, engine) = harness();

    // What the rule is FOR: a placement this service accepts is a zone a volume of the
    // same Region can be created in, so the attach the placement decides goes through.
    // Rows in Regions and zones the issue cited none of, including letters past `c`.
    let rows = [
        ("the first zone of the six-zone Region", "us-east-1", "us-east-1a"),
        ("the last zone of the six-zone Region", "us-east-1", "us-east-1f"),
        ("the fourth zone of a four-zone Region", "us-west-2", "us-west-2d"),
        (
            "the last zone of the Region whose run has a gap",
            "ca-central-1",
            "ca-central-1d",
        ),
    ];
    for (case, region, zone) in rows {
        let instance = assert_created(
            &router,
            &engine,
            case,
            region,
            Some(zone),
            create_body(Some(zone), json!({})),
        )
        .await;

        let (status, payload) = call_in_region(
            &router,
            region,
            "CreateVolume",
            json!({ "AvailabilityZone": zone, "SizeInGB": 4 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a volume must be creatable in the zone the instance was placed \
             in, got {status} with {payload}"
        );
        let volume = payload["VolumeId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no VolumeId in {payload}"))
            .to_string();

        let (status, payload) = call_in_region(
            &router,
            region,
            "AssociateVolume",
            json!({ "WorkspaceInstanceId": &instance, "VolumeId": &volume, "Device": DEVICE }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the attach inside {zone} to be performed, got {status} \
             with {payload}"
        );
        let (status, payload) = call_in_region(
            &router,
            region,
            "DisassociateVolume",
            json!({
                "WorkspaceInstanceId": &instance,
                "VolumeId": &volume,
                "Device": DEVICE,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stored attachment to be the one a detach removes, \
             got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn a_refused_placement_neither_replays_a_token_nor_mints_an_instance() {
    let (router, engine) = harness();
    let region = "us-east-2";
    let token = "tok-refused-placement";

    // The refusal comes before the store is read, so the token the refused create
    // carried names no instance afterwards: the next create with a published zone and
    // that token mints one.
    assert_refused(
        &router,
        &engine,
        "a refused placement carrying an idempotency token",
        region,
        {
            let mut body = create_body(Some("us-east-1a"), json!({}));
            body["ClientToken"] = json!(token);
            body
        },
    )
    .await;

    let id = assert_created(
        &router,
        &engine,
        "the token after the refusal",
        region,
        Some("us-east-2b"),
        {
            let mut body = create_body(Some("us-east-2b"), json!({}));
            body["ClientToken"] = json!(token);
            body
        },
    )
    .await;

    assert_eq!(
        stored_instances(&engine),
        1,
        "case the whole sequence: only the create with a published zone is stored beside \
         {id:?}"
    );
}

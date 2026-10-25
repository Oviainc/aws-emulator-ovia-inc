// SPDX-License-Identifier: CC0-1.0

//! Which Availability Zones a Region has, as `CreateVolume` decides it — driven over
//! the wire this service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule under test, stated once and asserted over a SET of inputs rather than one
//! recorded request:
//!
//! > the Availability Zones an AWS Region has are that Region's own published set —
//! > their count differs from Region to Region and their letters need not be a run
//! > starting at `a` — so a `CreateVolume` naming a zone the signed Region publishes
//! > creates the volume (HTTP 200, a `vol-…` id the follow-on calls then resolve), and
//! > one naming a zone it does not publish is refused with the modelled
//! > `ValidationException` (HTTP 400) naming `AvailabilityZone`, whichever side of any
//! > particular letter the zone falls on.
//!
//! The rows differ along what that rule generalises over: the Region (every Region
//! `ListRegions` reports), the zone (every zone each of those Regions publishes, which
//! for the six-zone Region reaches `f`), and, on the refusal side, the letters a Region
//! leaves out — both the letters past its last one and the gaps inside its run.
//!
//! Nothing here can pass against a canned answer: this service models no operation that
//! reports a volume, so an accepted create is read back through the store the service
//! itself was built on AND through the calls that resolve the id (`AssociateVolume`,
//! `DisassociateVolume`, `DeleteVolume`), and a refusal is only counted once the store
//! is seen to hold exactly what it held before.

use serde_json::json;
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{call_in_region, error_code, harness};

/// The `ValidationExceptionReason` a single bad member is reported with, as the modelled
/// enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// A device path an attach may name, inside the modelled `DeviceName` (`len 0..32`).
const DEVICE: &str = "/dev/sdf";

/// How many zones AWS gives a standard Region at least, which every row of the
/// service's zone catalog therefore has to publish.
const MINIMUM_ZONES_PER_REGION: usize = 3;

/// The Availability Zone the service stored for `volume`, read the one legal way:
/// through the engine the router was built from.
fn stored_zone(engine: &engine::InMemoryEngine, volume: &str) -> Option<String> {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .volumes
        .get(volume)
        .map(|record| record.availability_zone.clone())
}

/// How many volumes the service's own store holds.
fn stored_volumes(engine: &engine::InMemoryEngine) -> usize {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .volumes
        .len()
}

/// `CreateVolume` in `region` and `zone`, asserting the create is accepted, that the id
/// has the modelled `^vol-[0-9a-zA-Z]{1,63}$` form, and that the zone the store kept is
/// the one asked for. Returns the minted id.
async fn assert_created(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    region: &str,
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
        "case {case:?}: expected HTTP 200 for {zone:?} in {region}, got {status} with {payload}"
    );
    let id = payload["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no VolumeId in {payload}"))
        .to_string();
    let suffix = id.strip_prefix("vol-").unwrap_or_default();
    assert!(
        (1..=63).contains(&suffix.len()) && suffix.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected a modelled VolumeId, got {id:?}"
    );
    assert_eq!(
        stored_zone(engine, &id).as_deref(),
        Some(zone),
        "case {case:?}: expected the volume to be stored in the zone the create named, got {:?}",
        stored_zone(engine, &id)
    );
    id
}

/// Assert a `CreateVolume` naming `zone` in `region` is refused with the modelled
/// `ValidationException` naming `AvailabilityZone`, and that it stored nothing.
async fn assert_refused(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    region: &str,
    zone: &str,
) {
    let held = stored_volumes(engine);
    let (status, payload) = call_in_region(
        router,
        region,
        "CreateVolume",
        json!({ "AvailabilityZone": zone, "SizeInGB": 4 }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for {zone:?} in {region}, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case {case:?}: expected ValidationException for {zone:?} in {region}, got {payload}"
    );
    assert_eq!(
        payload["Reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case:?}: expected the single-member reason for {zone:?}, got {payload}"
    );
    let named: Vec<&str> = payload["FieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no FieldList in {payload}"))
        .iter()
        .map(|field| field["Name"].as_str().unwrap_or("<no Name>"))
        .collect();
    assert_eq!(
        named,
        vec!["AvailabilityZone"],
        "case {case:?}: expected the refusal to name AvailabilityZone, got {payload}"
    );
    assert_eq!(
        stored_volumes(engine),
        held,
        "case {case:?}: a refused create must store nothing for {zone:?} in {region}"
    );
}

/// `DeleteVolume`, asserting the minted id resolves in the store.
async fn assert_deleted(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    volume: &str,
) {
    let (status, payload) =
        call_in_region(router, region, "DeleteVolume", json!({ "VolumeId": volume })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the minted {volume:?} to resolve in {region}, got {status} \
         with {payload}"
    );
}

/// `CreateWorkspaceInstance` in `region`, placed in `zone`, returning the minted id.
async fn create_instance(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    zone: &str,
) -> String {
    let (status, payload) = call_in_region(
        router,
        region,
        "CreateWorkspaceInstance",
        json!({
            "ManagedInstance": {
                "ImageId": "ami-0123456789abcdef0",
                "InstanceType": "t3.medium",
                "Placement": { "AvailabilityZone": zone },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the instance create in {zone:?} to be accepted, got {status} \
         with {payload}"
    );
    payload["WorkspaceInstanceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstanceId in {payload}"))
        .to_string()
}

#[tokio::test]
async fn every_zone_a_region_publishes_creates_a_volume() {
    let (router, engine) = harness();

    // Every Region `ListRegions` reports, and every zone that Region publishes: the
    // rows reach `d`, `e` and `f` where a Region publishes them, and skip the letters it
    // does not.
    for region in wsinst::state::SUPPORTED_REGIONS {
        let zones = wsinst::state::availability_zones_in_region(region);
        assert!(
            zones.len() >= MINIMUM_ZONES_PER_REGION,
            "case {region:?}: a Region the service serves publishes at least \
             {MINIMUM_ZONES_PER_REGION} zones, got {zones:?}"
        );
        for zone in &zones {
            let case = format!("{zone} is a zone of {region}");
            let volume = assert_created(&router, &engine, &case, region, zone).await;
            assert_deleted(&router, &case, region, &volume).await;
        }
    }
}

#[tokio::test]
async fn a_letter_a_region_does_not_publish_is_refused() {
    let (router, engine) = harness();

    // For each Region: every letter of the Latin alphabet it does NOT publish — the
    // letters past its last one (`g` upward for the six-zone Region, `d` upward for a
    // three-zone one) and the gaps inside its run (Tokyo's `b`, Canada Central's `c`).
    for region in wsinst::state::SUPPORTED_REGIONS {
        let letters = wsinst::state::availability_zone_letters(region);
        for letter in 'a'..='z' {
            if letters.contains(letter) {
                continue;
            }
            let case = format!("{region} publishes no zone {letter}");
            assert_refused(&router, &engine, &case, region, &format!("{region}{letter}")).await;
        }
    }
}

#[tokio::test]
async fn a_volume_in_a_zone_past_the_third_letter_serves_the_follow_on_calls() {
    let (router, engine) = harness();

    // Zones whose letter is past `c`, so none of them could be reached by a zone set cut
    // at the third letter: two of the Regions the issue cited and two it did not. Each
    // row runs the whole chain a caller runs after a create — attach to an instance in
    // the same zone, which `DeleteVolume` then refuses, detach, delete — so the zone is
    // proven to be stored and usable rather than merely accepted.
    let rows = [
        ("the fourth zone of the six-zone Region", "us-east-1", "us-east-1d"),
        ("the last zone of the six-zone Region", "us-east-1", "us-east-1f"),
        ("the fourth zone of a four-zone Region", "us-west-2", "us-west-2d"),
        (
            "the last zone of the Region whose run has a gap",
            "ap-northeast-1",
            "ap-northeast-1d",
        ),
        (
            "the fourth zone of another four-zone Region",
            "ap-northeast-2",
            "ap-northeast-2d",
        ),
        (
            "the third zone of the Region that publishes no c",
            "ca-central-1",
            "ca-central-1d",
        ),
    ];

    for (case, region, zone) in rows {
        assert!(
            wsinst::state::availability_zones_in_region(region)
                .iter()
                .any(|published| published == zone),
            "case {case:?}: {zone:?} must be one of the zones the catalog publishes for \
             {region}, got {:?}",
            wsinst::state::availability_zones_in_region(region)
        );
        let volume = assert_created(&router, &engine, case, region, zone).await;
        let instance = create_instance(&router, case, region, zone).await;

        let (status, payload) = call_in_region(
            &router,
            region,
            "AssociateVolume",
            json!({ "WorkspaceInstanceId": &instance, "VolumeId": &volume, "Device": DEVICE }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the attach in {zone:?} to be performed, got {status} with \
             {payload}"
        );
        let (status, payload) =
            call_in_region(&router, region, "DeleteVolume", json!({ "VolumeId": &volume })).await;
        assert_eq!(
            status, 409,
            "case {case:?}: an attached volume must answer the in-use conflict, got {status} \
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
            "case {case:?}: expected the stored attachment to be the one a detach removes, got \
             {status} with {payload}"
        );
        assert_deleted(&router, case, region, &volume).await;
    }
}

#[tokio::test]
async fn the_zone_catalog_covers_every_region_the_service_reports() {
    // The catalog is hand-written data, so it is checked against the OTHER hand-written
    // set it has to agree with — the Regions `ListRegions` reports — and against its own
    // documented shape: sorted, free of duplicates, lower-case letters in ascending
    // order.
    let catalog = wsinst::state::REGION_AVAILABILITY_ZONE_LETTERS;
    let regions: Vec<&str> = catalog.iter().map(|(region, _)| *region).collect();
    assert_eq!(
        regions,
        wsinst::state::SUPPORTED_REGIONS.to_vec(),
        "case \"catalog membership\": the zone catalog must carry one row per Region the \
         service reports, in the same order, got {regions:?}"
    );

    for (region, letters) in catalog {
        let case = format!("the zones of {region}");
        let published: Vec<char> = letters.chars().collect();
        assert!(
            published.iter().all(|letter| letter.is_ascii_lowercase()),
            "case {case:?}: a zone letter is a lower-case ASCII letter, got {letters:?}"
        );
        let mut sorted = published.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, published,
            "case {case:?}: the letters must be ascending and free of duplicates, got \
             {letters:?}"
        );
        assert!(
            published.len() >= MINIMUM_ZONES_PER_REGION,
            "case {case:?}: expected at least {MINIMUM_ZONES_PER_REGION} zones, got {letters:?}"
        );
        assert_eq!(
            published.first().copied(),
            Some('a'),
            "case {case:?}: a Region's zone letters start at a, got {letters:?}"
        );
    }

    // Published counts and gaps, from the documented source on the catalog. The first
    // three rows are the Regions the issue cited; the rest it cited none of.
    let rows = [
        ("the oldest US Region publishes six zones", "us-east-1", "abcdef"),
        ("the US west Region publishes four", "us-west-2", "abcd"),
        ("Tokyo publishes a, c and d, with no b", "ap-northeast-1", "acd"),
        ("Seoul publishes four", "ap-northeast-2", "abcd"),
        ("Canada Central publishes a, b and d, with no c", "ca-central-1", "abd"),
        ("Ireland publishes three", "eu-west-1", "abc"),
        ("Sao Paulo publishes three", "sa-east-1", "abc"),
    ];
    for (case, region, letters) in rows {
        let published = wsinst::state::availability_zone_letters(region);
        assert_eq!(
            published, letters,
            "case {case:?}: expected the zone letters of {region} to be {letters:?}, got \
             {published:?}"
        );
    }

    // A Region this service does not serve has no endpoint here, so it publishes no
    // zones here either.
    for region in ["us-gov-west-1", "cn-north-1", "us-west-1", "eu-central-2"] {
        let published = wsinst::state::availability_zones_in_region(region);
        assert!(
            published.is_empty(),
            "case {region:?}: a Region outside the service's availability set publishes no \
             zones, got {published:?}"
        );
    }
}

/// The catalog is read, never written: the zones a Region has are a service fact, so two
/// reads answer the same set and a create cannot move it.
#[tokio::test]
async fn reading_the_zones_of_a_region_changes_nothing_a_later_caller_sees() {
    let (router, engine) = harness();
    let region = "us-east-1";
    let before = wsinst::state::availability_zones_in_region(region);

    let zone = before
        .last()
        .cloned()
        .expect("the Region publishes at least one zone");
    let case = "a create leaves the Region's zone set alone";
    let volume = assert_created(&router, &engine, case, region, &zone).await;
    assert_deleted(&router, case, region, &volume).await;

    let after = wsinst::state::availability_zones_in_region(region);
    assert_eq!(
        after, before,
        "case {case:?}: expected the same zone set, got {after:?} after {before:?}"
    );
}

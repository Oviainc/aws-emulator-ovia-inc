// SPDX-License-Identifier: CC0-1.0

//! The cross-field configuration rules `CreateVolume` applies, driven over the wire
//! this service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.CreateVolume`).
//!
//! The rule under test, stated once and asserted over a SET of inputs rather than one
//! recorded request:
//!
//! > a `CreateVolume` whose members are mutually inconsistent with one volume the
//! > backing Amazon EBS volume service can provision — an `AvailabilityZone` that is
//! > not a zone of the Region the request was signed for, no capacity source (neither
//! > `SizeInGB` nor `SnapshotId`), a `SizeInGB`, `Iops` or `Throughput` outside the
//! > range the named `VolumeType` publishes or not taken by that type at all, or a
//! > `KmsKeyId` without `Encrypted` true — is refused with the modelled
//! > `ValidationException` (HTTP 400) naming the offending member, and stores nothing;
//! > every other configuration is created.
//!
//! The rows below differ along what that rule generalises over: the member, the
//! Region, the zone, the volume type (every published one, not only the cited `st1`),
//! which side of a bound a value falls on (each published bound and the value one past
//! it), and whether a member is out of range or not taken at all. The four requests
//! the issue recorded are four of those rows.
//!
//! Nothing here can pass against a canned answer: an accepted create is only counted
//! once `DeleteVolume` resolves the minted id out of the store, and a refusal is only
//! counted once the store is seen to hold exactly what it held before.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;
use wsinst::state::VolumeTypeSpec;

use crate::workspace_instance_lifecycle::{call_in_region, error_code, harness, DEFAULT_TEST_REGION};

/// The `ValidationExceptionReason` a single bad member is reported with, as the
/// modelled enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// How many volumes the service's own store holds — the engine the router was built
/// from, read the one legal way.
fn stored_volumes(engine: &engine::InMemoryEngine) -> usize {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .volumes
        .len()
}

/// Assert `body` is refused with the modelled `ValidationException`, that the refusal
/// names `member` both in its wording and in `FieldList`, and that the store is
/// untouched.
async fn assert_refused(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    case: &str,
    member: &str,
    body: Value,
) {
    let held = stored_volumes(engine);
    let (status, payload) = call_in_region(router, region, "CreateVolume", body).await;
    assert_eq!(
        status, 400,
        "case {case:?} in {region}: expected HTTP 400, got {status} with {payload}"
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
        vec![member],
        "case {case:?} in {region}: expected the refusal to name {member:?}, got {payload}"
    );
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?} in {region}: no Message in {payload}"));
    assert!(
        message.contains(member),
        "case {case:?} in {region}: expected the wording to name {member:?}, got {message:?}"
    );
    assert_eq!(
        stored_volumes(engine),
        held,
        "case {case:?} in {region}: a refused create must store nothing"
    );
}

/// Assert `body` creates a volume, proving it reached the store by resolving the
/// minted id through `DeleteVolume` (and leaving the store as it was found).
async fn assert_created(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    case: &str,
    body: Value,
) {
    let held = stored_volumes(engine);
    let (status, payload) = call_in_region(router, region, "CreateVolume", body).await;
    assert_eq!(
        status, 200,
        "case {case:?} in {region}: expected HTTP 200, got {status} with {payload}"
    );
    let id = payload["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?} in {region}: no VolumeId in {payload}"))
        .to_string();
    assert_eq!(
        stored_volumes(engine),
        held + 1,
        "case {case:?} in {region}: an accepted create must store one volume"
    );
    let (status, payload) =
        call_in_region(router, region, "DeleteVolume", json!({ "VolumeId": &id })).await;
    assert_eq!(
        status, 200,
        "case {case:?} in {region}: the minted {id:?} must resolve in the store, got {status} \
         with {payload}"
    );
    assert_eq!(
        stored_volumes(engine),
        held,
        "case {case:?} in {region}: the delete must leave the store as it was found"
    );
}

/// The smallest create body `spec` accepts: a zone of the signed Region, the type's
/// minimum size, and provisioned IOPS only where the type requires them.
fn minimal_body(spec: &VolumeTypeSpec, zone: &str) -> Value {
    let mut body = json!({
        "AvailabilityZone": zone,
        "VolumeType": spec.volume_type,
        "SizeInGB": spec.size_range().0,
    });
    if spec.iops_required {
        body["Iops"] = json!(spec.iops.expect("a type that requires IOPS publishes a range").0);
    }
    body
}

#[tokio::test]
async fn the_availability_zone_must_be_one_of_the_signed_regions_zones() {
    let (router, engine) = harness();

    // Three Regions this service publishes (`ListRegions`), one of them the Region the
    // issue's requests were signed for and two it never cites. The zones of a Region
    // are `<region><letter>` for each letter the account's Regions carry
    // (`wsinst::state::AVAILABILITY_ZONE_LETTERS`), which is the set EC2 reports for
    // the same Region.
    let regions = ["us-east-1", "eu-west-3", "ap-northeast-2"];
    for region in regions {
        assert!(
            wsinst::state::SUPPORTED_REGIONS.contains(&region),
            "case {region:?}: the test's Region must be one the service supports, got \
             {:?}",
            wsinst::state::SUPPORTED_REGIONS
        );
        for letter in wsinst::state::AVAILABILITY_ZONE_LETTERS {
            let zone = format!("{region}{letter}");
            assert_created(
                &router,
                &engine,
                region,
                &format!("zone {zone} of its own Region"),
                json!({ "AvailabilityZone": zone, "SizeInGB": 8 }),
            )
            .await;
        }

        // One letter past the zones the Region has, a zone of a DIFFERENT Region, the
        // Region's own name, text that is not a zone at all, and the empty string the
        // modelled `len 0..64` admits.
        let past_the_last = format!(
            "{region}{}",
            char::from(
                wsinst::state::AVAILABILITY_ZONE_LETTERS
                    .last()
                    .copied()
                    .expect("the letter set is not empty") as u8
                    + 1
            )
        );
        let other_region = regions
            .iter()
            .find(|candidate| **candidate != region)
            .expect("three Regions, so another one exists");
        let rows: Vec<(String, String)> = vec![
            ("one letter past the Region's zones".to_string(), past_the_last),
            (
                "a zone of another Region".to_string(),
                format!("{other_region}a"),
            ),
            ("the Region's own name".to_string(), region.to_string()),
            ("text that is not a zone".to_string(), "not-a-zone".to_string()),
            ("the empty string".to_string(), String::new()),
            (
                "a 64-character value, the modelled maximum".to_string(),
                "a".repeat(64),
            ),
        ];
        for (case, zone) in rows {
            assert_refused(
                &router,
                &engine,
                region,
                &case,
                "AvailabilityZone",
                json!({ "AvailabilityZone": zone, "SizeInGB": 8 }),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_volume_needs_a_capacity_source() {
    let (router, engine) = harness();
    let region = DEFAULT_TEST_REGION;
    let zone = format!("{region}a");

    // Neither a size nor a snapshot: the request names no capacity source at all.
    assert_refused(
        &router,
        &engine,
        region,
        "neither SizeInGB nor SnapshotId",
        "SizeInGB",
        json!({ "AvailabilityZone": &zone }),
    )
    .await;
    assert_refused(
        &router,
        &engine,
        region,
        "no capacity source, with every other member set",
        "SizeInGB",
        json!({
            "AvailabilityZone": &zone,
            "VolumeType": "gp3",
            "Iops": 3000,
            "Throughput": 125,
            "Encrypted": true,
        }),
    )
    .await;

    // Either source alone is enough, and both together are consistent.
    for (case, body) in [
        (
            "SizeInGB alone",
            json!({ "AvailabilityZone": &zone, "SizeInGB": 1 }),
        ),
        (
            "SnapshotId alone, which carries the size",
            json!({ "AvailabilityZone": &zone, "SnapshotId": "snap-0123456789abcdef" }),
        ),
        (
            "both a snapshot and a size",
            json!({
                "AvailabilityZone": &zone,
                "SnapshotId": "snap-0123456789abcdef",
                "SizeInGB": 40,
            }),
        ),
        (
            "a snapshot restored onto a type with a higher minimum",
            json!({
                "AvailabilityZone": &zone,
                "SnapshotId": "snap-0123456789abcdef",
                "VolumeType": "st1",
            }),
        ),
    ] {
        assert_created(&router, &engine, region, case, body).await;
    }
}

#[tokio::test]
async fn size_iops_and_throughput_must_be_ones_the_named_volume_type_takes() {
    let (router, engine) = harness();
    let region = DEFAULT_TEST_REGION;
    let zone = format!("{region}a");
    let (modelled_min, modelled_max) = wsinst::state::MODELLED_VOLUME_SIZE_RANGE;

    // Every published volume type, from the service's own catalog — so a type the
    // model admits cannot be left untested. `st1` (the type the issue cites) is one
    // row of it.
    let specs = wsinst::state::VOLUME_TYPE_SPECS;
    assert_eq!(
        specs.len(),
        wsinst::ops::types::VOLUME_TYPE_ENUM_VALUES.len(),
        "case catalog: every modelled VolumeType needs rules, got {:?} for {:?}",
        specs.iter().map(|spec| spec.volume_type).collect::<Vec<_>>(),
        wsinst::ops::types::VOLUME_TYPE_ENUM_VALUES
    );

    for spec in &specs {
        let volume_type = spec.volume_type;
        let (size_min, size_max) = spec.size_range();

        // Each published bound of the type's size range is accepted …
        for (case, size) in [
            (format!("{volume_type} at its minimum size"), size_min),
            (format!("{volume_type} at its maximum size"), size_max),
        ] {
            let mut body = minimal_body(spec, &zone);
            body["SizeInGB"] = json!(size);
            assert_created(&router, &engine, region, &case, body).await;
        }

        // … and the value one past each bound is refused. A value one past a bound
        // that coincides with the modelled `SizeInGB` range is refused by the
        // boundary instead, which answers the same `ValidationException` without a
        // `FieldList` of this handler's making.
        for (case, size) in [
            (format!("{volume_type} one below its minimum size"), size_min - 1),
            (format!("{volume_type} one above its maximum size"), size_max + 1),
        ] {
            let mut body = minimal_body(spec, &zone);
            body["SizeInGB"] = json!(size);
            if (modelled_min..=modelled_max).contains(&size) {
                assert_refused(&router, &engine, region, &case, "SizeInGB", body).await;
            } else {
                let (status, payload) =
                    call_in_region(&router, region, "CreateVolume", body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: a size outside the modelled range must be refused, got \
                     {status} with {payload}"
                );
                assert_eq!(
                    error_code(&payload),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {payload}"
                );
            }
        }

        match spec.iops {
            // A type that provisions IOPS takes each published bound and refuses one
            // past it; `Iops: 0`, which the modelled `range 0..` admits, is below
            // every published minimum.
            Some((iops_min, iops_max)) => {
                for (case, iops) in [
                    (format!("{volume_type} at its minimum Iops"), iops_min),
                    (format!("{volume_type} at its maximum Iops"), iops_max),
                ] {
                    let mut body = minimal_body(spec, &zone);
                    body["Iops"] = json!(iops);
                    assert_created(&router, &engine, region, &case, body).await;
                }
                for (case, iops) in [
                    (format!("{volume_type} one below its minimum Iops"), iops_min - 1),
                    (format!("{volume_type} one above its maximum Iops"), iops_max + 1),
                    (format!("{volume_type} with Iops 0"), 0),
                ] {
                    let mut body = minimal_body(spec, &zone);
                    body["Iops"] = json!(iops);
                    assert_refused(&router, &engine, region, &case, "Iops", body).await;
                }
                // A type that REQUIRES IOPS is refused when the request names none.
                if spec.iops_required {
                    let mut body = minimal_body(spec, &zone);
                    body.as_object_mut()
                        .expect("the body is an object")
                        .remove("Iops");
                    assert_refused(
                        &router,
                        &engine,
                        region,
                        &format!("{volume_type} without Iops"),
                        "Iops",
                        body,
                    )
                    .await;
                }
            }
            // A type that provisions none takes no `Iops` at all, whatever the value.
            None => {
                for iops in [0, 1, 500, 80_000] {
                    let mut body = minimal_body(spec, &zone);
                    body["Iops"] = json!(iops);
                    assert_refused(
                        &router,
                        &engine,
                        region,
                        &format!("{volume_type} with Iops {iops}, which it does not provision"),
                        "Iops",
                        body,
                    )
                    .await;
                }
            }
        }

        match spec.throughput_mib {
            Some((min, max)) => {
                for (case, throughput) in [
                    (format!("{volume_type} at its minimum Throughput"), min),
                    (format!("{volume_type} at its maximum Throughput"), max),
                ] {
                    let mut body = minimal_body(spec, &zone);
                    body["Throughput"] = json!(throughput);
                    assert_created(&router, &engine, region, &case, body).await;
                }
                for (case, throughput) in [
                    (format!("{volume_type} one below its minimum Throughput"), min - 1),
                    (format!("{volume_type} one above its maximum Throughput"), max + 1),
                    (format!("{volume_type} with Throughput 0"), 0),
                ] {
                    let mut body = minimal_body(spec, &zone);
                    body["Throughput"] = json!(throughput);
                    assert_refused(&router, &engine, region, &case, "Throughput", body).await;
                }
            }
            None => {
                for throughput in [0, 125, 250, 2_000] {
                    let mut body = minimal_body(spec, &zone);
                    body["Throughput"] = json!(throughput);
                    assert_refused(
                        &router,
                        &engine,
                        region,
                        &format!(
                            "{volume_type} with Throughput {throughput}, which it does not take"
                        ),
                        "Throughput",
                        body,
                    )
                    .await;
                }
            }
        }
    }

    // A request that names no `VolumeType` is the backing service's documented
    // default (`gp2`), so the default type's rules are the ones applied: `gp2`
    // provisions no IOPS and takes no throughput, and its size range is the one
    // checked.
    let default = wsinst::state::volume_type_spec(wsinst::state::DEFAULT_VOLUME_TYPE)
        .expect("the default volume type is in the catalog");
    for member in ["Iops", "Throughput"] {
        let mut body = json!({ "AvailabilityZone": &zone, "SizeInGB": 10 });
        body[member] = json!(125);
        assert_refused(
            &router,
            &engine,
            region,
            &format!("{member} with no VolumeType, which is the default type"),
            member,
            body,
        )
        .await;
    }
    let mut body = json!({ "AvailabilityZone": &zone });
    body["SizeInGB"] = json!(default.size_range().1 + 1);
    let (status, payload) = call_in_region(&router, region, "CreateVolume", body).await;
    assert_eq!(
        status, 400,
        "case a size past the default type's maximum: expected HTTP 400, got {status} with \
         {payload}"
    );
}

#[tokio::test]
async fn a_kms_key_is_accepted_only_for_an_encrypted_volume() {
    let (router, engine) = harness();
    let region = DEFAULT_TEST_REGION;
    let zone = format!("{region}a");
    let key = "arn:aws:kms:us-east-1:123456789012:key/abcd";

    // The parameter dependency: a key names the encryption of a volume that is not
    // encrypted. Each row leaves `Encrypted` out, false, or names an empty key the
    // modelled `len 0..128` admits.
    for (case, body) in [
        (
            "a KmsKeyId with Encrypted absent",
            json!({ "AvailabilityZone": &zone, "SizeInGB": 10, "KmsKeyId": key }),
        ),
        (
            "a KmsKeyId with Encrypted false",
            json!({
                "AvailabilityZone": &zone,
                "SizeInGB": 10,
                "Encrypted": false,
                "KmsKeyId": key,
            }),
        ),
        (
            "a KmsKeyId with Encrypted false on another volume type",
            json!({
                "AvailabilityZone": &zone,
                "SizeInGB": 125,
                "VolumeType": "st1",
                "Encrypted": false,
                "KmsKeyId": key,
            }),
        ),
        (
            "an empty KmsKeyId, which names no key",
            json!({
                "AvailabilityZone": &zone,
                "SizeInGB": 10,
                "Encrypted": true,
                "KmsKeyId": "",
            }),
        ),
    ] {
        assert_refused(&router, &engine, region, case, "KmsKeyId", body).await;
    }

    // Encryption with a key, encryption without one (the service-managed key), and
    // no encryption at all are all consistent.
    for (case, body) in [
        (
            "an encrypted volume with a key",
            json!({
                "AvailabilityZone": &zone,
                "SizeInGB": 10,
                "Encrypted": true,
                "KmsKeyId": key,
            }),
        ),
        (
            "an encrypted volume with no key named",
            json!({ "AvailabilityZone": &zone, "SizeInGB": 10, "Encrypted": true }),
        ),
        (
            "an unencrypted volume",
            json!({ "AvailabilityZone": &zone, "SizeInGB": 10, "Encrypted": false }),
        ),
    ] {
        assert_created(&router, &engine, region, case, body).await;
    }
}

#[tokio::test]
async fn the_four_recorded_requests_are_refused_and_store_nothing() {
    let (router, engine) = harness();
    let region = DEFAULT_TEST_REGION;

    // The requests the issue recorded, each with the member its refusal names.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "an empty AvailabilityZone",
            "AvailabilityZone",
            json!({ "AvailabilityZone": "" }),
        ),
        (
            "the required AvailabilityZone alone",
            "SizeInGB",
            json!({ "AvailabilityZone": "us-east-1a" }),
        ),
        (
            "st1 below its minimum size, with Iops and Throughput it does not take",
            "SizeInGB",
            json!({
                "AvailabilityZone": "us-east-1a",
                "VolumeType": "st1",
                "SizeInGB": 1,
                "Iops": 500,
                "Throughput": 250,
            }),
        ),
        (
            "a KmsKeyId with no Encrypted",
            "KmsKeyId",
            json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "KmsKeyId": "arn:aws:kms:us-east-1:123456789012:key/abcd",
            }),
        ),
    ];

    for (case, member, body) in rows {
        assert_refused(&router, &engine, region, case, member, body).await;
    }

    // The store never held anything, so no `vol-…` id was minted for any of them.
    assert_eq!(
        stored_volumes(&engine),
        0,
        "case the recorded requests: none of them may create a volume"
    );
}

#[tokio::test]
async fn a_refused_configuration_neither_replays_a_token_nor_spends_quota() {
    let (router, engine) = harness();
    let region = DEFAULT_TEST_REGION;
    let zone = format!("{region}a");
    let token = "tok-refused-configuration";

    // A refused create is refused before the store is read, so the token it carried
    // names no volume afterwards: the next GOOD create with that token mints one.
    assert_refused(
        &router,
        &engine,
        region,
        "a refused create carrying an idempotency token",
        "SizeInGB",
        json!({ "AvailabilityZone": &zone, "ClientToken": token }),
    )
    .await;

    let (status, payload) = call_in_region(
        &router,
        region,
        "CreateVolume",
        json!({ "AvailabilityZone": &zone, "SizeInGB": 10, "ClientToken": token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the token after a refusal: expected HTTP 200, got {status} with {payload}"
    );
    let id = payload["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the token after a refusal: no VolumeId in {payload}"))
        .to_string();
    assert_eq!(
        stored_volumes(&engine),
        1,
        "case the token after a refusal: exactly the one good create is stored"
    );

    // And the refusal is still a refusal once the token names a volume: a bad
    // configuration is never answered with the stored id.
    assert_refused(
        &router,
        &engine,
        region,
        "a refused create carrying a token that now names a volume",
        "AvailabilityZone",
        json!({ "AvailabilityZone": "not-a-zone", "SizeInGB": 10, "ClientToken": token }),
    )
    .await;

    let (status, _) =
        call_in_region(&router, region, "DeleteVolume", json!({ "VolumeId": &id })).await;
    assert_eq!(status, 200, "case cleanup: the one volume deletes");
}

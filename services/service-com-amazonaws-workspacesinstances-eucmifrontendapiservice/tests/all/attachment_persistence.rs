// SPDX-License-Identifier: CC0-1.0

//! What an `AssociateVolume` persists, driven over the wire this service really speaks
//! (awsJson1_0, `POST /`, `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule under test, stated once and asserted over a SET of inputs rather than one
//! request:
//!
//! > `AssociateVolume` is served from stored state: an accepted call persists ONE
//! > attachment on the named volume, carrying exactly the WorkSpace Instance id and the
//! > device path that request named and changing nothing else about the volume, and a
//! > refused call persists nothing at all — so no input is answered with the signal of
//! > an operation nobody has written.
//!
//! The sibling `volume_attachment` suite asserts the attach/detach relation through the
//! operations that observe it; this one asserts what the STORE holds either way, and
//! spans the dimensions the rule generalises over: the volume configuration (every
//! volume type the service publishes rules for, each at its own published minimum size
//! and with the IOPS its type requires), the device path (the published attachment-point
//! forms and both ends of the modelled `DeviceName`), whether the instance was created
//! naming a placement zone, and the Region the pair lives in.
//!
//! Nothing here can pass against a canned answer: this service models no operation that
//! REPORTS an attachment, so each assertion either reads the service's own store through
//! the engine the router was built from, or reads the relation back through the
//! operations it changes (`DeleteVolume` refuses an attached volume, `DisassociateVolume`
//! frees it).
//!
//! The wire helpers come from the `workspace_instance_lifecycle` suite, the volume
//! create from `volume_lifecycle`, the per-Region instance create from
//! `volume_zone_affinity`, the stored-record read from `volume_modification` and the
//! `AssociateVolume` body and error-envelope reader from `volume_attachment`.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::volume_attachment::{attach_body, message};
use crate::volume_lifecycle::{create_volume, simple_create};
use crate::volume_modification::stored_volume_in;
use crate::volume_zone_affinity::create_instance;
use crate::workspace_instance_lifecycle::{
    call_in_region, error_code, harness, ok, DEFAULT_TEST_REGION,
};

/// The `ValidationExceptionReason` a single bad member is reported with, as the modelled
/// enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The status and wire error code an operation nobody has written answers with: the
/// answer no row here may get, because each one is served from stored state.
const NOT_IMPLEMENTED_STATUS: u16 = 501;
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// The Regions these rows are spread over, so no outcome can come from one Region's zone
/// names. Checked against the service's own availability set in [`attach_rows`].
const ATTACH_REGIONS: [&str; 2] = [DEFAULT_TEST_REGION, "eu-west-2"];

/// Device paths an attach may name.
///
/// The first four are the forms AWS publishes for a block-device attachment point (the
/// Amazon EC2 device-naming reference: `/dev/sd[f-p]` on Linux, the `xvd…` paravirtual
/// and Windows names); the last two sit on the bounds of the modelled `DeviceName`
/// (`len 0..32`, `smithy_query(query="shape",shape="DeviceName")`) — its shortest
/// non-empty value and its longest.
fn device_catalog() -> Vec<String> {
    let devices = vec![
        "/dev/sdf".to_string(),
        "/dev/sdp".to_string(),
        "/dev/xvdb".to_string(),
        "xvdh".to_string(),
        "a".to_string(),
        format!("/dev/{}", "d".repeat(27)),
    ];
    assert_eq!(
        devices[5].len(),
        32,
        "case device catalog: the longest row must sit on the modelled maximum, got {:?}",
        devices[5]
    );
    devices
}

/// One accepted-attach case: where the pair lives, how the volume is configured, and the
/// device path the attach names.
struct AttachRow {
    case: String,
    region: &'static str,
    /// The zone the volume is created in, and the one the instance is placed in when
    /// [`AttachRow::placed`] is set.
    zone: String,
    /// Whether the instance's create named a placement zone at all.
    placed: bool,
    device: String,
    /// The `CreateVolume` body: the row's volume type, with the size and IOPS that type
    /// takes.
    volume: Value,
}

/// One row per volume type the service publishes rules for
/// (`wsinst::state::VOLUME_TYPE_SPECS`, which covers every value of the modelled
/// `VolumeTypeEnum`), spread across the Regions, their zones, the device catalog and both
/// forms of instance placement — so no row shares a value with its neighbours on any
/// dimension the rule generalises over.
fn attach_rows() -> Vec<AttachRow> {
    let devices = device_catalog();
    let specs = wsinst::state::VOLUME_TYPE_SPECS;
    assert!(
        specs.len() > 1,
        "case row catalog: the rule needs more than one volume type, got {specs:?}"
    );
    for region in ATTACH_REGIONS {
        assert!(
            wsinst::state::SUPPORTED_REGIONS.contains(&region),
            "case {region:?}: these rows must run in Regions the service is available in, which \
             are {:?}",
            wsinst::state::SUPPORTED_REGIONS
        );
    }

    specs
        .iter()
        .enumerate()
        .map(|(n, spec)| {
            let region = ATTACH_REGIONS[n % ATTACH_REGIONS.len()];
            let zones = wsinst::state::availability_zones_in_region(region);
            assert!(
                !zones.is_empty(),
                "case row catalog: {region} must have zones, got {zones:?}"
            );
            let zone = zones[n % zones.len()].clone();
            let device = devices[n % devices.len()].clone();
            // The published minimum size for the type, which differs per type, and the
            // IOPS the type requires a create to name.
            let mut volume = json!({
                "AvailabilityZone": &zone,
                "SizeInGB": spec.published_size_gib.0,
                "VolumeType": spec.volume_type,
            });
            if spec.iops_required {
                let (min, _) = spec
                    .iops
                    .unwrap_or_else(|| panic!("a type that requires IOPS must publish a range"));
                volume["Iops"] = json!(min);
            }
            let placed = n % 2 == 0;
            AttachRow {
                case: format!(
                    "a {} volume in {zone} at {device}, instance {}",
                    spec.volume_type,
                    if placed {
                        "placed in that zone"
                    } else {
                        "placed by the service"
                    }
                ),
                region,
                zone,
                placed,
                device,
                volume,
            }
        })
        .collect()
}

/// Every answer here is computed from stored state, so none of them is the signal of an
/// operation nobody has written.
fn assert_served(case: &str, operation: &str, status: u16, payload: &Value) {
    assert_ne!(
        status, NOT_IMPLEMENTED_STATUS,
        "case {case:?}: {operation} answered the not-implemented status with {payload}"
    );
    assert_ne!(
        error_code(payload),
        NOT_IMPLEMENTED_CODE,
        "case {case:?}: {operation} answered the not-implemented error with {payload}"
    );
}

/// Every volume the store holds, as its own rendering keyed by id.
///
/// The WHOLE of what an attach could have persisted, which no per-id read can show: a
/// refusal that wrote to another volume, or minted one, shows up here.
fn volume_store_snapshot(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .volumes
        .iter()
        .map(|(id, record)| (id.clone(), format!("{record:?}")))
        .collect()
}

/// `CreateVolume` in `region` with `body`, returning the minted id. The sibling helpers
/// take a zone; these rows take a whole body, because the volume type is one of the
/// dimensions they span.
async fn create_volume_with(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: Value,
) -> String {
    let (status, payload) = call_in_region(router, region, "CreateVolume", body).await;
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

#[tokio::test]
async fn an_accepted_attach_persists_exactly_the_instance_and_the_device_it_named() {
    let (router, engine) = harness();

    for row in attach_rows() {
        let case = row.case.as_str();
        let instance = create_instance(
            &router,
            row.region,
            case,
            row.placed.then_some(row.zone.as_str()),
        )
        .await;
        let volume = create_volume_with(&router, row.region, case, row.volume.clone()).await;

        // What the create left, so the attach's effect can be told apart from it.
        let before = stored_volume_in(&engine, row.region, &volume)
            .unwrap_or_else(|| panic!("case {case:?}: the create stored no volume {volume:?}"));
        assert!(
            before.attachment.is_none(),
            "case {case:?}: a freshly created volume must hold no attachment, got {:?}",
            before.attachment
        );

        let (status, payload) = call_in_region(
            &router,
            row.region,
            "AssociateVolume",
            attach_body(&instance, &volume, &row.device),
        )
        .await;
        assert_served(case, "AssociateVolume", status, &payload);
        assert_eq!(
            status, 200,
            "case {case:?}: expected the attach inside one zone to be performed, got {status} \
             with {payload}"
        );
        assert!(
            payload.is_null() || payload.as_object().is_some_and(|body| body.is_empty()),
            "case {case:?}: AssociateVolumeResponse models no members, got {payload}"
        );

        // The stored attachment carries the two values the request named, and nothing
        // else about the volume moved: that relation is the whole effect of the call.
        let attached = stored_volume_in(&engine, row.region, &volume)
            .unwrap_or_else(|| panic!("case {case:?}: the store lost volume {volume:?}"));
        let attachment = attached.attachment.clone().unwrap_or_else(|| {
            panic!("case {case:?}: the accepted attach stored no attachment for {volume:?}")
        });
        assert_eq!(
            attachment.workspace_instance_id, instance,
            "case {case:?}: expected the attachment to name {instance:?}, got {attachment:?}"
        );
        assert_eq!(
            attachment.device, row.device,
            "case {case:?}: expected the attachment to hold the device path the request named, \
             got {attachment:?}"
        );
        let mut without_attachment = attached.clone();
        without_attachment.attachment = None;
        assert_eq!(
            format!("{without_attachment:?}"),
            format!("{before:?}"),
            "case {case:?}: an attach must change nothing but the attachment, got \
             {without_attachment:?}"
        );

        // The same relation read back through the operations it changes: an attached
        // volume cannot be deleted.
        let (status, payload) = call_in_region(
            &router,
            row.region,
            "DeleteVolume",
            json!({ "VolumeId": &volume }),
        )
        .await;
        assert_served(case, "DeleteVolume", status, &payload);
        assert_eq!(
            status, 409,
            "case {case:?}: an attached volume must answer the in-use conflict, got {status} with \
             {payload}"
        );
        assert_eq!(
            message(&payload),
            Some(wsinst::state::VOLUME_IN_USE_MESSAGE),
            "case {case:?}: expected the service's own in-use wording, got {payload}"
        );

        // The detach removes that one attachment, leaving the record exactly as the create
        // left it — the attach wrote nowhere else for a detach to miss.
        let (status, payload) = call_in_region(
            &router,
            row.region,
            "DisassociateVolume",
            attach_body(&instance, &volume, &row.device),
        )
        .await;
        assert_served(case, "DisassociateVolume", status, &payload);
        assert_eq!(
            status, 200,
            "case {case:?}: the stored attachment must be the one a detach removes, got {status} \
             with {payload}"
        );
        let detached = stored_volume_in(&engine, row.region, &volume)
            .unwrap_or_else(|| panic!("case {case:?}: the store lost volume {volume:?}"));
        assert!(
            detached.attachment.is_none(),
            "case {case:?}: the detach must leave no attachment, got {:?}",
            detached.attachment
        );
        assert_eq!(
            format!("{detached:?}"),
            format!("{before:?}"),
            "case {case:?}: a detached volume must be the record the create left, got {detached:?}"
        );

        // And a detached volume is deletable, which takes its record with it.
        let (status, payload) = call_in_region(
            &router,
            row.region,
            "DeleteVolume",
            json!({ "VolumeId": &volume }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a detached volume must be deletable, got {status} with {payload}"
        );
        assert!(
            stored_volume_in(&engine, row.region, &volume).is_none(),
            "case {case:?}: the delete must remove the record of {volume:?}"
        );
    }
}

/// One refused-attach case: the Region the call is signed for, the request, and the
/// modelled answer it must get.
struct RefusalRow {
    case: &'static str,
    region: String,
    body: Value,
    status: u16,
    code: &'static str,
    /// The service's fixed wording for the refusal, where it publishes one.
    message: Option<&'static str>,
    /// The input member a `ValidationException` row names in its `FieldList`.
    field: Option<&'static str>,
    /// The conflicting or missing resource the refusal names, where the shape carries
    /// one.
    resource_id: Option<String>,
}

#[tokio::test]
async fn a_refused_attach_persists_nothing() {
    let (router, engine) = harness();
    let region = DEFAULT_TEST_REGION;
    let zones = wsinst::state::availability_zones_in_region(region);
    assert!(
        zones.len() > 1,
        "case fixture: the zone row needs a Region with more than one zone, got {zones:?}"
    );
    let home = zones[0].clone();
    let other_zone = zones[1].clone();
    let foreign = "ap-northeast-2";
    assert!(
        wsinst::state::SUPPORTED_REGIONS.contains(&foreign),
        "case fixture: the foreign Region must be one the service serves, which are {:?}",
        wsinst::state::SUPPORTED_REGIONS
    );

    let instance = create_instance(&router, region, "fixture", Some(&home)).await;
    let attached = create_volume(&router, simple_create(&home)).await;
    let free = create_volume(&router, simple_create(&home)).await;
    let elsewhere = create_volume(&router, simple_create(&other_zone)).await;
    let device = "/dev/sdf";
    let spare_device = "/dev/sdg";

    ok(
        &router,
        "AssociateVolume",
        attach_body(&instance, &attached, device),
    )
    .await;

    // Well-formed per their modelled shapes, stored by nobody.
    let unknown_instance = format!("wsinst-{}", "a".repeat(12));
    let unknown_volume = "vol-abc123XYZ".to_string();
    let rows = vec![
        RefusalRow {
            case: "an instance id the store does not hold",
            region: region.to_string(),
            body: attach_body(&unknown_instance, &free, spare_device),
            status: wsinst::state::RESOURCE_NOT_FOUND_STATUS,
            code: "ResourceNotFoundException",
            message: Some(wsinst::state::INSTANCE_NOT_FOUND_MESSAGE),
            field: None,
            resource_id: Some(unknown_instance.clone()),
        },
        RefusalRow {
            case: "a volume id the store does not hold",
            region: region.to_string(),
            body: attach_body(&instance, &unknown_volume, spare_device),
            status: wsinst::state::RESOURCE_NOT_FOUND_STATUS,
            code: "ResourceNotFoundException",
            message: Some(wsinst::state::VOLUME_NOT_FOUND_MESSAGE),
            field: None,
            resource_id: Some(unknown_volume.clone()),
        },
        RefusalRow {
            case: "a volume in another Availability Zone",
            region: region.to_string(),
            body: attach_body(&instance, &elsewhere, spare_device),
            status: 400,
            code: "ValidationException",
            message: Some(wsinst::state::VOLUME_ZONE_MISMATCH_MESSAGE),
            field: Some(wsinst::state::VOLUME_ID_MEMBER),
            resource_id: None,
        },
        RefusalRow {
            case: "a volume that already holds an attachment",
            region: region.to_string(),
            body: attach_body(&instance, &attached, spare_device),
            status: 409,
            code: "ConflictException",
            message: Some(wsinst::state::VOLUME_ALREADY_ATTACHED_MESSAGE),
            field: None,
            resource_id: Some(attached.clone()),
        },
        RefusalRow {
            case: "a device path another volume occupies",
            region: region.to_string(),
            body: attach_body(&instance, &free, device),
            status: 409,
            code: "ConflictException",
            message: Some(wsinst::state::DEVICE_ALREADY_IN_USE_MESSAGE),
            field: None,
            resource_id: Some(attached.clone()),
        },
        RefusalRow {
            // `DeviceName`'s `len 0..32` admits the empty string, so this one reaches the
            // handler: the refusal names the member it was sent for rather than a
            // resource.
            case: "a Device that names no device path",
            region: region.to_string(),
            body: attach_body(&instance, &free, ""),
            status: 400,
            code: "ValidationException",
            message: None,
            field: Some("Device"),
            resource_id: None,
        },
        RefusalRow {
            case: "a pair of ids reached from another Region",
            region: foreign.to_string(),
            body: attach_body(&instance, &free, spare_device),
            status: wsinst::state::RESOURCE_NOT_FOUND_STATUS,
            code: "ResourceNotFoundException",
            message: Some(wsinst::state::INSTANCE_NOT_FOUND_MESSAGE),
            field: None,
            resource_id: Some(instance.clone()),
        },
    ];

    for row in rows {
        let case = row.case;
        let before = volume_store_snapshot(&engine);
        let (status, payload) =
            call_in_region(&router, &row.region, "AssociateVolume", row.body.clone()).await;
        assert_served(case, "AssociateVolume", status, &payload);
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {payload}",
            row.status
        );
        assert_eq!(
            error_code(&payload),
            row.code,
            "case {case:?}: expected {}, got {payload}",
            row.code
        );
        if let Some(expected) = row.message {
            assert_eq!(
                message(&payload),
                Some(expected),
                "case {case:?}: expected the service's own wording, got {payload}"
            );
        }
        if let Some(expected) = row.field {
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
                vec![expected],
                "case {case:?}: expected the refusal to name {expected:?}, got {payload}"
            );
        }
        if let Some(expected) = row.resource_id.as_deref() {
            assert_eq!(
                payload["ResourceId"].as_str(),
                Some(expected),
                "case {case:?}: expected the refusal to name {expected:?}, got {payload}"
            );
        }
        assert_eq!(
            volume_store_snapshot(&engine),
            before,
            "case {case:?}: a refused attach must persist nothing"
        );
    }

    // Each refusal was about its own request, not a blanket one: the free volume still
    // attaches to the same instance at a device nothing occupies, and the attachment the
    // fixture made is still the one the store holds.
    ok(
        &router,
        "AssociateVolume",
        attach_body(&instance, &free, spare_device),
    )
    .await;
    for (volume, expected_device) in [(&attached, device), (&free, spare_device)] {
        let record = stored_volume_in(&engine, region, volume)
            .unwrap_or_else(|| panic!("case after the refusals: the store lost {volume:?}"));
        let attachment = record.attachment.unwrap_or_else(|| {
            panic!("case after the refusals: {volume:?} must still hold its attachment")
        });
        assert_eq!(
            (
                attachment.workspace_instance_id.as_str(),
                attachment.device.as_str()
            ),
            (instance.as_str(), expected_device),
            "case after the refusals: expected {volume:?} attached to {instance:?} at \
             {expected_device:?}"
        );
    }
}

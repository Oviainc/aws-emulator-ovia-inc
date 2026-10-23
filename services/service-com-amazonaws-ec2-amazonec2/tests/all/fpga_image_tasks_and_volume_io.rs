// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the FPGA-image, AMI-task, Recycle Bin, volume-I/O and export-task
//! slice: `CreateFpgaImage`, `DeleteFpgaImage`, `ResetFpgaImageAttribute`,
//! `AttachImageWatermark`, `DeleteImageUsageReport`, `CreateRestoreImageTask`,
//! `ListVolumesInRecycleBin`, `EnableVolumeIO`, `DetachVolume` and `CancelExportTask`,
//! driven over the wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. AN AMAZON S3 STORAGE LOCATION THE SERVICE CANNOT READ IS REFUSED with 400
//!    `InaccessibleStorageLocation`, never accepted: a bucket name outside Amazon S3's
//!    published naming rules names no bucket, and a stored object nothing holds cannot be
//!    restored from;
//! 2. `DryRun` ASKS WHETHER THE CALLER MAY MAKE THE REQUEST, so an authorized `DryRun` call
//!    is answered 412 `DryRunOperation` once the request's own members have been read and
//!    before any resource is resolved or written — including for a resource the account
//!    holds nothing for;
//! 3. A RESOURCE ID IS RESOLVED MALFORMED BEFORE NOT FOUND: something that is not an id of
//!    that kind at all is `Invalid<Resource>ID.Malformed`, and a well-formed id the account
//!    holds nothing for is `Invalid<Resource>.NotFound`;
//! 4. A REQUEST THE RESOURCE'S STATE DOES NOT ADMIT IS REFUSED with `IncorrectState` —
//!    enabling I/O that is already enabled, detaching a volume nothing holds, cancelling an
//!    export task that is not active;
//! 5. EVERY WRITE IS READ BACK OUT OF THE STORE: a created AFI is the one a delete removes
//!    and a reset resets, an attached watermark is the one `DescribeImages` reports, a
//!    detached volume is the one `DescribeVolumes` reports, and a report of the Recycle Bin
//!    is the bin's own contents.
//!
//! Each table row is a distinct input along what the rule generalises over — which naming
//! rule a bucket name breaks and at which published bound, which operation carries the
//! `DryRun`, which id shape, which state, which page size — so no row is the specification.

use std::time::{Duration, Instant, SystemTime};

use ec2::ops::types::{
    AttachVolumeOutput, ExportTask, ExportToS3Task, ImageUsageReport, InstanceExportDetails,
    LoadPermission,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, EXPORT_TASK_STATE_ACTIVE,
    EXPORT_TASK_STATE_CANCELLED, EXPORT_TASK_STATE_CANCELLING, EXPORT_TASK_STATE_COMPLETED,
    FPGA_IMAGE_STATE_FAILED, FPGA_IMAGE_STATE_PENDING, ImageSpec, MAX_S3_BUCKET_NAME_CHARS,
    MIN_S3_BUCKET_NAME_CHARS, RECYCLE_BIN_PAGE_SIZE_RANGE, VOLUME_ATTACHMENT_STATE_ATTACHED,
    VOLUME_ATTACHMENT_STATE_DETACHING, VOLUME_STATE_AVAILABLE, VOLUME_STATE_DELETED,
    VOLUME_STATE_IN_USE, WATERMARK_NAME_PUNCTUATION, ExportTaskRecord, system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, indexed, unescaped};
use super::volume_lifecycle::{REGION, p, past_the_transition, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The HTTP status EC2 answers a `DryRun` request with: 412, carrying `DryRunOperation`.
const DRY_RUN_STATUS: u16 = 412;

/// A well-formed id of each kind that the account holds no resource for. The long (17 hex
/// digit) form, which is what EC2 mints today.
const UNKNOWN_FPGA_IMAGE_ID: &str = "afi-0123456789abcdef0";
const UNKNOWN_IMAGE_ID: &str = "ami-0123456789abcdef0";
const UNKNOWN_VOLUME_ID: &str = "vol-0123456789abcdef0";
const UNKNOWN_INSTANCE_ID: &str = "i-0123456789abcdef0";
const UNKNOWN_REPORT_ID: &str = "imgrpt-0123456789abcdef0";

/// A bucket name that satisfies every one of Amazon S3's general-purpose bucket naming
/// rules, for the rows that are about something other than the bucket.
const READABLE_BUCKET: &str = "my-design-checkpoints";

/// One `action` call signed for the Region the suite works in.
async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The parameters of a `CreateFpgaImage` whose design checkpoint is in `bucket`.
fn create_fpga_image_params(bucket: &str) -> Params {
    vec![
        p("InputStorageLocation.Bucket", bucket),
        p("InputStorageLocation.Key", "dcp/checkpoint.tar"),
    ]
}

/// Create one AFI from a readable storage location, and answer its id and global id.
async fn create_fpga_image(router: &MultiServiceRouter) -> (String, String) {
    let (status, body) = op(
        router,
        "CreateFpgaImage",
        &create_fpga_image_params(READABLE_BUCKET),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating an AFI: expected HTTP 200, got {status} with body {body}"
    );
    (
        element(&body, "fpgaImageId").unwrap_or_default(),
        element(&body, "fpgaImageGlobalId").unwrap_or_default(),
    )
}

/// Create one 8 GiB volume in the Region's first Availability Zone, and answer its id.
async fn create_volume(router: &MultiServiceRouter) -> String {
    let (status, body) = op(
        router,
        "CreateVolume",
        &[p("AvailabilityZone", zone_name(0)), p("Size", "8")],
    )
    .await;
    assert_eq!(
        status, 200,
        "creating a volume: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "volumeId").expect("CreateVolume reports the volume it minted")
}

/// The bucket names rule 1 is written against.
///
/// Every row names the sentence of the Amazon S3 User Guide's "General purpose bucket naming
/// rules" it exercises, and the accepted rows sit at each published bound while the refused
/// ones sit one character past it.
fn bucket_name_rows() -> Vec<(&'static str, String, bool)> {
    vec![
        (
            "three characters, the published minimum",
            "abc".to_string(),
            true,
        ),
        (
            "63 characters, the published maximum",
            "a".repeat(MAX_S3_BUCKET_NAME_CHARS),
            true,
        ),
        (
            "dots and hyphens between alphanumerics",
            "my.bucket-1".to_string(),
            true,
        ),
        ("digits only", "0123456789".to_string(), true),
        (
            "two characters, one below the minimum",
            "a".repeat(MIN_S3_BUCKET_NAME_CHARS - 1),
            false,
        ),
        (
            "64 characters, one above the maximum",
            "a".repeat(MAX_S3_BUCKET_NAME_CHARS + 1),
            false,
        ),
        ("an uppercase letter", "MyBucket".to_string(), false),
        ("an underscore", "my_bucket".to_string(), false),
        ("beginning with a dot", ".mybucket".to_string(), false),
        ("ending with a hyphen", "mybucket-".to_string(), false),
        ("two adjacent periods", "my..bucket".to_string(), false),
        (
            "formatted as an IP address",
            "192.168.5.4".to_string(),
            false,
        ),
        ("the reserved prefix xn--", "xn--mybucket".to_string(), false),
        (
            "the reserved prefix sthree-",
            "sthree-mybucket".to_string(),
            false,
        ),
        (
            "the reserved prefix amzn-s3-demo-",
            "amzn-s3-demo-bucket".to_string(),
            false,
        ),
        (
            "the reserved suffix -s3alias",
            "mybucket-s3alias".to_string(),
            false,
        ),
        (
            "the reserved suffix --ol-s3",
            "mybucket--ol-s3".to_string(),
            false,
        ),
        (
            "the reserved suffix .mrap",
            "mybucket.mrap".to_string(),
            false,
        ),
        (
            "the reserved suffix --x-s3",
            "mybucket--x-s3".to_string(),
            false,
        ),
        (
            "the reserved suffix --table-s3",
            "mybucket--table-s3".to_string(),
            false,
        ),
    ]
}

// ---------------------------------------------------------------------------
// RULE 1: a storage location the service cannot read is refused
// ---------------------------------------------------------------------------

/// `CreateFpgaImage` refuses every bucket name outside Amazon S3's naming rules with
/// `InaccessibleStorageLocation`, and accepts every name inside them.
#[tokio::test]
async fn create_fpga_image_reads_only_an_accessible_storage_location() {
    let (router, _engine) = emulator();

    for (case, bucket, accessible) in bucket_name_rows() {
        let (status, body) = op(&router, "CreateFpgaImage", &create_fpga_image_params(&bucket)).await;
        if accessible {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for bucket {bucket:?}, \
                 got {status} with body {body}"
            );
            let fpga_image_id = element(&body, "fpgaImageId").unwrap_or_default();
            let global_id = element(&body, "fpgaImageGlobalId").unwrap_or_default();
            assert!(
                fpga_image_id.starts_with("afi-"),
                "case {case:?}: expected an afi- id, got {fpga_image_id:?}"
            );
            assert!(
                global_id.starts_with("agfi-"),
                "case {case:?}: expected an agfi- global id, got {global_id:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for bucket {bucket:?}, \
                 got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InaccessibleStorageLocation",
                "case {case:?}: expected InaccessibleStorageLocation for bucket {bucket:?}, \
                 got {body}"
            );
        }
    }
}

/// The same rule on the logs location `CreateFpgaImage` also takes, and on a location that
/// names no bucket or no key at all — which names no object, so there is nothing to read.
#[tokio::test]
async fn create_fpga_image_holds_every_storage_location_to_the_one_rule() {
    let (router, _engine) = emulator();

    let logs_rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a logs location whose bucket name breaks the naming rules",
            {
                let mut params = create_fpga_image_params(READABLE_BUCKET);
                params.push(p("LogsStorageLocation.Bucket", "Logs_Bucket"));
                params.push(p("LogsStorageLocation.Key", "logs/"));
                params
            },
            400,
            "InaccessibleStorageLocation",
        ),
        (
            "a logs location that names a bucket but no key",
            {
                let mut params = create_fpga_image_params(READABLE_BUCKET);
                params.push(p("LogsStorageLocation.Bucket", "my-afi-logs"));
                params
            },
            400,
            "MissingParameter",
        ),
        (
            "an input location that names a key but no bucket",
            vec![p("InputStorageLocation.Key", "dcp/checkpoint.tar")],
            400,
            "MissingParameter",
        ),
        (
            "an input location that names no key",
            vec![p("InputStorageLocation.Bucket", READABLE_BUCKET)],
            400,
            "MissingParameter",
        ),
        (
            "both locations readable",
            {
                let mut params = create_fpga_image_params(READABLE_BUCKET);
                params.push(p("LogsStorageLocation.Bucket", "my-afi-logs"));
                params.push(p("LogsStorageLocation.Key", "logs/afi.log"));
                params
            },
            200,
            "",
        ),
    ];

    for (case, params, expected_status, expected_code) in logs_rows {
        let (status, body) = op(&router, "CreateFpgaImage", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
        }
    }
}

/// `CreateRestoreImageTask` keeps the same rule: a bucket name outside the naming rules is
/// refused for being no bucket, and a readable bucket name is refused for the object, whose
/// location the message names — this emulator holds no stored AMI object to restore from, so
/// no AMI is ever minted.
#[tokio::test]
async fn create_restore_image_task_refuses_a_location_it_cannot_read() {
    let (router, _engine) = emulator();

    for (case, bucket, readable_name) in bucket_name_rows() {
        let (status, body) = op(
            &router,
            "CreateRestoreImageTask",
            &[p("Bucket", bucket.clone()), p("ObjectKey", "ami/stored.bin")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for bucket {bucket:?}, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InaccessibleStorageLocation",
            "case {case:?}: expected InaccessibleStorageLocation for bucket {bucket:?}, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        if readable_name {
            assert!(
                message.contains(&format!("s3://{bucket}/ami/stored.bin")),
                "case {case:?}: expected the message to name the object that could not be \
                 read, got {message:?}"
            );
        }
        assert!(
            element(&body, "imageId").is_none(),
            "case {case:?}: expected no AMI to be minted, got {body}"
        );
    }
}

/// A `CreateRestoreImageTask` tag specification may only name the AMI and the snapshots it
/// restores, so one naming any other resource type is refused rather than discarded.
#[tokio::test]
async fn create_restore_image_task_tags_only_what_it_restores() {
    let (router, _engine) = emulator();

    let rows: [(&str, &str, &str); 4] = [
        ("the AMI", "image", "InaccessibleStorageLocation"),
        ("the snapshots", "snapshot", "InaccessibleStorageLocation"),
        ("a volume", "volume", "InvalidParameterValue"),
        ("an instance", "instance", "InvalidParameterValue"),
    ];
    for (case, resource_type, expected_code) in rows {
        let (status, body) = op(
            &router,
            "CreateRestoreImageTask",
            &[
                p("Bucket", READABLE_BUCKET),
                p("ObjectKey", "ami/stored.bin"),
                p("TagSpecification.1.ResourceType", resource_type),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "restored"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: DryRun is a question about the caller
// ---------------------------------------------------------------------------

/// Every operation of this slice that models a `DryRun` member answers an authorized
/// `DryRun` request with 412 `DryRunOperation` — including one naming a resource the account
/// holds nothing for, because the flag asks whether the CALLER MAY make the request and not
/// whether the resource is there.
#[tokio::test]
async fn dry_run_is_answered_before_any_resource_is_resolved() {
    let (router, engine) = emulator();

    let rows: Vec<(&str, Params)> = vec![
        ("CreateFpgaImage", create_fpga_image_params(READABLE_BUCKET)),
        (
            "DeleteFpgaImage",
            vec![p("FpgaImageId", UNKNOWN_FPGA_IMAGE_ID)],
        ),
        (
            "ResetFpgaImageAttribute",
            vec![
                p("FpgaImageId", UNKNOWN_FPGA_IMAGE_ID),
                p("Attribute", "loadPermission"),
            ],
        ),
        (
            "AttachImageWatermark",
            vec![
                p("ImageId", UNKNOWN_IMAGE_ID),
                p("WatermarkName", "approvedAmi"),
            ],
        ),
        (
            "DeleteImageUsageReport",
            vec![p("ReportId", UNKNOWN_REPORT_ID)],
        ),
        (
            "CreateRestoreImageTask",
            vec![p("Bucket", READABLE_BUCKET), p("ObjectKey", "ami/stored.bin")],
        ),
        (
            "ListVolumesInRecycleBin",
            indexed("VolumeId", &[UNKNOWN_VOLUME_ID]),
        ),
        ("EnableVolumeIO", vec![p("VolumeId", UNKNOWN_VOLUME_ID)]),
        ("DetachVolume", vec![p("VolumeId", UNKNOWN_VOLUME_ID)]),
    ];

    for (action, params) in rows {
        let mut with_dry_run = params.clone();
        with_dry_run.push(p("DryRun", "true"));
        let (status, body) = op(&router, action, &with_dry_run).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {action:?}: expected HTTP {DRY_RUN_STATUS} for a DryRun request, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {action:?}: expected DryRunOperation, got {body}"
        );
    }

    // A DryRun writes nothing: no AFI was created by the row above, and the store is as a
    // fresh account's is.
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    assert!(
        guard.fpga_images.is_empty(),
        "expected a DryRun CreateFpgaImage to store no AFI, got {} stored",
        guard.fpga_images.len()
    );
}

// ---------------------------------------------------------------------------
// RULE 3 and RULE 5: the FPGA-image slice
// ---------------------------------------------------------------------------

/// `CreateFpgaImage` hands back a `pending` AFI whose creation then FAILS on the clock,
/// because nothing read the design checkpoint: the AFI is stored, its ids are the ones the
/// call reported, and one `ClientToken` names one AFI.
#[tokio::test]
async fn a_created_afi_is_pending_and_its_creation_fails_on_the_clock() {
    let (router, engine) = emulator();

    let (fpga_image_id, global_id) = create_fpga_image(&router).await;
    assert_ne!(
        fpga_image_id, global_id,
        "the AFI id and its global id are different identifiers, got {fpga_image_id:?} twice"
    );

    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let record = guard
            .fpga_images
            .get(&fpga_image_id)
            .expect("the AFI the create reported is stored");
        let state = record.state_at(Instant::now());
        assert_eq!(
            state.code.as_deref(),
            Some(FPGA_IMAGE_STATE_PENDING),
            "straight after the create: expected a {FPGA_IMAGE_STATE_PENDING} AFI, got {state:?}"
        );
        let reported = record.view_at(Instant::now());
        assert_eq!(
            reported.state.and_then(|state| state.code),
            Some(FPGA_IMAGE_STATE_PENDING.to_string()),
            "the AFI as a report of it reads carries the state the clock has reached"
        );
        assert_eq!(
            record.view.fpga_image_global_id.as_deref(),
            Some(global_id.as_str()),
            "the stored global id is the one the create reported, got {record:?}"
        );
    }

    past_the_transition().await;

    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let record = guard
            .fpga_images
            .get(&fpga_image_id)
            .expect("the AFI is still stored");
        let state = record.state_at(Instant::now());
        assert_eq!(
            state.code.as_deref(),
            Some(FPGA_IMAGE_STATE_FAILED),
            "past the transition: expected a {FPGA_IMAGE_STATE_FAILED} AFI, got {state:?}"
        );
        let message = state.message.unwrap_or_default();
        assert!(
            message.contains(READABLE_BUCKET),
            "expected the failure message to name the design checkpoint, got {message:?}"
        );
    }

    // "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
    // request": one token names the one AFI it minted.
    let mut with_token = create_fpga_image_params(READABLE_BUCKET);
    with_token.push(p("ClientToken", "one-afi-please"));
    let (first_status, first) = op(&router, "CreateFpgaImage", &with_token).await;
    let (second_status, second) = op(&router, "CreateFpgaImage", &with_token).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "a replayed create: expected HTTP 200 twice, got {first_status} and {second_status}"
    );
    assert_eq!(
        element(&first, "fpgaImageId"),
        element(&second, "fpgaImageId"),
        "a replayed ClientToken names the AFI it minted, got {first} then {second}"
    );
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    assert_eq!(
        guard.fpga_images.len(),
        2,
        "a replayed create stores nothing, so two creates leave two AFIs, got {}",
        guard.fpga_images.len()
    );
}

/// An AFI id is resolved MALFORMED BEFORE NOT FOUND, and the same two answers come from
/// every operation that takes one.
#[tokio::test]
async fn an_fpga_image_id_is_resolved_malformed_before_not_found() {
    let (router, _engine) = emulator();

    // `(case, the id, the code it is answered with)`: the ids differ along prefix, suffix
    // length and alphabet, and the two accepted id FORMS (8 and 17 hex digits) are the rows
    // answered as not found.
    let id_rows: [(&str, &str, &str); 9] = [
        (
            "the long id form, which nothing is stored under",
            UNKNOWN_FPGA_IMAGE_ID,
            "InvalidFpgaImageID.NotFound",
        ),
        (
            "the short id form, which nothing is stored under",
            "afi-01234567",
            "InvalidFpgaImageID.NotFound",
        ),
        ("no prefix at all", "0123456789abcdef0", "InvalidFpgaImageID.Malformed"),
        (
            "another resource's prefix",
            "ami-0123456789abcdef0",
            "InvalidFpgaImageID.Malformed",
        ),
        ("the prefix alone", "afi-", "InvalidFpgaImageID.Malformed"),
        (
            "a suffix that is too short",
            "afi-0123456",
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "a suffix that is too long",
            "afi-0123456789abcdef01",
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "a suffix outside the hexadecimal alphabet",
            "afi-0123456789abcdefg",
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "an uppercase suffix",
            "afi-0123456789ABCDEF0",
            "InvalidFpgaImageID.Malformed",
        ),
    ];

    for (case, fpga_image_id, expected_code) in id_rows {
        for action in ["DeleteFpgaImage", "ResetFpgaImageAttribute"] {
            let mut params = vec![p("FpgaImageId", fpga_image_id)];
            if action == "ResetFpgaImageAttribute" {
                params.push(p("Attribute", "loadPermission"));
            }
            let (status, body) = op(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} on {action}: expected {expected_code}, got {body}"
            );
        }
    }
}

/// An AFI round-trips through the operations that change it: a reset clears the load
/// permissions it holds, a delete takes it out of the store, and a second delete or reset of
/// the same id is answered as not found.
#[tokio::test]
async fn an_afi_round_trips_through_reset_and_delete() {
    let (router, engine) = emulator();

    let (fpga_image_id, _) = create_fpga_image(&router).await;

    // A load permission an attribute change would have granted, so the reset has something
    // to clear.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let record = guard
            .fpga_images
            .get_mut(&fpga_image_id)
            .expect("the AFI the create reported is stored");
        record.load_permissions = vec![LoadPermission {
            user_id: Some("210987654321".to_string()),
            group: None,
        }];
    }

    let (status, body) = op(
        &router,
        "ResetFpgaImageAttribute",
        &[
            p("FpgaImageId", &fpga_image_id),
            p("Attribute", "loadPermission"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "resetting the load permissions: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "resetting the load permissions reports true, got {body}"
    );
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let record = guard
            .fpga_images
            .get(&fpga_image_id)
            .expect("the AFI is still stored");
        assert!(
            record.load_permissions.is_empty(),
            "a reset clears the load permissions, got {:?}",
            record.load_permissions
        );
    }

    // A reset with no attribute names nothing to reset, and one naming an attribute that
    // cannot be reset is refused by the modelled enum before the handler runs.
    let attribute_rows: [(&str, Params, &str); 2] = [
        (
            "no attribute at all",
            vec![p("FpgaImageId", &fpga_image_id)],
            "MissingParameter",
        ),
        (
            "an attribute that cannot be reset",
            vec![
                p("FpgaImageId", &fpga_image_id),
                p("Attribute", "description"),
            ],
            "ValidationError",
        ),
    ];
    for (case, params, expected_code) in attribute_rows {
        let (status, body) = op(&router, "ResetFpgaImageAttribute", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    let (status, body) = op(&router, "DeleteFpgaImage", &[p("FpgaImageId", &fpga_image_id)]).await;
    assert_eq!(
        status, 200,
        "deleting the AFI: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "deleting the AFI reports true, got {body}"
    );
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        assert!(
            !guard.fpga_images.contains_key(&fpga_image_id),
            "the delete takes the AFI out of the store, got {} stored",
            guard.fpga_images.len()
        );
    }

    // The AFI is gone, so every operation that takes its id answers not found.
    for action in ["DeleteFpgaImage", "ResetFpgaImageAttribute"] {
        let mut params = vec![p("FpgaImageId", &fpga_image_id)];
        if action == "ResetFpgaImageAttribute" {
            params.push(p("Attribute", "loadPermission"));
        }
        let (status, body) = op(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "{action} after the delete: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidFpgaImageID.NotFound",
            "{action} after the delete: expected InvalidFpgaImageID.NotFound, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// AMI watermarks
// ---------------------------------------------------------------------------

/// Store one private AMI of the calling account, and answer its `ImageId`.
fn seed_private_image(engine: &engine::InMemoryEngine, name: &str) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard.insert_image(ImageSpec {
        name: name.to_string(),
        description: "an AMI this account owns".to_string(),
        ..ImageSpec::default()
    })
}

/// The id of one of the public AMIs a fresh account can already see.
fn public_image_id(engine: &engine::InMemoryEngine) -> String {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .images
        .iter()
        .find(|(_, image)| image.public == Some(true))
        .map(|(image_id, _)| image_id.clone())
        .expect("the published catalog holds a public AMI")
}

/// A watermark key is DERIVED from the calling account and the name the request gave, and
/// the watermark it names is the one `DescribeImages` reports for that AMI. The name's
/// character set is the one its own constraints publish.
#[tokio::test]
async fn an_attached_watermark_is_keyed_by_account_and_name_and_reported_by_describe_images() {
    let (router, engine) = emulator();
    let image_id = seed_private_image(&engine, "owned-ami");
    let account = engine::emulator_account_id().to_string();

    // `(case, the watermark name, whether the constraints admit it)`. The accepted rows
    // cover every punctuation character the member documents
    // (`WATERMARK_NAME_PUNCTUATION`); the refused ones are characters outside that set.
    let name_rows: [(&str, &str, bool); 14] = [
        ("letters and digits", "approvedAmi2025", true),
        ("parentheses", "approved(ami)", true),
        ("square brackets", "approved[ami]", true),
        ("a space", "approved ami", true),
        ("a period", "approved.ami", true),
        ("a slash", "approved/ami", true),
        ("a dash", "approved-ami", true),
        ("a single quote", "approved'ami", true),
        ("an at-sign", "approved@ami", true),
        ("an underscore", "approved_ami", true),
        ("a hash", "approved#ami", false),
        ("a comma", "approved,ami", false),
        ("a colon, which the key format reserves", "approved:ami", false),
        ("an exclamation mark", "approved!ami", false),
    ];

    let mut expected_keys: Vec<String> = Vec::new();
    for (case, watermark_name, admitted) in name_rows {
        let (status, body) = op(
            &router,
            "AttachImageWatermark",
            &[p("ImageId", &image_id), p("WatermarkName", watermark_name)],
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for name {watermark_name:?}, \
                 got {status} with body {body}"
            );
            let key = unescaped(&element(&body, "watermarkKey").unwrap_or_default());
            assert_eq!(
                key,
                format!("{account}:{watermark_name}"),
                "case {case:?}: expected the accountId:watermarkName key, got {key:?}"
            );
            expected_keys.push(key);
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for name {watermark_name:?}, \
                 got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                unescaped(&element(&body, "Message").unwrap_or_default())
                    .contains(WATERMARK_NAME_PUNCTUATION),
                "case {case:?}: expected the message to name the punctuation the member \
                 allows, got {body}"
            );
        }
    }

    // Attaching the same name again names the watermark the AMI already carries: the same
    // key, and no second row.
    let (status, body) = op(
        &router,
        "AttachImageWatermark",
        &[p("ImageId", &image_id), p("WatermarkName", "approved-ami")],
    )
    .await;
    assert_eq!(
        status, 200,
        "re-attaching a watermark: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        unescaped(&element(&body, "watermarkKey").unwrap_or_default()),
        format!("{account}:approved-ami"),
        "re-attaching a watermark reports the key it already has, got {body}"
    );

    // RULE 5: every attached watermark is read back through `DescribeImages`.
    let (status, described) = op(&router, "DescribeImages", &indexed("ImageId", &[&image_id])).await;
    assert_eq!(
        status, 200,
        "describing the watermarked AMI: expected HTTP 200, got {status} with body {described}"
    );
    let mut reported: Vec<String> = elements(&described, "watermarkKey")
        .iter()
        .map(|key| unescaped(key))
        .collect();
    reported.sort();
    let mut wanted = expected_keys.clone();
    wanted.sort();
    assert_eq!(
        reported, wanted,
        "expected DescribeImages to report exactly the attached watermarks"
    );
    assert!(
        elements(&described, "sourceImageId")
            .iter()
            .all(|id| *id == image_id),
        "every watermark names the AMI it was attached to, got {described}"
    );
    assert!(
        elements(&described, "sourceImageRegion")
            .iter()
            .all(|region| region == REGION),
        "every watermark names the Region it was attached in, got {described}"
    );
}

/// An AMI this call cannot watermark is refused: a malformed id, an id the account can see no
/// image for, and a public AMI — "watermarks cannot be added to public AMIs".
#[tokio::test]
async fn attach_image_watermark_refuses_an_ami_it_cannot_watermark() {
    let (router, engine) = emulator();
    let public_image = public_image_id(&engine);

    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a value that is not an AMI id at all",
            "not-an-ami".to_string(),
            "InvalidAMIID.Malformed",
        ),
        (
            "another resource's prefix",
            UNKNOWN_VOLUME_ID.to_string(),
            "InvalidAMIID.Malformed",
        ),
        ("the prefix alone", "ami-".to_string(), "InvalidAMIID.Malformed"),
        (
            "a suffix outside the hexadecimal alphabet",
            "ami-0123456789abcdefg".to_string(),
            "InvalidAMIID.Malformed",
        ),
        (
            "a well-formed id the account can see no image for",
            UNKNOWN_IMAGE_ID.to_string(),
            "InvalidAMIID.NotFound",
        ),
        (
            "a public AMI of the published catalog",
            public_image,
            "OperationNotPermitted",
        ),
    ];

    for (case, image_id, expected_code) in rows {
        let (status, body) = op(
            &router,
            "AttachImageWatermark",
            &[p("ImageId", &image_id), p("WatermarkName", "approvedAmi")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// AMI usage reports
// ---------------------------------------------------------------------------

/// Store one AMI usage report, and answer its `ReportId`.
fn seed_image_usage_report(engine: &engine::InMemoryEngine, image_id: &str) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let report_id = guard.next_id("imgrpt");
    guard.image_usage_reports.insert(
        report_id.clone(),
        ImageUsageReport {
            report_id: Some(report_id.clone()),
            image_id: Some(image_id.to_string()),
            state: Some("available".to_string()),
            state_reason: None,
            account_ids: Some(vec![engine::emulator_account_id().to_string()]),
            resource_types: Some(Vec::new()),
            creation_time: Some(system_time_to_date_time(SystemTime::now())),
            expiration_time: None,
            tags: Some(Vec::new()),
        },
    );
    report_id
}

/// `DeleteImageUsageReport` removes the report it names and leaves every other one, and an id
/// the account holds no report for is refused.
#[tokio::test]
async fn delete_image_usage_report_removes_the_report_it_names() {
    let (router, engine) = emulator();
    let image_id = seed_private_image(&engine, "reported-ami");
    let first = seed_image_usage_report(&engine, &image_id);
    let second = seed_image_usage_report(&engine, &image_id);

    let (status, body) = op(&router, "DeleteImageUsageReport", &[p("ReportId", &first)]).await;
    assert_eq!(
        status, 200,
        "deleting a report: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "deleting a report reports true, got {body}"
    );
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        assert_eq!(
            guard.image_usage_reports.keys().cloned().collect::<Vec<_>>(),
            vec![second.clone()],
            "the delete removes the report it named and leaves the other"
        );
    }

    // `(case, the id)`: the report that was just deleted, and one nothing was ever stored
    // under.
    for (case, report_id) in [
        ("a report that was deleted", first.as_str()),
        ("a report that was never stored", UNKNOWN_REPORT_ID),
    ] {
        let (status, body) = op(&router, "DeleteImageUsageReport", &[p("ReportId", report_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidImageUsageReportId.NotFound",
            "case {case:?}: expected InvalidImageUsageReportId.NotFound, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The Recycle Bin
// ---------------------------------------------------------------------------

/// Create `count` volumes and move them into the Recycle Bin, each with its own retention
/// period, and answer their ids in the order the bin holds them.
async fn seed_recycle_bin(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    count: usize,
) -> Vec<String> {
    let mut volume_ids = Vec::new();
    for _ in 0..count {
        volume_ids.push(create_volume(router).await);
    }
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    for (index, volume_id) in volume_ids.iter().enumerate() {
        // A retention period of the Recycle Bin rule that covered the volume: one day to one
        // year is what a rule may hold, and each volume gets a different one so no row's
        // exit time is the specification.
        let retention = Duration::from_secs(60 * 60 * 24 * (1 + index as u64));
        guard
            .move_volume_to_recycle_bin(volume_id, SystemTime::now(), retention)
            .unwrap_or_else(|| panic!("volume {volume_id} was created, so it can be binned"));
    }
    volume_ids.sort();
    volume_ids
}

/// A fresh account's Recycle Bin is empty, reporting it is a READ, and a volume that is moved
/// into it is reported there and nowhere else — `DescribeVolumes` no longer holds it.
#[tokio::test]
async fn the_recycle_bin_reports_the_volumes_it_holds_and_nothing_else() {
    let (router, engine) = emulator();

    let (status, body) = op(&router, "ListVolumesInRecycleBin", &[]).await;
    assert_eq!(
        status, 200,
        "a fresh account's bin: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "volumeId").is_empty(),
        "a fresh account has deleted no volume, so its bin is empty, got {body}"
    );
    let (_, again) = op(&router, "ListVolumesInRecycleBin", &[]).await;
    assert_eq!(
        elements(&again, "volumeId"),
        elements(&body, "volumeId"),
        "reporting the bin is a read: two calls answer the same volumes"
    );

    let kept = create_volume(&router).await;
    let binned = seed_recycle_bin(&router, &engine, 2).await;

    let reported = {
        let (status, body) = op(&router, "ListVolumesInRecycleBin", &[]).await;
        assert_eq!(
            status, 200,
            "the seeded bin: expected HTTP 200, got {status} with body {body}"
        );
        (elements(&body, "volumeId"), body)
    };
    assert_eq!(
        reported.0, binned,
        "the bin reports the volumes that were moved into it, in id order"
    );
    assert!(
        elements(&reported.1, "state")
            .iter()
            .all(|state| state == VOLUME_STATE_DELETED),
        "a volume in the bin has been deleted, so it reports {VOLUME_STATE_DELETED}, got {}",
        reported.1
    );
    assert_eq!(
        elements(&reported.1, "recycleBinEnterTime").len(),
        binned.len(),
        "every entry says when it entered the bin, got {}",
        reported.1
    );
    assert_eq!(
        elements(&reported.1, "recycleBinExitTime").len(),
        binned.len(),
        "every entry says when it will be permanently deleted, got {}",
        reported.1
    );
    assert!(
        elements(&reported.1, "size").iter().all(|size| size == "8"),
        "the entry is derived from the volume that was binned, so it carries its size, got {}",
        reported.1
    );

    // RULE 5: a binned volume is no longer one of the account's volumes, and the volume that
    // was not binned still is.
    let (status, described) = op(&router, "DescribeVolumes", &[]).await;
    assert_eq!(
        status, 200,
        "describing the volumes: expected HTTP 200, got {status} with body {described}"
    );
    assert_eq!(
        elements(&described, "volumeId"),
        vec![kept.clone()],
        "only the volume that was not binned is still one of the account's volumes"
    );
    let (status, body) = op(&router, "DescribeVolumes", &indexed("VolumeId", &[&binned[0]])).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidVolume.NotFound"),
        "a binned volume is no longer described, got {status} with body {body}"
    );
}

/// The bin's report narrows to the ids the request names, refuses an id that is not a volume
/// in the bin, and cuts the answer into `MaxResults`-sized pages the `NextToken` continues.
#[tokio::test]
async fn the_recycle_bin_report_narrows_pages_and_refuses_an_id_it_does_not_hold() {
    let (router, engine) = emulator();
    let binned = seed_recycle_bin(&router, &engine, 7).await;
    let active = create_volume(&router).await;

    // Narrowing: `(case, the ids asked for, the ids reported)`.
    let narrowing_rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one id", vec![binned[2].clone()], vec![binned[2].clone()]),
        (
            "two ids, asked for in reverse",
            vec![binned[4].clone(), binned[1].clone()],
            vec![binned[1].clone(), binned[4].clone()],
        ),
        ("no id at all", Vec::new(), binned.clone()),
    ];
    for (case, asked, expected) in narrowing_rows {
        let refs: Vec<&str> = asked.iter().map(String::as_str).collect();
        let (status, body) = op(&router, "ListVolumesInRecycleBin", &indexed("VolumeId", &refs)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, "volumeId"),
            expected,
            "case {case:?}: expected exactly the volumes asked for"
        );
    }

    // An id that is not a volume in the bin: `(case, the id, the code)`.
    let refusal_rows: Vec<(&str, String, &str)> = vec![
        (
            "a volume the account still holds",
            active.clone(),
            "InvalidVolume.NotFound",
        ),
        (
            "a volume nothing was ever created for",
            UNKNOWN_VOLUME_ID.to_string(),
            "InvalidVolume.NotFound",
        ),
        (
            "a value that is not a volume id at all",
            "not-a-volume".to_string(),
            "InvalidVolumeID.Malformed",
        ),
        (
            "another resource's prefix",
            UNKNOWN_IMAGE_ID.to_string(),
            "InvalidVolumeID.Malformed",
        ),
    ];
    for (case, volume_id, expected_code) in refusal_rows {
        let (status, body) = op(
            &router,
            "ListVolumesInRecycleBin",
            &indexed("VolumeId", &[&volume_id]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Paging at the published minimum page size: "Valid range: 5 - 500".
    let (min_page, max_page) = RECYCLE_BIN_PAGE_SIZE_RANGE;
    let (status, first) = op(
        &router,
        "ListVolumesInRecycleBin",
        &[p("MaxResults", min_page.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the first page: expected HTTP 200, got {status} with body {first}"
    );
    let first_page = elements(&first, "volumeId");
    assert_eq!(
        first_page,
        binned[..min_page as usize].to_vec(),
        "a page of {min_page} is the first {min_page} volumes of the bin"
    );
    let token = element(&first, "nextToken").expect("a volume follows the first page");
    let (status, second) = op(
        &router,
        "ListVolumesInRecycleBin",
        &[p("MaxResults", min_page.to_string()), p("NextToken", &token)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the second page: expected HTTP 200, got {status} with body {second}"
    );
    assert_eq!(
        elements(&second, "volumeId"),
        binned[min_page as usize..].to_vec(),
        "the token continues from the end of the first page"
    );
    assert!(
        element(&second, "nextToken").is_none(),
        "no volume follows the last page, so it carries no token, got {second}"
    );

    // The page size itself: `(case, the value, the status, the code)` at each published
    // bound and one past it, plus a token this service did not mint.
    let page_size_rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "the published minimum",
            vec![p("MaxResults", min_page.to_string())],
            200,
            "",
        ),
        (
            "the published maximum",
            vec![p("MaxResults", max_page.to_string())],
            200,
            "",
        ),
        (
            "one below the minimum",
            vec![p("MaxResults", (min_page - 1).to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            "one above the maximum",
            vec![p("MaxResults", (max_page + 1).to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            "a token this service did not mint",
            vec![p("NextToken", "vol-0123456789abcdef0")],
            400,
            "InvalidPaginationToken",
        ),
    ];
    for (case, params, expected_status, expected_code) in page_size_rows {
        let (status, body) = op(&router, "ListVolumesInRecycleBin", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: volume I/O and the detach path
// ---------------------------------------------------------------------------

/// `EnableVolumeIO` has something to do only for a volume whose I/O is disabled: a volume
/// whose I/O is enabled — which every stored volume's is, nothing here finding a volume's
/// data inconsistent — is refused with `IncorrectState`, and one whose I/O is disabled is
/// enabled and stays enabled.
#[tokio::test]
async fn enable_volume_io_is_refused_unless_the_volumes_io_is_disabled() {
    let (router, engine) = emulator();
    let volume_id = create_volume(&router).await;

    let (status, body) = op(&router, "EnableVolumeIO", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        status, 400,
        "a volume whose I/O is enabled: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "a volume whose I/O is enabled: expected IncorrectState, got {body}"
    );

    // A volume whose data Amazon EBS found potentially inconsistent, which is the one case
    // the operation documents.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard
            .volumes
            .get_mut(&volume_id)
            .expect("the volume just created")
            .io_enabled = false;
    }
    let (status, body) = op(&router, "EnableVolumeIO", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        status, 200,
        "a volume whose I/O is disabled: expected HTTP 200, got {status} with body {body}"
    );
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        assert!(
            guard
                .volumes
                .get(&volume_id)
                .expect("the volume is still stored")
                .io_enabled,
            "the enable turns the volume's I/O back on"
        );
    }
    // And now that it is on again, a second enable has nothing to do.
    let (status, body) = op(&router, "EnableVolumeIO", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IncorrectState"),
        "a second enable: expected IncorrectState, got {status} with body {body}"
    );

    // The id itself is resolved malformed before not found.
    let id_rows: [(&str, &str, &str); 3] = [
        ("a value that is not a volume id", "not-a-volume", "InvalidVolumeID.Malformed"),
        (
            "another resource's prefix",
            UNKNOWN_IMAGE_ID,
            "InvalidVolumeID.Malformed",
        ),
        (
            "a well-formed id nothing was created for",
            UNKNOWN_VOLUME_ID,
            "InvalidVolume.NotFound",
        ),
    ];
    for (case, id, expected_code) in id_rows {
        let (status, body) = op(&router, "EnableVolumeIO", &[p("VolumeId", id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

/// Give the volume `volume_id` an attachment to `instance_id` at `device`, the way
/// `AttachVolume` will.
fn attach_in_store(
    engine: &engine::InMemoryEngine,
    volume_id: &str,
    instance_id: &str,
    device: &str,
) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let record = guard
        .volumes
        .get_mut(volume_id)
        .expect("the volume just created");
    let attachment = AttachVolumeOutput {
        volume_id: Some(volume_id.to_string()),
        instance_id: Some(instance_id.to_string()),
        device: Some(device.to_string()),
        state: Some(VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
        attach_time: Some(system_time_to_date_time(SystemTime::now())),
        delete_on_termination: Some(false),
        associated_resource: None,
        instance_owning_service: None,
        ebs_card_index: Some(0),
    };
    record
        .view
        .attachments
        .get_or_insert_with(Vec::new)
        .push(attachment);
}

/// A detach is refused unless the volume HOLDS the attachment the request names: a volume
/// attached to nothing, to another instance, or at another device is `IncorrectState`, and
/// the id itself is resolved malformed before not found.
#[tokio::test]
async fn detach_volume_is_refused_unless_the_volume_holds_that_attachment() {
    let (router, engine) = emulator();
    let unattached = create_volume(&router).await;
    let attached = create_volume(&router).await;
    attach_in_store(&engine, &attached, UNKNOWN_INSTANCE_ID, "/dev/sdf");

    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a volume attached to nothing",
            vec![p("VolumeId", &unattached)],
            400,
            "IncorrectState",
        ),
        (
            "a volume attached to another instance",
            vec![
                p("VolumeId", &attached),
                p("InstanceId", "i-0fedcba987654321f"),
            ],
            400,
            "IncorrectState",
        ),
        (
            "a volume attached at another device",
            vec![p("VolumeId", &attached), p("Device", "/dev/sdz")],
            400,
            "IncorrectState",
        ),
        (
            "a value that is not a volume id at all",
            vec![p("VolumeId", "not-a-volume")],
            400,
            "InvalidVolumeID.Malformed",
        ),
        (
            "a well-formed volume id nothing was created for",
            vec![p("VolumeId", UNKNOWN_VOLUME_ID)],
            400,
            "InvalidVolume.NotFound",
        ),
        (
            "a value that is not an instance id at all",
            vec![p("VolumeId", &attached), p("InstanceId", "not-an-instance")],
            400,
            "InvalidInstanceID.Malformed",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = op(&router, "DetachVolume", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // None of those refusals detached anything: the attachment is still there.
    let (_, described) = op(&router, "DescribeVolumes", &indexed("VolumeId", &[&attached])).await;
    assert!(
        elements(&described, "status").contains(&VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
        "a refused detach leaves the attachment as it was, got {described}"
    );
}

/// An attachment is the pair of an instance AND a device, not the instance alone: detaching
/// a Multi-Attach volume at one device of an instance leaves the attachment that instance
/// holds at another device, and naming no device at all with two attachments is refused
/// rather than guessed at.
#[tokio::test]
async fn one_device_of_an_instance_is_detached_without_touching_another() {
    let (router, engine) = emulator();
    let volume_id = create_volume(&router).await;
    past_the_transition().await;
    attach_in_store(&engine, &volume_id, UNKNOWN_INSTANCE_ID, "/dev/sdf");
    attach_in_store(&engine, &volume_id, UNKNOWN_INSTANCE_ID, "/dev/sdg");

    // Two attachments answer a request that names neither an instance nor a device, so
    // there is no single one to take down and the request is refused rather than one of
    // them being guessed at.
    let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        status, 400,
        "a detach naming neither instance nor device: expected HTTP 400, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombination",
        "a detach naming neither instance nor device: expected InvalidParameterCombination, \
         got {body}"
    );

    let (status, body) = op(
        &router,
        "DetachVolume",
        &[
            p("VolumeId", &volume_id),
            p("InstanceId", UNKNOWN_INSTANCE_ID),
            p("Device", "/dev/sdf"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "detaching one device: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "device").as_deref(),
        Some("/dev/sdf"),
        "the detach reports the device it was asked about, got {body}"
    );

    past_the_transition().await;

    let (_, described) = op(&router, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
    assert_eq!(
        elements(&described, "device"),
        vec!["/dev/sdg".to_string()],
        "the attachment at the other device is untouched, got {described}"
    );
    assert!(
        elements(&described, "status").contains(&VOLUME_STATE_IN_USE.to_string()),
        "a volume that still holds an attachment is {VOLUME_STATE_IN_USE}, got {described}"
    );

    // One attachment is left, so a detach naming no device has exactly one to take down.
    let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        status, 200,
        "detaching the last attachment: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "device").as_deref(),
        Some("/dev/sdg"),
        "the last attachment is the one detached, got {body}"
    );
}

/// A detach that is admitted moves the attachment to `detaching` and the clock finishes it:
/// the volume is `in-use` with a `detaching` attachment straight after the call, and
/// `available` with no attachment once the move falls due — which is what `DescribeVolumes`
/// reports.
#[tokio::test]
async fn detaching_a_volume_frees_it_on_the_clock() {
    let (router, engine) = emulator();
    let volume_id = create_volume(&router).await;
    past_the_transition().await;
    attach_in_store(&engine, &volume_id, UNKNOWN_INSTANCE_ID, "/dev/sdf");

    // While it is attached, the volume is in use.
    let (_, described) = op(&router, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
    assert!(
        elements(&described, "status").contains(&VOLUME_STATE_IN_USE.to_string()),
        "an attached volume is {VOLUME_STATE_IN_USE}, got {described}"
    );

    let (status, body) = op(
        &router,
        "DetachVolume",
        &[p("VolumeId", &volume_id), p("Device", "/dev/sdf")],
    )
    .await;
    assert_eq!(
        status, 200,
        "detaching the volume: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "status").as_deref(),
        Some(VOLUME_ATTACHMENT_STATE_DETACHING),
        "the detach reports the attachment as {VOLUME_ATTACHMENT_STATE_DETACHING}, got {body}"
    );
    assert_eq!(
        element(&body, "volumeId").as_deref(),
        Some(volume_id.as_str()),
        "the detach reports the volume it was asked about, got {body}"
    );
    assert_eq!(
        element(&body, "instanceId").as_deref(),
        Some(UNKNOWN_INSTANCE_ID),
        "the detach reports the instance the volume was attached to, got {body}"
    );

    let (_, described) = op(&router, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
    let states = elements(&described, "status");
    assert!(
        states.contains(&VOLUME_ATTACHMENT_STATE_DETACHING.to_string())
            && states.contains(&VOLUME_STATE_IN_USE.to_string()),
        "straight after the detach the volume still holds a {VOLUME_ATTACHMENT_STATE_DETACHING} \
         attachment, got {described}"
    );

    past_the_transition().await;

    let (_, described) = op(&router, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
    assert_eq!(
        elements(&described, "status"),
        vec![VOLUME_STATE_AVAILABLE.to_string()],
        "once the detach falls due the volume holds no attachment and is \
         {VOLUME_STATE_AVAILABLE}, got {described}"
    );

    // The attachment is gone, so there is nothing left to detach.
    let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IncorrectState"),
        "detaching twice: expected IncorrectState, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// Instance export tasks
// ---------------------------------------------------------------------------

/// Store one instance export task in `state`, and answer its `ExportTaskId`.
fn seed_export_task(engine: &engine::InMemoryEngine, state: &str) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let export_task_id = guard.next_id("export-i");
    guard.export_tasks.insert(
        export_task_id.clone(),
        ExportTaskRecord {
            view: ExportTask {
                export_task_id: Some(export_task_id.clone()),
                description: Some("an instance export".to_string()),
                state: Some(state.to_string()),
                status_message: None,
                export_to_s3_task: Some(ExportToS3Task {
                    s3_bucket: Some("my-vm-exports".to_string()),
                    s3_key: Some("vms/instance.ova".to_string()),
                    container_format: Some("ova".to_string()),
                    disk_image_format: Some("VMDK".to_string()),
                }),
                instance_export_details: Some(InstanceExportDetails {
                    instance_id: Some(UNKNOWN_INSTANCE_ID.to_string()),
                    target_environment: Some("vmware".to_string()),
                }),
                tags: Some(Vec::new()),
            },
            cancelled_at: None,
        },
    );
    export_task_id
}

/// An export task id the account holds no task for is refused with
/// `InvalidExportTaskID.NotFound`, whatever shape the id has — the model publishes no pattern
/// for one, so there is no second answer to give.
#[tokio::test]
async fn cancel_export_task_refuses_an_id_the_account_holds_no_task_for() {
    let (router, _engine) = emulator();

    let id_rows: [(&str, &str); 5] = [
        ("an instance export task id", "export-i-0123456789abcdef0"),
        ("an image export task id", "export-ami-0123456789abcdef0"),
        ("the short id form", "export-i-01234567"),
        ("a value of no particular shape", "not-an-export-task"),
        ("another resource's id", UNKNOWN_VOLUME_ID),
    ];
    for (case, export_task_id) in id_rows {
        let (status, body) = op(
            &router,
            "CancelExportTask",
            &[p("ExportTaskId", export_task_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidExportTaskID.NotFound",
            "case {case:?}: expected InvalidExportTaskID.NotFound, got {body}"
        );
    }
}

/// Only an ACTIVE export task can be cancelled: the cancel puts it into `cancelling` and the
/// clock finishes it, and a task in any other state is refused with `IncorrectState`.
#[tokio::test]
async fn cancelling_an_active_export_task_finishes_on_the_clock() {
    let (router, engine) = emulator();
    let active = seed_export_task(&engine, EXPORT_TASK_STATE_ACTIVE);

    let (status, body) = op(&router, "CancelExportTask", &[p("ExportTaskId", &active)]).await;
    assert_eq!(
        status, 200,
        "cancelling an active task: expected HTTP 200, got {status} with body {body}"
    );
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let record = guard
            .export_tasks
            .get(&active)
            .expect("the task is still stored");
        assert_eq!(
            record.state_at(Instant::now()),
            EXPORT_TASK_STATE_CANCELLING,
            "straight after the cancel the task is {EXPORT_TASK_STATE_CANCELLING}"
        );
    }

    past_the_transition().await;

    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let record = guard
            .export_tasks
            .get(&active)
            .expect("the task is still stored");
        assert_eq!(
            record.state_at(Instant::now()),
            EXPORT_TASK_STATE_CANCELLED,
            "once the cancellation falls due the task is {EXPORT_TASK_STATE_CANCELLED}"
        );
        assert_eq!(
            record.view_at(Instant::now()).state.as_deref(),
            Some(EXPORT_TASK_STATE_CANCELLED),
            "the task's reported state is the one the clock has reached"
        );
    }

    // `(case, the task's state)`: every state that is not active refuses the cancel.
    let state_rows: Vec<(&str, String)> = vec![
        (
            "a task that is already cancelled",
            active.clone(),
        ),
        (
            "a task that has completed",
            seed_export_task(&engine, EXPORT_TASK_STATE_COMPLETED),
        ),
        (
            "a task that is already cancelling",
            {
                let task = seed_export_task(&engine, EXPORT_TASK_STATE_ACTIVE);
                let store = engine.state::<Ec2State>();
                let mut guard = store.lock();
                guard
                    .export_tasks
                    .get_mut(&task)
                    .expect("the task was just seeded")
                    .cancelled_at = Some(Instant::now() + Duration::from_secs(60));
                task
            },
        ),
    ];
    for (case, export_task_id) in state_rows {
        let (status, body) = op(
            &router,
            "CancelExportTask",
            &[p("ExportTaskId", &export_task_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got {body}"
        );
    }
}

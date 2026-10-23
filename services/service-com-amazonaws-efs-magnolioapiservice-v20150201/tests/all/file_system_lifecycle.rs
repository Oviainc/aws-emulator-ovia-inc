// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EFS file-system lifecycle slice: CreateFileSystem,
//! DescribeFileSystems, CreateMountTarget, DescribeMountTargets, DeleteMountTarget and
//! DeleteFileSystem, driven over HTTP through the same `MultiServiceRouter` the
//! production entrypoint serves.
//!
//! Every rule is asserted over a SET of inputs rather than on one recorded call: the
//! tables below vary the member the rule generalises over (mode, value, count, id,
//! Region, page size), and the single-row cases are the published boundaries.
//!
//! The harness every suite of this test target shares — `Emulator`, `Reply`, `settle`,
//! `matches_id_pattern` — lives in `tests/all/harness.rs` and is re-exported below, so
//! each suite drives the same one service over the same router rather than standing up a
//! second copy of it.

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

// The HTTP harness every suite in this crate shares lives in `tests/all/harness.rs`;
// it is re-exported here so a suite that reaches it as
// `crate::file_system_lifecycle::{settle, Emulator, Reply}` keeps resolving to the one
// harness rather than to a copy of it.
pub(crate) use super::harness::{matches_id_pattern, settle, Emulator, Reply};

// ---------------------------------------------------------------------------
// The whole lifecycle, in order, with every read observing the preceding write.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_system_and_mount_target_lifecycle_round_trips_through_state() {
    let efs = Emulator::new();
    let subnet = "subnet-0123456789abcdef0";

    // CreateFileSystem -> 201 with a minted, stored id.
    let created = efs
        .create_file_system(json!({
            "CreationToken": "boot-1",
            "Tags": [{ "Key": "Name", "Value": "demo" }],
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateFileSystem: expected 201, got {} {:?}",
        created.status, created.body
    );
    let file_system_id = created.string("FileSystemId");
    assert!(
        matches_id_pattern(&file_system_id, "fs-"),
        "CreateFileSystem: expected a minted fs- id, got {file_system_id:?}"
    );
    assert_eq!(
        created.string("FileSystemArn"),
        format!("arn:aws:elasticfilesystem:us-east-1:000000000000:file-system/{file_system_id}"),
        "CreateFileSystem: unexpected FileSystemArn, got {:?}",
        created.get("FileSystemArn")
    );
    assert_eq!(
        created.string("LifeCycleState"),
        "creating",
        "CreateFileSystem: a new file system is reported while still creating, got {:?}",
        created.get("LifeCycleState")
    );
    assert_eq!(
        created.get("NumberOfMountTargets"),
        &json!(0),
        "CreateFileSystem: expected NumberOfMountTargets 0, got {:?}",
        created.get("NumberOfMountTargets")
    );
    assert_eq!(
        created.string("Name"),
        "demo",
        "CreateFileSystem: Name comes from the Name tag, got {:?}",
        created.get("Name")
    );
    assert_eq!(
        created.get("SizeInBytes")["Value"],
        json!(0),
        "CreateFileSystem: expected an empty file system, got {:?}",
        created.get("SizeInBytes")
    );
    assert_eq!(
        created.get("SizeInBytes")["Timestamp"],
        Value::Null,
        "CreateFileSystem: SizeInBytes.Timestamp is the time the size \"was determined\", \
         so it is absent until a metering happens, got {:?}",
        created.get("SizeInBytes")
    );

    // The same CreationToken is idempotent: no second file system, and the error
    // names the first one.
    let repeat = efs
        .create_file_system(json!({ "CreationToken": "boot-1" }))
        .await;
    assert_eq!(
        repeat.status, 409,
        "repeat CreateFileSystem: expected 409, got {} {:?}",
        repeat.status, repeat.body
    );
    assert_eq!(
        repeat.error_type.as_deref(),
        Some("FileSystemAlreadyExists"),
        "repeat CreateFileSystem: unexpected error type, got {:?}",
        repeat.error_type
    );
    assert_eq!(
        repeat.string("FileSystemId"),
        file_system_id,
        "repeat CreateFileSystem: the error must name the existing id, got {:?}",
        repeat.get("FileSystemId")
    );

    // DescribeFileSystems reports exactly the one stored file system.
    let listed = efs.describe_file_systems("").await;
    assert_eq!(
        listed.status, 200,
        "DescribeFileSystems: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    let file_systems = listed.get("FileSystems").as_array().cloned().unwrap_or_default();
    assert_eq!(
        file_systems.len(),
        1,
        "DescribeFileSystems: expected exactly the one created file system, got {file_systems:?}"
    );
    assert_eq!(
        file_systems[0]["FileSystemId"].as_str().unwrap_or_default(),
        file_system_id,
        "DescribeFileSystems: unexpected id, got {:?}",
        file_systems[0]
    );

    // An unknown id is a 404, not an empty list.
    let unknown = efs
        .describe_file_systems("FileSystemId=fs-deadbeef")
        .await;
    assert_eq!(
        unknown.status, 404,
        "DescribeFileSystems(fs-deadbeef): expected 404, got {} {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_type.as_deref(),
        Some("FileSystemNotFound"),
        "DescribeFileSystems(fs-deadbeef): unexpected error type, got {:?}",
        unknown.error_type
    );

    // While the file system is still `creating`, a mount target cannot be added.
    let too_early = efs
        .create_mount_target(json!({
            "FileSystemId": file_system_id,
            "SubnetId": subnet,
        }))
        .await;
    assert_eq!(
        too_early.status, 409,
        "CreateMountTarget on a creating file system: expected 409, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_type.as_deref(),
        Some("IncorrectFileSystemLifeCycleState"),
        "CreateMountTarget on a creating file system: unexpected error type, got {:?}",
        too_early.error_type
    );

    // The clock owes the transition; after it, the read reports `available`.
    settle().await;
    let settled = efs
        .describe_file_systems(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        settled.get("FileSystems")[0]["LifeCycleState"],
        json!("available"),
        "DescribeFileSystems after the creating span: expected available, got {:?}",
        settled.get("FileSystems")[0]
    );

    // CreateMountTarget -> 200 with a minted id and an allocated address.
    let mount = efs
        .create_mount_target(json!({
            "FileSystemId": file_system_id,
            "SubnetId": subnet,
        }))
        .await;
    assert_eq!(
        mount.status, 200,
        "CreateMountTarget: expected 200, got {} {:?}",
        mount.status, mount.body
    );
    let mount_target_id = mount.string("MountTargetId");
    assert!(
        matches_id_pattern(&mount_target_id, "fsmt-"),
        "CreateMountTarget: expected a minted fsmt- id, got {mount_target_id:?}"
    );
    assert_eq!(
        mount.string("IpAddress"),
        "10.0.0.4",
        "CreateMountTarget: the first address of a fresh subnet is its first assignable \
         host, got {:?}",
        mount.get("IpAddress")
    );
    assert_eq!(
        mount.string("AvailabilityZoneName"),
        "us-east-1a",
        "CreateMountTarget: unexpected AvailabilityZoneName, got {:?}",
        mount.get("AvailabilityZoneName")
    );
    assert_eq!(
        mount.string("AvailabilityZoneId"),
        "use1-az1",
        "CreateMountTarget: unexpected AvailabilityZoneId, got {:?}",
        mount.get("AvailabilityZoneId")
    );
    assert!(
        mount.string("NetworkInterfaceId").starts_with("eni-"),
        "CreateMountTarget: expected a minted network interface, got {:?}",
        mount.get("NetworkInterfaceId")
    );
    assert!(
        mount.string("VpcId").starts_with("vpc-"),
        "CreateMountTarget: expected the subnet's VPC, got {:?}",
        mount.get("VpcId")
    );

    // DescribeMountTargets returns the STORED description, identical to the create
    // answer while the clock has not yet moved the lifecycle on.
    let described = efs
        .describe_mount_targets(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        described.status, 200,
        "DescribeMountTargets: expected 200, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.get("MountTargets"),
        &json!([mount.body.clone()]),
        "DescribeMountTargets: expected exactly the stored create description, got {:?}",
        described.get("MountTargets")
    );

    // And the owning file system's count moved with it.
    let counted = efs
        .describe_file_systems(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        counted.get("FileSystems")[0]["NumberOfMountTargets"],
        json!(1),
        "DescribeFileSystems after CreateMountTarget: expected 1, got {:?}",
        counted.get("FileSystems")[0]
    );

    // A second mount target in the same subnet is one per Availability Zone too many.
    let conflict = efs
        .create_mount_target(json!({
            "FileSystemId": file_system_id,
            "SubnetId": subnet,
        }))
        .await;
    assert_eq!(
        conflict.status, 409,
        "second CreateMountTarget in the same subnet: expected 409, got {} {:?}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.error_type.as_deref(),
        Some("MountTargetConflict"),
        "second CreateMountTarget in the same subnet: unexpected error type, got {:?}",
        conflict.error_type
    );

    // The file system is in use while it has mount targets.
    let in_use = efs.delete_file_system(&file_system_id).await;
    assert_eq!(
        in_use.status, 409,
        "DeleteFileSystem with a mount target: expected 409, got {} {:?}",
        in_use.status, in_use.body
    );
    assert_eq!(
        in_use.error_type.as_deref(),
        Some("FileSystemInUse"),
        "DeleteFileSystem with a mount target: unexpected error type, got {:?}",
        in_use.error_type
    );

    // DeleteMountTarget -> 204 with an empty body; the count goes back down.
    let deleted = efs.delete_mount_target(&mount_target_id).await;
    assert_eq!(
        deleted.status, 204,
        "DeleteMountTarget: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.body_len, 0,
        "DeleteMountTarget: expected an empty body, got {} byte(s)",
        deleted.body_len
    );
    let emptied = efs
        .describe_mount_targets(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        emptied.get("MountTargets"),
        &json!([]),
        "DescribeMountTargets after the delete: expected no mount targets, got {:?}",
        emptied.get("MountTargets")
    );
    let recounted = efs
        .describe_file_systems(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        recounted.get("FileSystems")[0]["NumberOfMountTargets"],
        json!(0),
        "DescribeFileSystems after DeleteMountTarget: expected 0, got {:?}",
        recounted.get("FileSystems")[0]
    );
    let repeat_delete = efs.delete_mount_target(&mount_target_id).await;
    assert_eq!(
        repeat_delete.status, 404,
        "repeat DeleteMountTarget: expected 404, got {} {:?}",
        repeat_delete.status, repeat_delete.body
    );
    assert_eq!(
        repeat_delete.error_type.as_deref(),
        Some("MountTargetNotFound"),
        "repeat DeleteMountTarget: unexpected error type, got {:?}",
        repeat_delete.error_type
    );

    // DeleteFileSystem -> 204 empty; the record and its describe are gone.
    let dropped = efs.delete_file_system(&file_system_id).await;
    assert_eq!(
        dropped.status, 204,
        "DeleteFileSystem: expected 204, got {} {:?}",
        dropped.status, dropped.body
    );
    assert_eq!(
        dropped.body_len, 0,
        "DeleteFileSystem: expected an empty body, got {} byte(s)",
        dropped.body_len
    );
    let gone = efs
        .describe_file_systems(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        gone.status, 404,
        "DescribeFileSystems after DeleteFileSystem: expected 404, got {} {:?}",
        gone.status, gone.body
    );
    let repeat_drop = efs.delete_file_system(&file_system_id).await;
    assert_eq!(
        repeat_drop.status, 404,
        "repeat DeleteFileSystem: expected 404, got {} {:?}",
        repeat_drop.status, repeat_drop.body
    );

    // Deleting the file system freed its CreationToken for reuse.
    let reused = efs
        .create_file_system(json!({ "CreationToken": "boot-1" }))
        .await;
    assert_eq!(
        reused.status, 201,
        "CreateFileSystem reusing a freed token: expected 201, got {} {:?}",
        reused.status, reused.body
    );
    assert_ne!(
        reused.string("FileSystemId"),
        file_system_id,
        "CreateFileSystem reusing a freed token: expected a freshly minted id, got the old one"
    );
}

// ---------------------------------------------------------------------------
// CreateFileSystem stores what it was given, and defaults what it was not.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_file_system_stores_every_configuration_and_reads_it_back() {
    // Rows differ along the members the description is built from: the two
    // PerformanceMode values, the three ThroughputMode values, encryption with each
    // documented KmsKeyId form, tag count, and One Zone placement.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "all defaults",
            json!({ "CreationToken": "row-1" }),
            json!({
                "PerformanceMode": "generalPurpose",
                "ThroughputMode": "bursting",
            }),
        ),
        (
            "maxIO with bursting throughput",
            json!({ "CreationToken": "row-2", "PerformanceMode": "maxIO" }),
            json!({ "PerformanceMode": "maxIO", "ThroughputMode": "bursting" }),
        ),
        (
            "elastic throughput",
            json!({ "CreationToken": "row-3", "ThroughputMode": "elastic" }),
            json!({ "PerformanceMode": "generalPurpose", "ThroughputMode": "elastic" }),
        ),
        (
            "provisioned throughput",
            json!({
                "CreationToken": "row-4",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 128.0,
            }),
            json!({
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 128.0,
            }),
        ),
        (
            "encrypted with the default EFS key",
            json!({ "CreationToken": "row-5", "Encrypted": true }),
            // The key the EFS default-key ALIAS resolves to is asserted by
            // `mutation_results.rs`, which covers every `KmsKeyId` form at once.
            json!({ "Encrypted": true }),
        ),
        (
            "encrypted with a bare key id",
            json!({
                "CreationToken": "row-6",
                "Encrypted": true,
                "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
            json!({
                "Encrypted": true,
                "KmsKeyId":
                    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
        ),
        (
            "encrypted with a key alias",
            json!({
                "CreationToken": "row-7",
                "Encrypted": true,
                "KmsKeyId": "alias/projectKey1",
            }),
            json!({ "Encrypted": true }),
        ),
        (
            "encrypted with a key ARN",
            json!({
                "CreationToken": "row-8",
                "Encrypted": true,
                "KmsKeyId":
                    "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
            json!({
                "KmsKeyId":
                    "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
        ),
        (
            "several tags, one of them Name",
            json!({
                "CreationToken": "row-9",
                "Tags": [
                    { "Key": "team", "Value": "platform" },
                    { "Key": "Name", "Value": "shared-home" },
                    { "Key": "env", "Value": "test" },
                ],
            }),
            json!({ "Name": "shared-home" }),
        ),
        (
            "a One Zone file system",
            json!({ "CreationToken": "row-10", "AvailabilityZoneName": "us-east-1c" }),
            json!({ "AvailabilityZoneName": "us-east-1c", "AvailabilityZoneId": "use1-az3" }),
        ),
    ];

    let efs = Emulator::new();
    for (case, request, expected) in cases {
        let created = efs.create_file_system(request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        for (member, value) in expected.as_object().expect("an object of expectations") {
            assert_eq!(
                created.get(member),
                value,
                "case {case:?}: expected {member} {value}, got {:?}",
                created.get(member)
            );
        }
        assert_eq!(
            created.get("CreationToken"),
            &request["CreationToken"],
            "case {case:?}: CreationToken must be the one specified, got {:?}",
            created.get("CreationToken")
        );
        assert_eq!(
            created.get("Tags"),
            request.get("Tags").unwrap_or(&json!([])),
            "case {case:?}: Tags must be the ones stored, got {:?}",
            created.get("Tags")
        );
        assert_eq!(
            created.string("OwnerId"),
            "000000000000",
            "case {case:?}: unexpected OwnerId, got {:?}",
            created.get("OwnerId")
        );

        // Every member the create answered with comes back from the store, unchanged
        // but for the lifecycle the clock moves.
        let id = created.string("FileSystemId");
        let read_back = efs
            .describe_file_systems(&format!("FileSystemId={id}"))
            .await;
        assert_eq!(
            read_back.get("FileSystems"),
            &json!([created.body.clone()]),
            "case {case:?}: DescribeFileSystems must return the stored description, got {:?}",
            read_back.get("FileSystems")
        );

        // And the CreationToken filter selects the same one record.
        let token = request["CreationToken"].as_str().unwrap_or_default();
        let by_token = efs
            .describe_file_systems(&format!("CreationToken={token}"))
            .await;
        assert_eq!(
            by_token.get("FileSystems"),
            &json!([created.body.clone()]),
            "case {case:?}: the CreationToken filter must select it, got {:?}",
            by_token.get("FileSystems")
        );
    }

    // A CreationToken no file system holds is the same 404 an unknown id is: the
    // selector names one file system, so a selector that matches nothing has not found
    // it — the observable DeleteFileSystem documents for a released token.
    let none = efs.describe_file_systems("CreationToken=never-used").await;
    assert_eq!(
        none.status, 404,
        "an unused CreationToken filter: expected 404, got {} {:?}",
        none.status, none.body
    );
    assert_eq!(
        none.error_type.as_deref(),
        Some("FileSystemNotFound"),
        "an unused CreationToken filter: expected FileSystemNotFound, got {:?}",
        none.error_type
    );
}

// ---------------------------------------------------------------------------
// CreateFileSystem refusals.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_file_system_refuses_each_unsatisfiable_configuration() {
    // Each row is one documented rule, named by the error AWS models for it.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "provisioned throughput with no amount",
            json!({ "CreationToken": "bad-1", "ThroughputMode": "provisioned" }),
            400,
            "BadRequest",
        ),
        (
            "an amount without provisioned throughput",
            json!({ "CreationToken": "bad-2", "ProvisionedThroughputInMibps": 64.0 }),
            400,
            "BadRequest",
        ),
        (
            "an amount one past the published upper bound",
            json!({
                "CreationToken": "bad-3",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 3415.0,
            }),
            400,
            "ThroughputLimitExceeded",
        ),
        (
            "maxIO with elastic throughput",
            json!({
                "CreationToken": "bad-4",
                "PerformanceMode": "maxIO",
                "ThroughputMode": "elastic",
            }),
            400,
            "BadRequest",
        ),
        (
            "maxIO on a One Zone file system",
            json!({
                "CreationToken": "bad-5",
                "PerformanceMode": "maxIO",
                "AvailabilityZoneName": "us-east-1a",
            }),
            400,
            "BadRequest",
        ),
        (
            "a KMS key on an unencrypted file system",
            json!({
                "CreationToken": "bad-6",
                "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
            400,
            "BadRequest",
        ),
        (
            "two tags with the same key",
            json!({
                "CreationToken": "bad-7",
                "Tags": [
                    { "Key": "env", "Value": "a" },
                    { "Key": "env", "Value": "b" },
                ],
            }),
            400,
            "BadRequest",
        ),
        (
            "an Availability Zone of another Region",
            json!({ "CreationToken": "bad-8", "AvailabilityZoneName": "eu-west-1a" }),
            400,
            "UnsupportedAvailabilityZone",
        ),
        (
            "a PerformanceMode outside the modelled enum",
            json!({ "CreationToken": "bad-9", "PerformanceMode": "turbo" }),
            400,
            "ValidationException",
        ),
        (
            "a ThroughputMode outside the modelled enum",
            json!({ "CreationToken": "bad-10", "ThroughputMode": "unlimited" }),
            400,
            "ValidationException",
        ),
    ];

    let efs = Emulator::new();
    for (case, request, status, error_type) in cases {
        let refused = efs.create_file_system(request).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(error_type),
            "case {case:?}: expected error type {error_type}, got {:?}",
            refused.error_type
        );
    }

    // A refused create stores nothing.
    let listed = efs.describe_file_systems("").await;
    assert_eq!(
        listed.get("FileSystems"),
        &json!([]),
        "a refused CreateFileSystem must store nothing, got {:?}",
        listed.get("FileSystems")
    );
}

#[tokio::test]
async fn provisioned_throughput_accepts_each_published_bound() {
    // "Valid values are 1-3414 MiBps" — both ends accepted, the step past the top
    // refused. The lower end is the model's own `range {min: 1}`.
    let efs = Emulator::new();
    for (index, mibps) in [1.0_f64, 1024.0, 3414.0].iter().enumerate() {
        let created = efs
            .create_file_system(json!({
                "CreationToken": format!("bound-{index}"),
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": mibps,
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {mibps} MiBps: expected 201, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.get("ProvisionedThroughputInMibps"),
            &json!(mibps),
            "case {mibps} MiBps: expected it stored, got {:?}",
            created.get("ProvisionedThroughputInMibps")
        );
    }
    let over = efs
        .create_file_system(json!({
            "CreationToken": "bound-over",
            "ThroughputMode": "provisioned",
            "ProvisionedThroughputInMibps": 3415.0,
        }))
        .await;
    assert_eq!(
        over.status, 400,
        "case 3415 MiBps: expected 400, got {} {:?}",
        over.status, over.body
    );
    let under = efs
        .create_file_system(json!({
            "CreationToken": "bound-under",
            "ThroughputMode": "provisioned",
            "ProvisionedThroughputInMibps": 0.0,
        }))
        .await;
    assert_eq!(
        under.status, 400,
        "case 0 MiBps: expected 400, got {} {:?}",
        under.status, under.body
    );
}

// ---------------------------------------------------------------------------
// Availability Zone naming, over Regions the issue did not cite.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_zone_file_systems_report_the_published_availability_zone_id() {
    // Published AZ-ID prefixes: us-east-1 -> use1, ap-southeast-1 -> apse1,
    // eu-central-1 -> euc1, us-gov-west-1 -> usgw1 ("Availability Zone IDs for your
    // resources", AWS RAM User Guide). The zone letter picks the -azN suffix.
    let cases = [
        ("us-east-1", "us-east-1a", "use1-az1"),
        ("ap-southeast-1", "ap-southeast-1b", "apse1-az2"),
        ("eu-central-1", "eu-central-1c", "euc1-az3"),
        ("us-gov-west-1", "us-gov-west-1a", "usgw1-az1"),
        ("ap-northeast-2", "ap-northeast-2d", "apne2-az4"),
    ];

    for (region, az_name, expected) in cases {
        let efs = Emulator::in_region(region);
        let created = efs
            .create_file_system(json!({
                "CreationToken": "one-zone",
                "AvailabilityZoneName": az_name,
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {az_name:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.string("AvailabilityZoneId"),
            expected,
            "case {az_name:?}: expected AvailabilityZoneId {expected}, got {:?}",
            created.get("AvailabilityZoneId")
        );
        assert_eq!(
            created.string("FileSystemArn"),
            format!(
                "arn:aws:elasticfilesystem:{region}:000000000000:file-system/{}",
                created.string("FileSystemId")
            ),
            "case {az_name:?}: the ARN carries the signed Region, got {:?}",
            created.get("FileSystemArn")
        );
    }
}

// ---------------------------------------------------------------------------
// Mount target placement and addressing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refused_create_mount_target_records_no_subnet_facts() {
    // A subnet id is caller-chosen, so the store that remembers a subnet's zone and
    // address range must only ever be written by a call that succeeded. Zone letters are
    // handed out in the order subnets are first RECORDED, which makes the rule
    // observable: had a refused call recorded its subnet, the next subnet to be recorded
    // would be pushed off the Region's first zone.
    let efs = Emulator::new();
    let one_zone = efs
        .create_file_system(json!({
            "CreationToken": "one-zone-refusal",
            "AvailabilityZoneName": "us-east-1b",
        }))
        .await;
    assert_eq!(
        one_zone.status, 201,
        "creating the One Zone file system: expected 201, got {} {:?}",
        one_zone.status, one_zone.body
    );
    let one_zone_id = one_zone.string("FileSystemId");
    let plain_id = efs.available_file_system("after-refusal").await;

    // Every subnet id here is fresh, and each call is refused AFTER the subnet facts are
    // resolved, so none of them may leave a record behind.
    for subnet in [
        "subnet-0aaaaaaaaaaaaaaa1",
        "subnet-0aaaaaaaaaaaaaaa2",
        "subnet-0aaaaaaaaaaaaaaa3",
    ] {
        let refused = efs
            .create_mount_target(json!({
                "FileSystemId": one_zone_id,
                "SubnetId": subnet,
            }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {subnet:?}: a One Zone file system refuses another zone, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("AvailabilityZonesMismatch"),
            "case {subnet:?}: expected AvailabilityZonesMismatch, got {:?}",
            refused.error_type
        );
    }

    // The first subnet this emulator actually records still takes the Region's FIRST
    // zone, which it could not if any refusal above had recorded its own.
    let accepted = efs
        .create_mount_target(json!({
            "FileSystemId": plain_id,
            "SubnetId": "subnet-0bbbbbbbbbbbbbbb0",
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "the first accepted mount target: expected 200, got {} {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.string("AvailabilityZoneName"),
        format!("us-east-1{}", state::AZ_SUFFIXES[0]),
        "a refused call records no subnet, so the first recorded one takes zone a, got {:?}",
        accepted.get("AvailabilityZoneName")
    );
    assert_eq!(
        accepted.string("AvailabilityZoneId"),
        "use1-az1",
        "a refused call records no subnet, so the first recorded one takes az1, got {:?}",
        accepted.get("AvailabilityZoneId")
    );

    // The VPC a mount target reports is a pure read of the store, so neither the
    // refusals above nor the ids minted for the accepted call can have shifted it: a
    // fresh emulator whose first mount target is accepted straight away reports the same
    // VPC.
    let clean = Emulator::new();
    let clean_file_system = clean.available_file_system("no-refusal").await;
    let clean_mount = clean
        .create_mount_target(json!({
            "FileSystemId": clean_file_system,
            "SubnetId": "subnet-0bbbbbbbbbbbbbbb0",
        }))
        .await;
    assert_eq!(
        clean_mount.status, 200,
        "the unrefused mount target: expected 200, got {} {:?}",
        clean_mount.status, clean_mount.body
    );
    assert_eq!(
        accepted.string("VpcId"),
        clean_mount.string("VpcId"),
        "reading the emulated VPC must write nothing, so refused calls before the first \
         mount target cannot change it; got {:?} after refusals and {:?} without",
        accepted.get("VpcId"),
        clean_mount.get("VpcId")
    );
}

#[tokio::test]
async fn mount_targets_take_one_availability_zone_each_and_distinct_addresses() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("many-subnets").await;

    // One subnet per zone letter: the Region's zones, in the order the subnets are
    // first referenced. Each row differs in the subnet id, which is what the
    // placement rule generalises over.
    let subnets = [
        "subnet-00000000000000001",
        "subnet-00000000000000002",
        "subnet-00000000000000003",
        "subnet-00000000000000004",
        "subnet-00000000000000005",
        "subnet-00000000000000006",
    ];
    let mut zones = Vec::new();
    let mut addresses = Vec::new();
    let mut vpcs = Vec::new();
    for (index, subnet) in subnets.iter().enumerate() {
        let mount = efs
            .create_mount_target(json!({
                "FileSystemId": file_system_id,
                "SubnetId": subnet,
            }))
            .await;
        assert_eq!(
            mount.status, 200,
            "case {subnet:?}: expected 200, got {} {:?}",
            mount.status, mount.body
        );
        assert_eq!(
            mount.string("SubnetId"),
            *subnet,
            "case {subnet:?}: the description names its own subnet, got {:?}",
            mount.get("SubnetId")
        );
        assert_eq!(
            mount.string("IpAddress"),
            format!("10.0.{index}.4"),
            "case {subnet:?}: each subnet allocates from its own range, got {:?}",
            mount.get("IpAddress")
        );
        zones.push(mount.string("AvailabilityZoneName"));
        addresses.push(mount.string("IpAddress"));
        vpcs.push(mount.string("VpcId"));
    }

    let mut distinct_zones = zones.clone();
    distinct_zones.sort();
    distinct_zones.dedup();
    assert_eq!(
        distinct_zones.len(),
        subnets.len(),
        "each subnet must sit in its own Availability Zone, got {zones:?}"
    );
    let mut distinct_addresses = addresses.clone();
    distinct_addresses.sort();
    distinct_addresses.dedup();
    assert_eq!(
        distinct_addresses.len(),
        subnets.len(),
        "each mount target must hold its own address, got {addresses:?}"
    );
    let mut distinct_vpcs = vpcs.clone();
    distinct_vpcs.sort();
    distinct_vpcs.dedup();
    assert_eq!(
        distinct_vpcs.len(),
        1,
        "every emulated subnet belongs to the one VPC, got {vpcs:?}"
    );

    // All six are stored against the file system, and the count follows them.
    let described = efs
        .describe_mount_targets(&format!("FileSystemId={file_system_id}"))
        .await;
    let stored = described
        .get("MountTargets")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        stored.len(),
        subnets.len(),
        "DescribeMountTargets: expected {} stored mount targets, got {stored:?}",
        subnets.len()
    );
    let counted = efs
        .describe_file_systems(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        counted.get("FileSystems")[0]["NumberOfMountTargets"],
        json!(6),
        "DescribeFileSystems: expected NumberOfMountTargets 6, got {:?}",
        counted.get("FileSystems")[0]
    );

    // A seventh subnet wraps onto a zone already taken, which is the documented
    // one-per-zone conflict.
    let wrapped = efs
        .create_mount_target(json!({
            "FileSystemId": file_system_id,
            "SubnetId": "subnet-00000000000000007",
        }))
        .await;
    assert_eq!(
        wrapped.status, 409,
        "a seventh zone: expected 409, got {} {:?}",
        wrapped.status, wrapped.body
    );
    assert_eq!(
        wrapped.error_type.as_deref(),
        Some("MountTargetConflict"),
        "a seventh zone: unexpected error type, got {:?}",
        wrapped.error_type
    );

    // Each mount target is reachable on its own id, and only that one.
    for entry in &stored {
        let id = entry["MountTargetId"].as_str().unwrap_or_default();
        let one = efs
            .describe_mount_targets(&format!("MountTargetId={id}"))
            .await;
        assert_eq!(
            one.get("MountTargets"),
            &json!([entry.clone()]),
            "case {id:?}: expected exactly its own stored description, got {:?}",
            one.get("MountTargets")
        );
    }
}

#[tokio::test]
async fn mount_target_addressing_honours_the_requested_address_and_family() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("addressing").await;

    // A supplied IPv4 address is honoured as it stands.
    let supplied = efs
        .create_mount_target(json!({
            "FileSystemId": file_system_id,
            "SubnetId": "subnet-0aaaaaaaaaaaaaaa1",
            "IpAddress": "10.0.0.37",
        }))
        .await;
    assert_eq!(
        supplied.status, 200,
        "a supplied IpAddress: expected 200, got {} {:?}",
        supplied.status, supplied.body
    );
    assert_eq!(
        supplied.string("IpAddress"),
        "10.0.0.37",
        "a supplied IpAddress must be honoured, got {:?}",
        supplied.get("IpAddress")
    );
    assert_eq!(
        supplied.get("Ipv6Address"),
        &Value::Null,
        "IPv4-only addressing reports no IPv6 address, got {:?}",
        supplied.get("Ipv6Address")
    );

    // Another file system in the same subnet cannot take the same address.
    let other = efs.available_file_system("addressing-other").await;
    let taken = efs
        .create_mount_target(json!({
            "FileSystemId": other,
            "SubnetId": "subnet-0aaaaaaaaaaaaaaa1",
            "IpAddress": "10.0.0.37",
        }))
        .await;
    assert_eq!(
        taken.status, 409,
        "a reused IpAddress: expected 409, got {} {:?}",
        taken.status, taken.body
    );
    assert_eq!(
        taken.error_type.as_deref(),
        Some("IpAddressInUse"),
        "a reused IpAddress: unexpected error type, got {:?}",
        taken.error_type
    );

    // The address family follows IpAddressType.
    let cases: [(&str, &str, bool, bool); 3] = [
        ("IPV4_ONLY", "subnet-0bbbbbbbbbbbbbbb1", true, false),
        ("DUAL_STACK", "subnet-0bbbbbbbbbbbbbbb2", true, true),
        ("IPV6_ONLY", "subnet-0bbbbbbbbbbbbbbb3", false, true),
    ];
    for (ip_address_type, subnet, ipv4, ipv6) in cases {
        let family = efs
            .available_file_system(&format!("family-{ip_address_type}"))
            .await;
        let mount = efs
            .create_mount_target(json!({
                "FileSystemId": family,
                "SubnetId": subnet,
                "IpAddressType": ip_address_type,
            }))
            .await;
        assert_eq!(
            mount.status, 200,
            "case {ip_address_type:?}: expected 200, got {} {:?}",
            mount.status, mount.body
        );
        assert_eq!(
            mount.get("IpAddress").is_string(),
            ipv4,
            "case {ip_address_type:?}: IpAddress presence, got {:?}",
            mount.get("IpAddress")
        );
        assert_eq!(
            mount.get("Ipv6Address").is_string(),
            ipv6,
            "case {ip_address_type:?}: Ipv6Address presence, got {:?}",
            mount.get("Ipv6Address")
        );
    }

    // An address of the family the type excludes is refused.
    let mismatched = efs
        .create_mount_target(json!({
            "FileSystemId": file_system_id,
            "SubnetId": "subnet-0ccccccccccccccc1",
            "IpAddressType": "IPV6_ONLY",
            "IpAddress": "10.0.9.9",
        }))
        .await;
    assert_eq!(
        mismatched.status, 400,
        "an IPv4 address on an IPv6-only mount target: expected 400, got {} {:?}",
        mismatched.status, mismatched.body
    );
    assert_eq!(
        mismatched.error_type.as_deref(),
        Some("BadRequest"),
        "an IPv4 address on an IPv6-only mount target: unexpected error type, got {:?}",
        mismatched.error_type
    );
}

#[tokio::test]
async fn create_mount_target_refuses_each_unsatisfiable_request() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("refusals").await;

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an unknown file system",
            json!({
                "FileSystemId": "fs-deadbeef",
                "SubnetId": "subnet-0dddddddddddddddd",
            }),
            404,
            "FileSystemNotFound",
        ),
        (
            "an IpAddressType outside the modelled enum",
            json!({
                "FileSystemId": file_system_id,
                "SubnetId": "subnet-0dddddddddddddddd",
                "IpAddressType": "IPV8",
            }),
            400,
            "ValidationException",
        ),
        (
            "a security group that is not a VPC security group id",
            json!({
                "FileSystemId": file_system_id,
                "SubnetId": "subnet-0dddddddddddddddd",
                "SecurityGroups": ["not-a-group"],
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, status, error_type) in cases {
        let refused = efs.create_mount_target(request).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(error_type),
            "case {case:?}: expected error type {error_type}, got {:?}",
            refused.error_type
        );
    }

    // None of them stored a mount target.
    let described = efs
        .describe_mount_targets(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        described.get("MountTargets"),
        &json!([]),
        "a refused CreateMountTarget must store nothing, got {:?}",
        described.get("MountTargets")
    );

    // A well-formed security group id is accepted and stored against the mount target.
    let accepted = efs
        .create_mount_target(json!({
            "FileSystemId": file_system_id,
            "SubnetId": "subnet-0dddddddddddddddd",
            "SecurityGroups": ["sg-0123456789abcdef0", "sg-abcdef01"],
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "well-formed security groups: expected 200, got {} {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn describe_mount_targets_requires_a_selector_and_reports_unknown_ids() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("selectors").await;

    let cases: Vec<(&str, String, u16, &str)> = vec![
        ("no selector at all", String::new(), 400, "BadRequest"),
        (
            "an unknown file system",
            "FileSystemId=fs-deadbeef".to_string(),
            404,
            "FileSystemNotFound",
        ),
        (
            "an unknown mount target",
            "MountTargetId=fsmt-deadbeef".to_string(),
            404,
            "MountTargetNotFound",
        ),
        (
            "an access point, which this emulator creates none of yet",
            "AccessPointId=fsap-0123456789abcdef0".to_string(),
            404,
            "AccessPointNotFound",
        ),
    ];

    for (case, query, status, error_type) in cases {
        let refused = efs.describe_mount_targets(&query).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(error_type),
            "case {case:?}: expected error type {error_type}, got {:?}",
            refused.error_type
        );
    }

    // A known file system with no mount targets is an empty list, not an error.
    let empty = efs
        .describe_mount_targets(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        empty.status, 200,
        "a file system with no mount targets: expected 200, got {} {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.get("MountTargets"),
        &json!([]),
        "a file system with no mount targets: expected an empty list, got {:?}",
        empty.get("MountTargets")
    );
}

// ---------------------------------------------------------------------------
// Pagination.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_file_systems_pages_consistently_from_its_next_marker() {
    let efs = Emulator::new();
    let mut created = Vec::new();
    for index in 0..7 {
        let reply = efs
            .create_file_system(json!({ "CreationToken": format!("page-{index}") }))
            .await;
        assert_eq!(
            reply.status, 201,
            "creating page-{index}: expected 201, got {} {:?}",
            reply.status, reply.body
        );
        created.push(reply.string("FileSystemId"));
    }

    // Every page size, including ones that divide the set and ones that do not, walks
    // the whole set exactly once and in one order.
    for page_size in [1_usize, 2, 3, 7, 10] {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &marker {
                Some(value) => format!("MaxItems={page_size}&Marker={value}"),
                None => format!("MaxItems={page_size}"),
            };
            let page = efs.describe_file_systems(&query).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: expected 200, got {} {:?}",
                page.status, page.body
            );
            assert_eq!(
                page.get("Marker"),
                &marker
                    .as_ref()
                    .map_or(Value::Null, |value| Value::String(value.clone())),
                "case page size {page_size}: Marker must echo the request, got {:?}",
                page.get("Marker")
            );
            let items = page
                .get("FileSystems")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page must not exceed MaxItems, got {}",
                items.len()
            );
            for item in &items {
                walked.push(item["FileSystemId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case page size {page_size}: pagination did not terminate after {pages} pages"
            );
            match page.get("NextMarker").as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case page size {page_size}: paging must walk every file system once, in creation \
             order; got {walked:?}"
        );
    }

    // A marker that names nothing in the result set is a refusal, not a silent empty
    // page.
    let bogus = efs.describe_file_systems("Marker=fs-deadbeef").await;
    assert_eq!(
        bogus.status, 400,
        "an unknown Marker: expected 400, got {} {:?}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error_type.as_deref(),
        Some("BadRequest"),
        "an unknown Marker: unexpected error type, got {:?}",
        bogus.error_type
    );
}

// ---------------------------------------------------------------------------
// Published bounds on what one request may ask the store for.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tags_are_accepted_up_to_the_published_quota_and_refused_past_it() {
    // Published quota: "Tags per file system: 50" ("Amazon EFS quotas that you cannot
    // change", Amazon EFS User Guide). The modelled `Tags` list carries no length bound,
    // so the quota is the handler's own.
    let efs = Emulator::new();
    for count in [0_usize, 1, 50] {
        let tags: Vec<Value> = (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
            .collect();
        let created = efs
            .create_file_system(json!({
                "CreationToken": format!("tags-{count}"),
                "Tags": tags,
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {count} tags: expected 201, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.get("Tags").as_array().map(Vec::len),
            Some(count),
            "case {count} tags: expected them all stored, got {:?}",
            created.get("Tags")
        );
    }

    let over: Vec<Value> = (0..51)
        .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
        .collect();
    let refused = efs
        .create_file_system(json!({ "CreationToken": "tags-51", "Tags": over }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case 51 tags: expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("BadRequest"),
        "case 51 tags: unexpected error type, got {:?}",
        refused.error_type
    );
}

#[tokio::test]
async fn describe_file_systems_clamps_a_page_to_the_documented_maximum() {
    // "This number is automatically set to 100. The response is paginated at 100 per page
    // if you have more than 100 file systems." So an absent MaxItems pages at 100 and a
    // larger one is clamped to 100 rather than returning the whole store.
    let efs = Emulator::new();
    for index in 0..105 {
        let reply = efs
            .create_file_system(json!({ "CreationToken": format!("clamp-{index}") }))
            .await;
        assert_eq!(
            reply.status, 201,
            "creating clamp-{index}: expected 201, got {} {:?}",
            reply.status, reply.body
        );
    }

    for case in ["", "MaxItems=100", "MaxItems=1000", "MaxItems=2147483647"] {
        let page = efs.describe_file_systems(case).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            page.status, page.body
        );
        let items = page
            .get("FileSystems")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            items.len(),
            100,
            "case {case:?}: expected a page of 100, got {}",
            items.len()
        );
        assert!(
            page.get("NextMarker").is_string(),
            "case {case:?}: a truncated page must hand back a NextMarker, got {:?}",
            page.get("NextMarker")
        );
    }

    // The remainder arrives on the next page, and that page is the last.
    let first = efs.describe_file_systems("").await;
    let marker = first.string("NextMarker");
    let second = efs
        .describe_file_systems(&format!("Marker={marker}"))
        .await;
    assert_eq!(
        second.get("FileSystems").as_array().map(Vec::len),
        Some(5),
        "the second page must hold the remaining 5, got {:?}",
        second.get("FileSystems")
    );
    assert_eq!(
        second.get("NextMarker"),
        &Value::Null,
        "the last page must hand back no NextMarker, got {:?}",
        second.get("NextMarker")
    );
}

#[tokio::test]
async fn the_emulated_vpc_records_subnets_only_up_to_the_published_quota() {
    // Published VPC quota: "Subnets per VPC: 200" ("Amazon VPC quotas", Amazon VPC User
    // Guide). Past it a further subnet id names no subnet of this VPC, which is the
    // modelled SubnetNotFound — and it is what stops a caller naming a fresh subnet on
    // every request from growing the registry without limit.
    //
    // Only a SUCCESSFUL CreateMountTarget records a subnet, so the subnets are grouped
    // one-per-zone onto a file system of their own and every call below must be accepted.
    let efs = Emulator::new();
    let zones = state::AZ_SUFFIXES.len();
    let groups = state::MAX_SUBNETS_PER_VPC.div_ceil(zones);
    let mut file_systems = Vec::with_capacity(groups);
    for group in 0..groups {
        let created = efs
            .create_file_system(json!({ "CreationToken": format!("subnet-quota-{group}") }))
            .await;
        assert_eq!(
            created.status, 201,
            "case group {group}: expected 201, got {} {:?}",
            created.status, created.body
        );
        file_systems.push(created.string("FileSystemId"));
    }
    settle().await;

    for index in 0..state::MAX_SUBNETS_PER_VPC {
        let reply = efs
            .create_mount_target(json!({
                "FileSystemId": file_systems[index / zones],
                "SubnetId": format!("subnet-{index:017x}"),
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case subnet {index}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
    }

    // The file system of the last, partly filled group still has free zones, so what
    // refuses the next call is the subnet quota and nothing else.
    let last = file_systems.last().expect("at least one group").clone();
    let past_quota = efs
        .create_mount_target(json!({
            "FileSystemId": last,
            "SubnetId": "subnet-0ffffffffffffffff",
        }))
        .await;
    assert_eq!(
        past_quota.status, 400,
        "case one subnet past the quota: expected 400, got {} {:?}",
        past_quota.status, past_quota.body
    );
    assert_eq!(
        past_quota.error_type.as_deref(),
        Some("SubnetNotFound"),
        "case one subnet past the quota: unexpected error type, got {:?}",
        past_quota.error_type
    );

    // A subnet already recorded still resolves, so the cap refuses only new ones. Subnet
    // 2 was recorded third, which put it in the Region's third zone — one the last
    // group's file system has not taken.
    let recorded = efs
        .create_mount_target(json!({
            "FileSystemId": last,
            "SubnetId": format!("subnet-{:017x}", 2),
        }))
        .await;
    assert_eq!(
        recorded.status, 200,
        "case an already recorded subnet: expected 200, got {} {:?}",
        recorded.status, recorded.body
    );
    assert_eq!(
        recorded.string("AvailabilityZoneName"),
        format!("us-east-1{}", state::AZ_SUFFIXES[2]),
        "case an already recorded subnet: it keeps the zone it was first recorded with, got {:?}",
        recorded.get("AvailabilityZoneName")
    );
}

#[tokio::test]
async fn describe_mount_targets_pages_consistently_from_its_next_marker() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("mt-pages").await;

    let subnets = [
        "subnet-0eeeeeeeeeeeeeee1",
        "subnet-0eeeeeeeeeeeeeee2",
        "subnet-0eeeeeeeeeeeeeee3",
        "subnet-0eeeeeeeeeeeeeee4",
        "subnet-0eeeeeeeeeeeeeee5",
    ];
    let mut created = Vec::new();
    for subnet in subnets {
        let reply = efs
            .create_mount_target(json!({
                "FileSystemId": file_system_id,
                "SubnetId": subnet,
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "creating a mount target in {subnet}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        created.push(reply.string("MountTargetId"));
    }

    for page_size in [1_usize, 2, 5] {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let query = match &marker {
                Some(value) => format!(
                    "FileSystemId={file_system_id}&MaxItems={page_size}&Marker={value}"
                ),
                None => format!("FileSystemId={file_system_id}&MaxItems={page_size}"),
            };
            let page = efs.describe_mount_targets(&query).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: expected 200, got {} {:?}",
                page.status, page.body
            );
            let items = page
                .get("MountTargets")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page must not exceed MaxItems, got {}",
                items.len()
            );
            for item in &items {
                walked.push(
                    item["MountTargetId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            match page.get("NextMarker").as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case page size {page_size}: paging must walk every mount target once, in creation \
             order; got {walked:?}"
        );
    }
}

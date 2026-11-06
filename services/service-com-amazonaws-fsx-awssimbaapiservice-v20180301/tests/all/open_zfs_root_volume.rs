// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the root volume an OpenZFS file system HAS, driven over the wire.
//!
//! RULE: an FSx for OpenZFS file system is created WITH a volume — AWS documents
//! `OpenZFSFileSystemConfiguration$RootVolumeId` as "the ID of the root volume of the
//! OpenZFS file system" and takes `RootVolumeConfiguration` on the create itself — so the
//! create mints that volume as a resource of its own, the id the file system reports
//! resolves to it through `DescribeVolumes`, and the volume carries the configuration the
//! create stated with the documented defaults materialised for what it did not. A file
//! system of a type that has no volumes of its own reports no root volume, and a restore
//! gets its own root volume rather than the one its backup was taken of.
//!
//! Each case asserts that rule over a SET of inputs — every file-system type, a span of
//! root-volume configurations, the volume read back by id and by filter — rather than over
//! one recorded call. The requests go through the same front door as
//! `file_system_lifecycle.rs` (its harness is reused), so nothing here reaches into the
//! store directly.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, emulator, failure, ok, tag_pairs, with_client_request_token, ACCOUNT, PAST_TRANSITION,
    REGION,
};

/// The modelled `VolumeId` pattern `^(fsvol-[0-9a-f]{17,})$`, with `length 23..23`.
fn is_well_formed_volume_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("fsvol-") else {
        return false;
    };
    id.len() == 23
        && hex.len() >= 17
        && hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
}

/// Describe exactly one volume, requiring it to exist.
async fn describe_one_volume(router: &MultiServiceRouter, volume_id: &str) -> Value {
    let body = ok(router, "DescribeVolumes", &json!({ "VolumeIds": [volume_id] })).await;
    let volumes = body["Volumes"]
        .as_array()
        .expect("Volumes must be a list")
        .clone();
    assert_eq!(
        volumes.len(),
        1,
        "describing {volume_id} must return exactly one volume, got {body}"
    );
    volumes[0].clone()
}

/// Create one file system and return the stored `FileSystem` of the create response.
async fn create_file_system(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "CreateFileSystem", &with_client_request_token(request)).await["FileSystem"].clone()
}

/// RULE: every OpenZFS file system reports a `RootVolumeId`, and that id names a volume
/// `DescribeVolumes` returns — of this file system, with the stated root-volume
/// configuration and the documented defaults for what was not stated.
#[tokio::test]
async fn an_open_zfs_file_system_has_a_root_volume_describe_volumes_reads_back() {
    let router = emulator().await;
    // (the create's RootVolumeConfiguration, the volume's expected RecordSizeKiB,
    // DataCompressionType and CopyTagsToSnapshots). The documented defaults come from
    // `OpenZFSCreateRootVolumeConfiguration`: RecordSizeKiB "The default is 128 KiB",
    // DataCompressionType "is NONE by default", CopyTagsToSnapshots "defaults to false".
    let cases: &[(Option<Value>, i64, &str, bool)] = &[
        (None, 128, "NONE", false),
        (Some(json!({})), 128, "NONE", false),
        (
            Some(json!({ "DataCompressionType": "ZSTD", "RecordSizeKiB": 128 })),
            128,
            "ZSTD",
            false,
        ),
        (
            Some(json!({ "DataCompressionType": "LZ4", "RecordSizeKiB": 4 })),
            4,
            "LZ4",
            false,
        ),
        (
            Some(json!({
                "RecordSizeKiB": 1024,
                "CopyTagsToSnapshots": true,
                "ReadOnly": false,
            })),
            1024,
            "NONE",
            true,
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (root_volume_configuration, record_size, compression, copy_tags) in cases {
        let case = format!("{root_volume_configuration:?}");
        let subnet = router.subnet_a().to_string();
        let mut open_zfs = json!({
            "DeploymentType": "SINGLE_AZ_1",
            "ThroughputCapacity": 64,
        });
        if let Some(configuration) = root_volume_configuration {
            open_zfs["RootVolumeConfiguration"] = configuration.clone();
        }
        let file_system = create_file_system(
            &router,
            &json!({
                "FileSystemType": "OPENZFS",
                "StorageCapacity": 64,
                "StorageType": "SSD",
                "SubnetIds": [subnet],
                "OpenZFSConfiguration": open_zfs,
            }),
        )
        .await;
        let file_system_id = file_system["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let root_volume_id = file_system["OpenZFSConfiguration"]["RootVolumeId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            is_well_formed_volume_id(&root_volume_id),
            "case {case:?}: expected a RootVolumeId matching ^fsvol-[0-9a-f]{{17,}}$ of length \
             23, got {root_volume_id:?} in {file_system}"
        );
        assert!(
            !minted.contains(&root_volume_id),
            "case {case:?}: expected a fresh RootVolumeId, got the already-minted \
             {root_volume_id:?}"
        );
        minted.push(root_volume_id.clone());

        let volume = describe_one_volume(&router, &root_volume_id).await;
        assert_eq!(
            volume["FileSystemId"],
            json!(file_system_id),
            "case {case:?}: expected the volume of {file_system_id}, got {volume}"
        );
        assert_eq!(
            volume["VolumeType"], "OPENZFS",
            "case {case:?}: expected VolumeType OPENZFS, got {:?}",
            volume["VolumeType"]
        );
        assert_eq!(
            volume["ResourceARN"],
            json!(format!(
                "arn:aws:fsx:{REGION}:{ACCOUNT}:volume/{root_volume_id}"
            )),
            "case {case:?}: expected the volume's ARN for the signed region, got {:?}",
            volume["ResourceARN"]
        );
        assert!(
            volume["CreationTime"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected a CreationTime, got {:?}",
            volume["CreationTime"]
        );
        assert_eq!(
            volume["OpenZFSConfiguration"]["RecordSizeKiB"],
            json!(record_size),
            "case {case:?}: expected RecordSizeKiB {record_size}, got {:?}",
            volume["OpenZFSConfiguration"]
        );
        assert_eq!(
            volume["OpenZFSConfiguration"]["DataCompressionType"],
            json!(compression),
            "case {case:?}: expected DataCompressionType {compression}, got {:?}",
            volume["OpenZFSConfiguration"]
        );
        assert_eq!(
            volume["OpenZFSConfiguration"]["CopyTagsToSnapshots"],
            json!(copy_tags),
            "case {case:?}: expected CopyTagsToSnapshots {copy_tags}, got {:?}",
            volume["OpenZFSConfiguration"]
        );
        assert_eq!(
            volume["OpenZFSConfiguration"].get("ParentVolumeId"),
            None,
            "case {case:?}: expected the root volume to have no parent, got {:?}",
            volume["OpenZFSConfiguration"]
        );

        // The clock owes the new volume the same transition the file system owes.
        assert_eq!(
            volume["Lifecycle"], "CREATING",
            "case {case:?}: expected the documented initial Lifecycle CREATING, got {:?}",
            volume["Lifecycle"]
        );
        tokio::time::sleep(PAST_TRANSITION).await;
        let settled = describe_one_volume(&router, &root_volume_id).await;
        assert_eq!(
            settled["Lifecycle"], "AVAILABLE",
            "case {case:?}: expected the volume to reach AVAILABLE, got {:?}",
            settled["Lifecycle"]
        );

        // Selected by the modelled `file-system-id` filter, the volume of this file system
        // is the one returned.
        let filtered = ok(
            &router,
            "DescribeVolumes",
            &json!({
                "Filters": [{ "Name": "file-system-id", "Values": [file_system_id] }],
            }),
        )
        .await;
        let ids: Vec<String> = filtered["Volumes"]
            .as_array()
            .expect("Volumes must be a list")
            .iter()
            .map(|volume| volume["VolumeId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            ids,
            vec![root_volume_id.clone()],
            "case {case:?}: expected the file-system-id filter to select its root volume, got \
             {filtered}"
        );
    }
}

/// RULE: a file-system type with no volumes of its own reports no root volume, and nothing
/// is stored for it.
#[tokio::test]
async fn a_file_system_with_no_volumes_reports_no_root_volume() {
    let router = emulator().await;
    // (FileSystemType, the per-type configuration member, its configuration) over the
    // modelled `FileSystemType` enum (WINDOWS | LUSTRE | ONTAP | OPENZFS) minus OPENZFS.
    let cases: &[(&str, &str, Value)] = &[
        (
            "WINDOWS",
            "WindowsConfiguration",
            json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 32 }),
        ),
        (
            "LUSTRE",
            "LustreConfiguration",
            json!({ "DeploymentType": "PERSISTENT_1", "PerUnitStorageThroughput": 50 }),
        ),
        (
            "ONTAP",
            "OntapConfiguration",
            json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 128 }),
        ),
    ];

    for (file_system_type, member, configuration) in cases {
        let subnet = router.subnet_a().to_string();
        let mut request = json!({
            "FileSystemType": file_system_type,
            "StorageCapacity": 1200,
            "StorageType": "SSD",
            "SubnetIds": [subnet],
        });
        request[*member] = configuration.clone();
        let file_system = create_file_system(&router, &request).await;
        let file_system_id = file_system["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let listed = ok(
            &router,
            "DescribeVolumes",
            &json!({
                "Filters": [{ "Name": "file-system-id", "Values": [file_system_id] }],
            }),
        )
        .await;
        assert_eq!(
            listed["Volumes"],
            json!([]),
            "case {file_system_type:?}: expected no volumes for a {file_system_type} file \
             system, got {listed}"
        );
    }
}

/// RULE: the root volume is a volume of ITS file system, so a restore provisions a new one
/// — configured as the volume the backup recorded was — rather than reporting the id of the
/// volume the backup was taken of.
#[tokio::test]
async fn a_restored_file_system_gets_its_own_root_volume() {
    let router = emulator().await;
    let subnet = router.subnet_a().to_string();
    let source = create_file_system(
        &router,
        &json!({
            "FileSystemType": "OPENZFS",
            "StorageCapacity": 64,
            "StorageType": "SSD",
            "SubnetIds": [subnet],
            "OpenZFSConfiguration": {
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 64,
                "RootVolumeConfiguration": {
                    "DataCompressionType": "ZSTD",
                    "RecordSizeKiB": 256,
                },
            },
        }),
    )
    .await;
    let source_id = source["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let source_root_volume_id = source["OpenZFSConfiguration"]["RootVolumeId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    tokio::time::sleep(PAST_TRANSITION).await;

    let backup_id = ok(
        &router,
        "CreateBackup",
        &with_client_request_token(&json!({ "FileSystemId": source_id })),
    )
    .await["Backup"]["BackupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    tokio::time::sleep(PAST_TRANSITION).await;

    let restored = ok(
        &router,
        "CreateFileSystemFromBackup",
        &with_client_request_token(&json!({
            "BackupId": backup_id,
            "SubnetIds": [subnet],
        })),
    )
    .await["FileSystem"]
        .clone();
    let restored_root_volume_id = restored["OpenZFSConfiguration"]["RootVolumeId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    assert!(
        is_well_formed_volume_id(&restored_root_volume_id),
        "case \"restore\": expected a well-formed RootVolumeId, got \
         {restored_root_volume_id:?} in {restored}"
    );
    assert_ne!(
        restored_root_volume_id, source_root_volume_id,
        "case \"restore\": expected a root volume of its own, got the source's \
         {source_root_volume_id}"
    );

    let restored_volume = describe_one_volume(&router, &restored_root_volume_id).await;
    let source_volume = describe_one_volume(&router, &source_root_volume_id).await;
    assert_eq!(
        restored_volume["FileSystemId"], restored["FileSystemId"],
        "case \"restore\": expected the new volume to belong to the restored file system, got \
         {restored_volume}"
    );
    assert_eq!(
        restored_volume["OpenZFSConfiguration"], source_volume["OpenZFSConfiguration"],
        "case \"restore\": expected the recorded root-volume configuration, got \
         {restored_volume}"
    );

    // A restore that states its own root-volume configuration gets that one instead.
    let restated = ok(
        &router,
        "CreateFileSystemFromBackup",
        &with_client_request_token(&json!({
            "BackupId": backup_id,
            "SubnetIds": [subnet],
            "OpenZFSConfiguration": {
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 64,
                "RootVolumeConfiguration": { "DataCompressionType": "LZ4" },
            },
        })),
    )
    .await["FileSystem"]
        .clone();
    let restated_volume = describe_one_volume(
        &router,
        restated["OpenZFSConfiguration"]["RootVolumeId"]
            .as_str()
            .unwrap_or_default(),
    )
    .await;
    assert_eq!(
        restated_volume["OpenZFSConfiguration"]["DataCompressionType"], "LZ4",
        "case \"restore with its own root volume\": expected the stated DataCompressionType, \
         got {restated_volume}"
    );
}

/// One row of the tag-copying table: what the create states for `CopyTagsToVolumes`, and
/// the tag pairs the root volume must then carry.
struct CopyTagsCase {
    copies: Option<bool>,
    expected_pairs: &'static [(&'static str, &'static str)],
}

/// RULE: a volume belongs to its file system — deleting the file system deletes its volumes
/// — and the tags of the volume are the file system's exactly when `CopyTagsToVolumes` says
/// they are copied.
#[tokio::test]
async fn a_root_volume_belongs_to_its_file_system() {
    let router = emulator().await;
    let cases = &[
        CopyTagsCase {
            copies: None,
            expected_pairs: &[],
        },
        CopyTagsCase {
            copies: Some(false),
            expected_pairs: &[],
        },
        CopyTagsCase {
            copies: Some(true),
            expected_pairs: &[("Name", "zfs"), ("Team", "storage")],
        },
    ];

    for CopyTagsCase {
        copies,
        expected_pairs,
    } in cases
    {
        let expected_tags: Vec<(String, String)> = expected_pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let case = format!("CopyTagsToVolumes {copies:?}");
        let subnet = router.subnet_a().to_string();
        let mut open_zfs = json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 64 });
        if let Some(copies) = copies {
            open_zfs["CopyTagsToVolumes"] = json!(copies);
        }
        let file_system = create_file_system(
            &router,
            &json!({
                "FileSystemType": "OPENZFS",
                "StorageCapacity": 64,
                "StorageType": "SSD",
                "SubnetIds": [subnet],
                "OpenZFSConfiguration": open_zfs,
                "Tags": [
                    { "Key": "Name", "Value": "zfs" },
                    { "Key": "Team", "Value": "storage" },
                ],
            }),
        )
        .await;
        let file_system_id = file_system["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let root_volume_id = file_system["OpenZFSConfiguration"]["RootVolumeId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let volume = describe_one_volume(&router, &root_volume_id).await;
        assert_eq!(
            tag_pairs(volume.get("Tags")),
            expected_tags,
            "case {case:?}: expected the volume tags {expected_tags:?}, got {:?}",
            volume["Tags"]
        );

        // The volume's own ARN resolves to the volume, so its tags are readable through the
        // tagging operations too.
        let listed = ok(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceARN": volume["ResourceARN"] }),
        )
        .await;
        assert_eq!(
            tag_pairs(listed.get("Tags")),
            expected_tags,
            "case {case:?}: expected ListTagsForResource on the volume to report \
             {expected_tags:?}, got {listed}"
        );

        // Deleting the file system takes its volumes with it.
        tokio::time::sleep(PAST_TRANSITION).await;
        ok(
            &router,
            "DeleteFileSystem",
            &json!({ "FileSystemId": file_system_id }),
        )
        .await;
        tokio::time::sleep(PAST_TRANSITION).await;
        let (status, code) = failure(
            &router,
            "DescribeVolumes",
            &json!({ "VolumeIds": [root_volume_id] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "VolumeNotFound"),
            "case {case:?}: expected the volume to be gone with its file system, got {status} \
             {code}"
        );
    }
}

/// RULE: `DescribeVolumes` refuses what it cannot answer: a `VolumeId` the account has no
/// volume for, a filter name the listing does not select by, and a `NextToken` it did not
/// mint.
#[tokio::test]
async fn describe_volumes_refuses_what_it_cannot_answer() {
    let router = emulator().await;
    // Absent ids spanning the modelled `VolumeId` shape: `fsvol-` plus exactly 17 hex
    // digits (`length 23..23`).
    for volume_id in [
        "fsvol-00000000000000000",
        "fsvol-0123456789abcdef0",
        "fsvol-fffffffffffffffff",
    ] {
        let (status, parsed) = call(
            &router,
            "DescribeVolumes",
            &json!({ "VolumeIds": [volume_id] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {volume_id:?}: expected HTTP 400 for an absent volume, got {status} with body \
             {parsed}"
        );
        assert_eq!(
            parsed["Message"],
            json!(format!("Volume '{volume_id}' does not exist.")),
            "case {volume_id:?}: expected the refusal to name the requested volume, got {parsed}"
        );
    }

    for filter_name in ["backup-type", "file-system-type", "volume-id", ""] {
        let (status, code) = failure(
            &router,
            "DescribeVolumes",
            &json!({ "Filters": [{ "Name": filter_name, "Values": ["x"] }] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BadRequest"),
            "case {filter_name:?}: expected HTTP 400 BadRequest for a filter this listing does \
             not select by, got {status} {code}"
        );
    }

    // A base64-shaped token this service never minted, satisfying the modelled `NextToken`
    // pattern.
    let (status, code) = failure(
        &router,
        "DescribeVolumes",
        &json!({ "NextToken": "AAAABBBB" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "case \"foreign NextToken\": expected HTTP 400 BadRequest, got {status} {code}"
    );
}

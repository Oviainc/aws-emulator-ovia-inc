// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FSx backup slice, driven over the wire.
//!
//! RULE: `CreateBackup` persists a backup of the file system the request names — a minted
//! `BackupId`, the documented initial `CREATING` lifecycle that the clock settles to
//! `AVAILABLE`, the `USER_INITIATED` type, and a copy of that file system's metadata, which
//! AWS documents is "persisted even if the file system is deleted" — and answers with the
//! stored record, so a following `DescribeBackups` reports the very same values. A request
//! naming a resource the account does not have, a file system whose configuration does not
//! support backups, or one whose backup is still being created is refused with the modelled
//! error for that case.
//!
//! Each case below asserts that rule over a SET of inputs — every file-system type a backup
//! can be taken of, every placement of the documented `CopyTagsToBackups` member, every
//! Lustre deployment type, a span of absent identifiers — rather than over one recorded
//! call. The requests go through the same front door as `file_system_lifecycle.rs` (its
//! harness is reused), so nothing here reaches into the store directly.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, emulator, failure, ok, tag_pairs, with_client_request_token, ACCOUNT, PAST_TRANSITION,
    REGION,
};

/// Absent `FileSystemId`s spanning the modelled `FileSystemId` shape: pattern
/// `^(fs-[0-9a-f]{8,})$` with `length 11..21`, so the shortest legal id, a mid-length one
/// and the longest legal id.
const ABSENT_FILE_SYSTEM_IDS: &[&str] = &[
    "fs-deadbeef",
    "fs-00000000deadbeef",
    "fs-ffffffffffffffffff",
];

/// `VolumeId`s spanning the modelled `VolumeId` shape: pattern
/// `^(fsvol-[0-9a-f]{17,})$` with `length 23..23`, so `fsvol-` plus exactly 17 hex digits.
const VOLUME_IDS: &[&str] = &[
    "fsvol-00000000000000000",
    "fsvol-0123456789abcdef0",
    "fsvol-fffffffffffffffff",
];

/// The modelled `BackupId` pattern `^(backup-[0-9a-f]{8,})$`, with `length 12..128`.
fn is_well_formed_backup_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("backup-") else {
        return false;
    };
    (12..=128).contains(&id.len())
        && hex.len() >= 8
        && hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
}

/// The modelled `NextToken` pattern
/// `^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$`: base64 with padding.
fn is_base64_shaped(token: &str) -> bool {
    if !token.len().is_multiple_of(4) || token.is_empty() {
        return false;
    }
    let body = token.trim_end_matches('=');
    let padding = token.len() - body.len();
    padding <= 2
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// Create one file system and return its id, with the clock-owed `CREATING` ->  `AVAILABLE`
/// transition already settled: a backup is taken of a file system that is in service.
async fn available_file_system(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok(
        router,
        "CreateFileSystem",
        &with_client_request_token(request),
    )
    .await;
    let id = created["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "CreateFileSystem must answer with a FileSystemId, got {created}"
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    id
}

/// A `CreateFileSystem` request for one row of a type table.
fn create_file_system_request(
    file_system_type: &str,
    subnets: &[&str],
    configuration: Option<(&str, Value)>,
) -> Value {
    let mut request = json!({
        "FileSystemType": file_system_type,
        "StorageCapacity": 1200,
        "StorageType": "SSD",
        "SubnetIds": subnets,
    });
    if let Some((member, configuration)) = configuration {
        request[member] = configuration;
    }
    with_client_request_token(&request)
}

/// The `WindowsConfiguration` of a create, with `ThroughputCapacity` (required by the
/// modelled shape) and the documented `CopyTagsToBackups` when the case names one.
fn windows_configuration(copy_tags_to_backups: Option<bool>) -> (&'static str, Value) {
    let mut configuration = json!({ "ThroughputCapacity": 32 });
    if let Some(copies) = copy_tags_to_backups {
        configuration["CopyTagsToBackups"] = json!(copies);
    }
    ("WindowsConfiguration", configuration)
}

/// The `LustreConfiguration` of a create: `deployment_type` is what decides whether the
/// file system can be backed up at all.
fn lustre_configuration(
    deployment_type: Option<&str>,
    copy_tags_to_backups: Option<bool>,
) -> (&'static str, Value) {
    let mut configuration = json!({ "PerUnitStorageThroughput": 50 });
    if let Some(deployment_type) = deployment_type {
        configuration["DeploymentType"] = json!(deployment_type);
    }
    if let Some(copies) = copy_tags_to_backups {
        configuration["CopyTagsToBackups"] = json!(copies);
    }
    ("LustreConfiguration", configuration)
}

/// The `OpenZFSConfiguration` of a create, with the members the modelled shape requires.
fn open_zfs_configuration(copy_tags_to_backups: Option<bool>) -> (&'static str, Value) {
    let mut configuration = json!({
        "DeploymentType": "SINGLE_AZ_1",
        "ThroughputCapacity": 64,
    });
    if let Some(copies) = copy_tags_to_backups {
        configuration["CopyTagsToBackups"] = json!(copies);
    }
    ("OpenZFSConfiguration", configuration)
}

/// Create one backup of `file_system_id`, requiring a 200, and return the `Backup`.
async fn backup_of(router: &MultiServiceRouter, file_system_id: &str) -> Value {
    ok(
        router,
        "CreateBackup",
        &with_client_request_token(&json!({ "FileSystemId": file_system_id })),
    )
    .await["Backup"]
        .clone()
}

/// Describe exactly one backup, requiring it to exist.
async fn describe_one(router: &MultiServiceRouter, backup_id: &str) -> Value {
    let body = ok(router, "DescribeBackups", &json!({ "BackupIds": [backup_id] })).await;
    let backups = body["Backups"]
        .as_array()
        .expect("Backups must be a list")
        .clone();
    assert_eq!(
        backups.len(),
        1,
        "describing {backup_id} must return exactly one backup, got {body}"
    );
    backups[0].clone()
}

/// The `BackupId`s of a `DescribeBackups` page, sorted.
fn listed_backup_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body["Backups"]
        .as_array()
        .expect("Backups must be a list")
        .iter()
        .map(|backup| backup["BackupId"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// Dispatch one operation, require a failure, and return `(status, error code, Message)`.
async fn error_of(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, parsed) = call(router, operation, body).await;
    assert_ne!(
        status, 200,
        "{operation} must fail, got HTTP 200 with body {parsed}"
    );
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        // awsJson error codes may be namespaced (`com.amazonaws.fsx#X`); clients match on
        // the shape name.
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = parsed
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// RULE: a created backup is persisted, and the response is that stored record, so a
/// following `DescribeBackups` reports the very same values — for every file-system type a
/// backup can be taken of.
#[tokio::test]
async fn create_backup_persists_a_record_describe_backups_reports_back() {
    let router = emulator().await;
    // (FileSystemType, the per-type configuration member) over the modelled
    // `FileSystemType` enum (WINDOWS | LUSTRE | ONTAP | OPENZFS), with the Lustre rows on
    // the deployment types this operation documents as backable.
    let cases: &[(&str, Option<(&str, Value)>)] = &[
        ("WINDOWS", Some(windows_configuration(None))),
        (
            "LUSTRE",
            Some(lustre_configuration(Some("PERSISTENT_1"), None)),
        ),
        (
            "LUSTRE",
            Some(lustre_configuration(Some("PERSISTENT_2"), None)),
        ),
        ("ONTAP", None),
        ("OPENZFS", Some(open_zfs_configuration(None))),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (file_system_type, configuration) in cases {
        let case = file_system_type;
        let subnets = [router.subnet_a(), router.subnet_b()];
        let file_system_id = available_file_system(
            &router,
            &create_file_system_request(file_system_type, &subnets, configuration.clone()),
        )
        .await;

        let backup = backup_of(&router, &file_system_id).await;
        let backup_id = backup["BackupId"].as_str().unwrap_or_default().to_string();

        assert!(
            is_well_formed_backup_id(&backup_id),
            "case {case:?}: expected a BackupId matching ^backup-[0-9a-f]{{8,}}$ of length \
             12..128, got {backup_id:?}"
        );
        assert!(
            !minted.contains(&backup_id),
            "case {case:?}: expected a fresh BackupId, got the already-minted {backup_id:?}"
        );
        assert_eq!(
            backup["Lifecycle"], "CREATING",
            "case {case:?}: expected Lifecycle CREATING on create, got {:?}",
            backup["Lifecycle"]
        );
        assert_eq!(
            backup["ProgressPercent"], 0,
            "case {case:?}: expected ProgressPercent 0 while CREATING, got {:?}",
            backup["ProgressPercent"]
        );
        assert_eq!(
            backup["Type"], "USER_INITIATED",
            "case {case:?}: expected Type USER_INITIATED, got {:?}",
            backup["Type"]
        );
        assert_eq!(
            backup["ResourceType"], "FILE_SYSTEM",
            "case {case:?}: expected ResourceType FILE_SYSTEM, got {:?}",
            backup["ResourceType"]
        );
        assert_eq!(
            backup["OwnerId"], ACCOUNT,
            "case {case:?}: expected OwnerId {ACCOUNT}, got {:?}",
            backup["OwnerId"]
        );
        assert_eq!(
            backup["ResourceARN"],
            json!(format!("arn:aws:fsx:{REGION}:{ACCOUNT}:backup/{backup_id}")),
            "case {case:?}: expected the backup ARN for the signed region, got {:?}",
            backup["ResourceARN"]
        );
        assert!(
            backup["CreationTime"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected a CreationTime, got {:?}",
            backup["CreationTime"]
        );
        assert_eq!(
            backup["FileSystem"]["FileSystemId"], json!(file_system_id),
            "case {case:?}: expected the backed-up FileSystemId {file_system_id}, got {:?}",
            backup["FileSystem"]["FileSystemId"]
        );
        assert_eq!(
            backup["FileSystem"]["FileSystemType"], json!(*file_system_type),
            "case {case:?}: expected the backed-up FileSystemType, got {:?}",
            backup["FileSystem"]["FileSystemType"]
        );
        assert_eq!(
            backup["FileSystem"]["Lifecycle"], "AVAILABLE",
            "case {case:?}: the metadata copy must be the file system as it stood, which was \
             AVAILABLE, got {:?}",
            backup["FileSystem"]["Lifecycle"]
        );
        assert_eq!(
            backup["FileSystem"]["StorageCapacity"], json!(1200),
            "case {case:?}: expected the backed-up StorageCapacity, got {:?}",
            backup["FileSystem"]["StorageCapacity"]
        );

        // The persisted record, read back through the implemented reader.
        let described = describe_one(&router, &backup_id).await;
        for member in [
            "BackupId",
            "Type",
            "ResourceType",
            "OwnerId",
            "ResourceARN",
            "CreationTime",
            "FileSystem",
        ] {
            assert_eq!(
                described[member], backup[member],
                "case {case:?}: DescribeBackups must report the stored {member}; create said \
                 {:?} and describe said {:?}",
                backup[member], described[member]
            );
        }

        // The transition the clock owes the record: due, never slept on by the service.
        tokio::time::sleep(PAST_TRANSITION).await;
        let settled = describe_one(&router, &backup_id).await;
        assert_eq!(
            settled["Lifecycle"], "AVAILABLE",
            "case {case:?}: the clock-owed transition must leave the backup AVAILABLE, got {:?}",
            settled["Lifecycle"]
        );
        assert_eq!(
            settled["ProgressPercent"], 100,
            "case {case:?}: expected ProgressPercent 100 once AVAILABLE, got {:?}",
            settled["ProgressPercent"]
        );
        minted.push(backup_id);
    }

    let all = ok(&router, "DescribeBackups", &json!({})).await;
    let mut expected = minted.clone();
    expected.sort();
    assert_eq!(
        listed_backup_ids(&all),
        expected,
        "an unfiltered DescribeBackups must report exactly the stored backups"
    );
}

/// One row of the tag table: a label, the file-system type, the per-type configuration its
/// create carries, the tags the `CreateBackup` request names, and whether the file system's
/// own tags are the ones expected on the backup.
type TagCase<'row> = (
    &'row str,
    &'row str,
    Option<(&'row str, Value)>,
    &'row [(&'row str, &'row str)],
    bool,
);

/// RULE: the request's tags are the backup's tags, and the file system's own tags are
/// copied only when it is configured to copy them AND the request named none — which is
/// what AWS documents on `CreateBackup$Tags`.
///
/// The table spans the request's tags and every per-type configuration that declares
/// `CopyTagsToBackups` (Windows, Lustre, OpenZFS), plus one type that declares none.
#[tokio::test]
async fn backup_tags_follow_the_copy_tags_to_backups_rule() {
    let router = emulator().await;
    // The tags every file system below carries.
    let file_system_tags = [("Name", "lifecycle-case"), ("env", "test")];
    // (label, file-system type, configuration, requested tags, expected backup tags)
    let cases: &[TagCase<'_>] = &[
        (
            "windows copies",
            "WINDOWS",
            Some(windows_configuration(Some(true))),
            &[],
            true,
        ),
        (
            "windows copies but request names tags",
            "WINDOWS",
            Some(windows_configuration(Some(true))),
            &[("Name", "from-request")],
            false,
        ),
        (
            "windows does not copy",
            "WINDOWS",
            Some(windows_configuration(Some(false))),
            &[],
            false,
        ),
        (
            "windows silent about copying",
            "WINDOWS",
            Some(windows_configuration(None)),
            &[],
            false,
        ),
        (
            "lustre copies",
            "LUSTRE",
            Some(lustre_configuration(Some("PERSISTENT_1"), Some(true))),
            &[],
            true,
        ),
        (
            "openzfs copies",
            "OPENZFS",
            Some(open_zfs_configuration(Some(true))),
            &[],
            true,
        ),
        (
            "ontap declares no CopyTagsToBackups",
            "ONTAP",
            None,
            &[("plan", "nightly")],
            false,
        ),
    ];

    for case in cases {
        let (label, file_system_type, configuration, requested, copies_file_system_tags) = case;
        let subnets = [router.subnet_a(), router.subnet_b()];
        let mut create =
            create_file_system_request(file_system_type, &subnets, configuration.clone());
        create["Tags"] = json!(file_system_tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect::<Vec<Value>>());
        let file_system_id = available_file_system(&router, &create).await;

        let mut request = json!({ "FileSystemId": file_system_id });
        if !requested.is_empty() {
            request["Tags"] = json!(requested
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect::<Vec<Value>>());
        }
        let created = ok(&router, "CreateBackup", &with_client_request_token(&request)).await;
        let backup = created["Backup"].clone();

        let mut expected: Vec<(String, String)> = if *copies_file_system_tags {
            file_system_tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        } else {
            requested
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        };
        expected.sort();

        assert_eq!(
            tag_pairs(backup.get("Tags")),
            expected,
            "case {label:?}: expected the backup's tags to be {expected:?}, got {:?}",
            backup.get("Tags")
        );
        let backup_id = backup["BackupId"].as_str().unwrap_or_default().to_string();
        let described = describe_one(&router, &backup_id).await;
        assert_eq!(
            tag_pairs(described.get("Tags")),
            expected,
            "case {label:?}: DescribeBackups must report the stored tags {expected:?}, got {:?}",
            described.get("Tags")
        );
    }
}

/// RULE: a backup of a resource the account does not have is refused with the modelled
/// not-found error for that resource type, and its `Message` names the identifier the
/// request supplied.
#[tokio::test]
async fn creating_a_backup_names_the_resource_it_cannot_find() {
    let router = emulator().await;

    for file_system_id in ABSENT_FILE_SYSTEM_IDS {
        let (status, code, message) = error_of(
            &router,
            "CreateBackup",
            &with_client_request_token(&json!({ "FileSystemId": file_system_id })),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FileSystemNotFound"),
            "case {file_system_id:?}: expected HTTP 400 FileSystemNotFound, got {status} {code}"
        );
        assert_eq!(
            message,
            format!("File system '{file_system_id}' does not exist."),
            "case {file_system_id:?}: expected the Message to name the requested id, got \
             {message:?}"
        );
    }

    // A volume is a resource no operation of this emulator creates yet, so every id names
    // nothing.
    for volume_id in VOLUME_IDS {
        let (status, code, message) = error_of(
            &router,
            "CreateBackup",
            &with_client_request_token(&json!({ "VolumeId": volume_id })),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "VolumeNotFound"),
            "case {volume_id:?}: expected HTTP 400 VolumeNotFound, got {status} {code}"
        );
        assert_eq!(
            message,
            format!("Volume '{volume_id}' does not exist."),
            "case {volume_id:?}: expected the Message to name the requested id, got {message:?}"
        );
    }
}

/// RULE: a backup is taken OF one resource, so a request naming neither a file system nor a
/// volume — or both — is a client error.
#[tokio::test]
async fn a_backup_request_must_name_exactly_one_resource() {
    let router = emulator().await;
    let file_system_id = available_file_system(
        &router,
        &create_file_system_request(
            "WINDOWS",
            &[router.subnet_a()],
            Some(windows_configuration(None)),
        ),
    )
    .await;

    // (label, request, expected Message)
    let cases: &[(&str, Value, &str)] = &[
        (
            "neither",
            json!({}),
            "Either FileSystemId or VolumeId must be specified.",
        ),
        (
            "both",
            json!({
                "FileSystemId": file_system_id,
                "VolumeId": VOLUME_IDS[0],
            }),
            "FileSystemId and VolumeId must not both be specified.",
        ),
    ];

    for (label, request, expected) in cases {
        let (status, code, message) =
            error_of(&router, "CreateBackup", &with_client_request_token(request)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BadRequest"),
            "case {label:?}: expected HTTP 400 BadRequest, got {status} {code}"
        );
        assert_eq!(
            message, *expected,
            "case {label:?}: expected the Message {expected:?}, got {message:?}"
        );
    }
}

/// RULE: while a backup of a file system is still being created, a further backup of that
/// file system is refused; once the clock has settled the first, the next one is accepted.
#[tokio::test]
async fn a_second_backup_waits_for_the_one_under_way() {
    let router = emulator().await;
    let file_system_id = available_file_system(
        &router,
        &create_file_system_request(
            "WINDOWS",
            &[router.subnet_a()],
            Some(windows_configuration(None)),
        ),
    )
    .await;

    let first = backup_of(&router, &file_system_id).await;
    let first_id = first["BackupId"].as_str().unwrap_or_default().to_string();

    let (status, code, message) = error_of(
        &router,
        "CreateBackup",
        &with_client_request_token(&json!({ "FileSystemId": file_system_id })),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BackupInProgress"),
        "case \"while CREATING\": expected HTTP 400 BackupInProgress, got {status} {code}"
    );
    assert_eq!(
        message,
        format!("Another backup of file system {file_system_id} is already in progress."),
        "case \"while CREATING\": expected the Message to name the file system, got {message:?}"
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let second = backup_of(&router, &file_system_id).await;
    let second_id = second["BackupId"].as_str().unwrap_or_default().to_string();
    assert_ne!(
        second_id, first_id,
        "case \"after the first settled\": expected a second backup, got {second_id:?} again"
    );

    let listed = ok(
        &router,
        "DescribeBackups",
        &json!({ "Filters": [{ "Name": "file-system-id", "Values": [file_system_id] }] }),
    )
    .await;
    let mut expected = vec![first_id, second_id];
    expected.sort();
    assert_eq!(
        listed_backup_ids(&listed),
        expected,
        "case \"after the first settled\": both backups of the file system must be stored"
    );
}

/// RULE: a Lustre file system can be backed up only on a persistent deployment type, which
/// is what this operation documents; the table spans the modelled `LustreDeploymentType`
/// enum and the case where the create named none (AWS documents the default SCRATCH_1).
#[tokio::test]
async fn lustre_backups_require_a_persistent_deployment_type() {
    let router = emulator().await;
    // (DeploymentType in the create, the effective type, whether a backup is supported)
    let cases: &[(Option<&str>, &str, bool)] = &[
        (None, "SCRATCH_1", false),
        (Some("SCRATCH_1"), "SCRATCH_1", false),
        (Some("SCRATCH_2"), "SCRATCH_2", false),
        (Some("PERSISTENT_1"), "PERSISTENT_1", true),
        (Some("PERSISTENT_2"), "PERSISTENT_2", true),
    ];

    for case in cases {
        let (deployment_type, effective, supported) = *case;
        let file_system_id = available_file_system(
            &router,
            &create_file_system_request(
                "LUSTRE",
                &[router.subnet_a()],
                Some(lustre_configuration(deployment_type, None)),
            ),
        )
        .await;
        let request = with_client_request_token(&json!({ "FileSystemId": file_system_id }));

        if supported {
            let backup = ok(&router, "CreateBackup", &request).await["Backup"].clone();
            assert!(
                is_well_formed_backup_id(backup["BackupId"].as_str().unwrap_or_default()),
                "case {case:?}: expected a backup of a {effective} file system, got {backup}"
            );
            continue;
        }

        let (status, code, message) = error_of(&router, "CreateBackup", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "UnsupportedOperation"),
            "case {case:?}: expected HTTP 400 UnsupportedOperation, got {status} {code}"
        );
        assert_eq!(
            message,
            format!(
                "Backups are not supported for FSx for Lustre file systems with the {effective} \
                 deployment type."
            ),
            "case {case:?}: expected the Message to name the deployment type, got {message:?}"
        );
    }
}

/// RULE: one `ClientRequestToken` creates one backup — a replay with the same parameters
/// answers with the backup the first call created, and a replay with different parameters is
/// refused as incompatible.
#[tokio::test]
async fn one_client_request_token_creates_one_backup() {
    let router = emulator().await;
    let file_system_id = available_file_system(
        &router,
        &create_file_system_request(
            "WINDOWS",
            &[router.subnet_a()],
            Some(windows_configuration(None)),
        ),
    )
    .await;

    let request = json!({
        "FileSystemId": file_system_id,
        "ClientRequestToken": "one-backup-token",
        "Tags": [{ "Key": "Name", "Value": "nightly" }],
    });
    let first = ok(&router, "CreateBackup", &request).await["Backup"].clone();
    let backup_id = first["BackupId"].as_str().unwrap_or_default().to_string();

    let replay = ok(&router, "CreateBackup", &request).await["Backup"].clone();
    assert_eq!(
        replay["BackupId"], first["BackupId"],
        "case \"same token, same parameters\": expected the first backup {backup_id:?} back, got \
         {:?}",
        replay["BackupId"]
    );

    let all = ok(&router, "DescribeBackups", &json!({})).await;
    assert_eq!(
        listed_backup_ids(&all),
        vec![backup_id.clone()],
        "case \"same token, same parameters\": exactly one backup must be stored"
    );

    let mut changed = request.clone();
    changed["Tags"] = json!([{ "Key": "Name", "Value": "weekly" }]);
    let (status, code, message) = error_of(&router, "CreateBackup", &changed).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "IncompatibleParameterError"),
        "case \"same token, different parameters\": expected HTTP 400 \
         IncompatibleParameterError, got {status} {code}"
    );
    assert!(
        message.contains("one-backup-token") && message.contains(&backup_id),
        "case \"same token, different parameters\": expected the Message to name the token and \
         the backup it created, got {message:?}"
    );
}

/// RULE: the file-system metadata a backup carries is a COPY, which AWS documents "is
/// persisted even if the file system is deleted" — so a backup still describes what it was
/// taken from after that file system is gone.
#[tokio::test]
async fn a_backup_outlives_the_file_system_it_was_taken_of() {
    let router = emulator().await;
    let subnets = [router.subnet_a(), router.subnet_b()];
    let file_system_id = available_file_system(
        &router,
        &create_file_system_request("ONTAP", &subnets, None),
    )
    .await;

    let backup = backup_of(&router, &file_system_id).await;
    let backup_id = backup["BackupId"].as_str().unwrap_or_default().to_string();

    ok(
        &router,
        "DeleteFileSystem",
        &with_client_request_token(&json!({ "FileSystemId": file_system_id })),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let (status, code) = failure(
        &router,
        "DescribeFileSystems",
        &json!({ "FileSystemIds": [file_system_id] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "FileSystemNotFound"),
        "case \"deleted file system\": the file system must be gone, got {status} {code}"
    );

    let described = describe_one(&router, &backup_id).await;
    assert_eq!(
        described["FileSystem"], backup["FileSystem"],
        "case \"deleted file system\": the backup must still report the metadata it was taken \
         with, got {:?}",
        described["FileSystem"]
    );
    assert_eq!(
        described["FileSystem"]["SubnetIds"], json!(subnets),
        "case \"deleted file system\": expected the backed-up SubnetIds, got {:?}",
        described["FileSystem"]["SubnetIds"]
    );
}

/// RULE: `DescribeBackups` reports the backups the store holds; `BackupIds` selects them and
/// overrides the filters, a supported `Filter` selects by a value the record carries, and
/// the page is cut to what the caller asked for with an opaque `NextToken` scoped to the
/// selection it was minted for.
#[tokio::test]
async fn describe_backups_selects_by_id_filter_and_page() {
    let router = emulator().await;
    let windows_id = available_file_system(
        &router,
        &create_file_system_request(
            "WINDOWS",
            &[router.subnet_a()],
            Some(windows_configuration(None)),
        ),
    )
    .await;
    let lustre_id = available_file_system(
        &router,
        &create_file_system_request(
            "LUSTRE",
            &[router.subnet_a()],
            Some(lustre_configuration(Some("PERSISTENT_1"), None)),
        ),
    )
    .await;
    let windows_backup = backup_of(&router, &windows_id).await["BackupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let lustre_backup = backup_of(&router, &lustre_id).await["BackupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut every_backup = vec![windows_backup.clone(), lustre_backup.clone()];
    every_backup.sort();

    // (label, request, expected ids)
    let selections: &[(&str, Value, Vec<String>)] = &[
        (
            "one id",
            json!({ "BackupIds": [windows_backup.clone()] }),
            vec![windows_backup.clone()],
        ),
        (
            "ids override filters",
            json!({
                "BackupIds": [lustre_backup.clone()],
                "Filters": [{ "Name": "file-system-id", "Values": [windows_id.clone()] }],
            }),
            vec![lustre_backup.clone()],
        ),
        (
            "by file system",
            json!({ "Filters": [{ "Name": "file-system-id", "Values": [lustre_id.clone()] }] }),
            vec![lustre_backup.clone()],
        ),
        (
            "by file-system type",
            json!({ "Filters": [{ "Name": "file-system-type", "Values": ["WINDOWS"] }] }),
            vec![windows_backup.clone()],
        ),
        (
            "by backup type",
            json!({ "Filters": [{ "Name": "backup-type", "Values": ["USER_INITIATED"] }] }),
            every_backup.clone(),
        ),
        (
            "by a backup type nothing here has",
            json!({ "Filters": [{ "Name": "backup-type", "Values": ["AUTOMATIC"] }] }),
            Vec::new(),
        ),
        (
            "two filters must both hold",
            json!({ "Filters": [
                { "Name": "file-system-id", "Values": [windows_id.clone()] },
                { "Name": "file-system-type", "Values": ["LUSTRE"] },
            ] }),
            Vec::new(),
        ),
        ("everything", json!({}), every_backup.clone()),
    ];

    for (label, request, expected) in selections {
        let body = ok(&router, "DescribeBackups", request).await;
        assert_eq!(
            listed_backup_ids(&body),
            *expected,
            "case {label:?}: expected {expected:?}, got {body}"
        );
    }

    // MaxResults cuts the page, and the token walks the rest of the same selection.
    let first_page = ok(&router, "DescribeBackups", &json!({ "MaxResults": 1 })).await;
    assert_eq!(
        first_page["Backups"].as_array().map(Vec::len),
        Some(1),
        "case \"MaxResults 1\": expected one backup per page, got {first_page}"
    );
    let token = first_page["NextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        is_base64_shaped(&token),
        "case \"MaxResults 1\": expected a base64-shaped NextToken, got {token:?}"
    );
    let mut walked = listed_backup_ids(&first_page);
    let second_page = ok(
        &router,
        "DescribeBackups",
        &json!({ "MaxResults": 1, "NextToken": token.clone() }),
    )
    .await;
    walked.extend(listed_backup_ids(&second_page));
    walked.sort();
    assert_eq!(
        walked, every_backup,
        "case \"MaxResults 1\": paging must walk every stored backup exactly once"
    );
    assert_eq!(
        second_page["NextToken"],
        Value::Null,
        "case \"MaxResults 1\": the last page must carry no NextToken, got {:?}",
        second_page["NextToken"]
    );

    // (label, request, expected status, expected code, expected Message)
    let refusals: &[(&str, Value, u16, &str, String)] = &[
        (
            "an absent backup id",
            json!({ "BackupIds": ["backup-0123456789abcdef0"] }),
            400,
            "BackupNotFound",
            "Backup 'backup-0123456789abcdef0' does not exist.".to_string(),
        ),
        (
            "a filter naming an absent file system",
            json!({ "Filters": [{
                "Name": "file-system-id",
                "Values": [ABSENT_FILE_SYSTEM_IDS[1]],
            }] }),
            400,
            "FileSystemNotFound",
            format!("File system '{}' does not exist.", ABSENT_FILE_SYSTEM_IDS[1]),
        ),
        (
            "a filter naming a volume",
            json!({ "Filters": [{ "Name": "volume-id", "Values": [VOLUME_IDS[1]] }] }),
            400,
            "VolumeNotFound",
            format!("Volume '{}' does not exist.", VOLUME_IDS[1]),
        ),
        (
            "a filter this listing does not support",
            json!({ "Filters": [{ "Name": "file-cache-id", "Values": ["fc-0123456789abcdef0"] }] }),
            400,
            "BadRequest",
            "The filter name 'file-cache-id' is not supported by DescribeBackups. The supported \
             names are file-system-id, backup-type, file-system-type, volume-id."
                .to_string(),
        ),
        (
            "a token minted for another selection",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{ "Name": "file-system-type", "Values": ["WINDOWS"] }],
            }),
            400,
            "BadRequest",
            "NextToken is not a token this service issued".to_string(),
        ),
        (
            "a token this service never minted",
            json!({ "NextToken": "bm90LW1pbmU=" }),
            400,
            "BadRequest",
            "NextToken is not a token this service issued".to_string(),
        ),
    ];

    for (label, request, expected_status, expected_code, expected_message) in refusals {
        let (status, code, message) = error_of(&router, "DescribeBackups", request).await;
        assert_eq!(
            (status, code.as_str()),
            (*expected_status, *expected_code),
            "case {label:?}: expected HTTP {expected_status} {expected_code}, got {status} {code}"
        );
        assert_eq!(
            message, *expected_message,
            "case {label:?}: expected the Message {expected_message:?}, got {message:?}"
        );
    }
}

/// RULE: a stored backup is removed only once it has been taken: deleting one that is still
/// being created is refused with the modelled error, a delete that goes through answers
/// `DELETED` and the backup is gone from later reads, and one `ClientRequestToken` makes the
/// delete idempotent.
#[tokio::test]
async fn delete_backup_removes_a_taken_backup_and_protects_one_under_way() {
    let router = emulator().await;
    let file_system_id = available_file_system(
        &router,
        &create_file_system_request(
            "WINDOWS",
            &[router.subnet_a()],
            Some(windows_configuration(None)),
        ),
    )
    .await;
    let backup_id = backup_of(&router, &file_system_id).await["BackupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Still CREATING: the data is not there yet, so it cannot be thrown away.
    let (status, code, message) = error_of(
        &router,
        "DeleteBackup",
        &with_client_request_token(&json!({ "BackupId": backup_id })),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BackupInProgress"),
        "case \"while CREATING\": expected HTTP 400 BackupInProgress, got {status} {code}"
    );
    assert_eq!(
        message,
        format!("Backup {backup_id} cannot be deleted while it is in CREATING state."),
        "case \"while CREATING\": expected the Message to name the backup and its lifecycle, \
         got {message:?}"
    );
    assert_eq!(
        describe_one(&router, &backup_id).await["BackupId"],
        json!(backup_id),
        "case \"while CREATING\": the refused delete must leave the backup stored"
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let deleted = ok(
        &router,
        "DeleteBackup",
        &json!({ "BackupId": backup_id, "ClientRequestToken": "delete-backup-token" }),
    )
    .await;
    assert_eq!(
        (deleted["BackupId"].clone(), deleted["Lifecycle"].clone()),
        (json!(backup_id), json!("DELETED")),
        "case \"once AVAILABLE\": expected the deleted id with Lifecycle DELETED, got {deleted}"
    );

    let listed = ok(&router, "DescribeBackups", &json!({})).await;
    assert_eq!(
        listed_backup_ids(&listed),
        Vec::<String>::new(),
        "case \"once AVAILABLE\": a deleted backup must not show up in later DescribeBackups, \
         got {listed}"
    );

    // "Idempotent deletion": the same call under the same token answers as the first did.
    let replay = ok(
        &router,
        "DeleteBackup",
        &json!({ "BackupId": backup_id, "ClientRequestToken": "delete-backup-token" }),
    )
    .await;
    assert_eq!(
        replay, deleted,
        "case \"replayed delete\": expected the first answer {deleted}, got {replay}"
    );

    // The same token naming another backup is a different request.
    let other_id = {
        let other_file_system = available_file_system(
            &router,
            &create_file_system_request(
                "WINDOWS",
                &[router.subnet_a()],
                Some(windows_configuration(None)),
            ),
        )
        .await;
        backup_of(&router, &other_file_system).await["BackupId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, code, message) = error_of(
        &router,
        "DeleteBackup",
        &json!({ "BackupId": other_id, "ClientRequestToken": "delete-backup-token" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "IncompatibleParameterError"),
        "case \"token reused for another backup\": expected HTTP 400 \
         IncompatibleParameterError, got {status} {code}"
    );
    assert!(
        message.contains("delete-backup-token") && message.contains(&backup_id),
        "case \"token reused for another backup\": expected the Message to name the token and \
         the backup it deleted, got {message:?}"
    );
    assert_eq!(
        describe_one(&router, &other_id).await["BackupId"],
        json!(other_id),
        "case \"token reused for another backup\": the refused delete must leave the backup \
         stored"
    );
}

/// RULE: deleting a backup the store does not hold is refused with `BackupNotFound`, whose
/// `Message` names the identifier the request supplied.
#[tokio::test]
async fn deleting_an_absent_backup_names_it() {
    let router = emulator().await;
    // Spanning the modelled `BackupId` shape: pattern `^(backup-[0-9a-f]{8,})$` with
    // `length 12..128`, so the shortest legal id, a mid-length one and a long one.
    let absent = [
        "backup-deadbeef",
        "backup-0123456789abcdef0",
        "backup-ffffffffffffffffffffffff",
    ];

    for backup_id in absent {
        let (status, code, message) = error_of(
            &router,
            "DeleteBackup",
            &with_client_request_token(&json!({ "BackupId": backup_id })),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BackupNotFound"),
            "case {backup_id:?}: expected HTTP 400 BackupNotFound, got {status} {code}"
        );
        assert_eq!(
            message,
            format!("Backup '{backup_id}' does not exist."),
            "case {backup_id:?}: expected the Message to name the requested id, got {message:?}"
        );
    }
}

/// RULE: a backup is a taggable FSx resource like any other — the tagging operations resolve
/// its `ResourceARN` to the record `CreateBackup` stored, and what they change is what
/// `DescribeBackups` reports.
#[tokio::test]
async fn a_stored_backup_is_a_taggable_resource() {
    let router = emulator().await;
    let file_system_id = available_file_system(
        &router,
        &create_file_system_request(
            "WINDOWS",
            &[router.subnet_a()],
            Some(windows_configuration(None)),
        ),
    )
    .await;
    let backup = backup_of(&router, &file_system_id).await;
    let backup_id = backup["BackupId"].as_str().unwrap_or_default().to_string();
    let backup_arn = backup["ResourceARN"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    ok(
        &router,
        "TagResource",
        &json!({
            "ResourceARN": backup_arn,
            "Tags": [
                { "Key": "Name", "Value": "nightly" },
                { "Key": "retain", "Value": "30" },
            ],
        }),
    )
    .await;

    let listed = ok(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceARN": backup_arn }),
    )
    .await;
    assert_eq!(
        tag_pairs(listed.get("Tags")),
        vec![
            ("Name".to_string(), "nightly".to_string()),
            ("retain".to_string(), "30".to_string()),
        ],
        "case \"tagged backup\": ListTagsForResource must report the stored tags, got {listed}"
    );
    let described = describe_one(&router, &backup_id).await;
    assert_eq!(
        tag_pairs(described.get("Tags")),
        tag_pairs(listed.get("Tags")),
        "case \"tagged backup\": DescribeBackups and ListTagsForResource must report one tag \
         set, got {:?} and {:?}",
        described.get("Tags"),
        listed.get("Tags")
    );

    ok(
        &router,
        "UntagResource",
        &json!({ "ResourceARN": backup_arn, "TagKeys": ["retain"] }),
    )
    .await;
    let after = describe_one(&router, &backup_id).await;
    assert_eq!(
        tag_pairs(after.get("Tags")),
        vec![("Name".to_string(), "nightly".to_string())],
        "case \"untagged backup\": expected only the remaining tag, got {:?}",
        after.get("Tags")
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateFileSystemFromBackup`, driven over the wire.
//!
//! RULE: a restore creates a NEW file system from the metadata the backup recorded of the
//! file system it was taken of — AWS documents that its "parameters ... default to the
//! parameters of the file system that was backed up, unless overridden" — placed in the
//! subnets THIS request names, in the documented initial `CREATING` lifecycle that the
//! clock settles to `AVAILABLE`, and the response is the stored record, so a following
//! `DescribeFileSystems` reports the very same values. A request naming a backup the
//! account does not have, a subnet it does not have, or a `StorageCapacity` the documented
//! rule does not admit is refused with the modelled error for that case.
//!
//! Each case asserts that rule over a SET of inputs — every file-system type a backup can
//! be restored to, every documented override, both disk-IOPS modes, a span of absent
//! backup ids — rather than over one recorded call. The requests go through the same front
//! door as `file_system_lifecycle.rs` (its harness is reused), so nothing here reaches
//! into the store directly.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, emulator, failure, ok, tag_pairs, with_client_request_token, ACCOUNT, PAST_TRANSITION,
    REGION,
};

/// Absent `BackupId`s spanning the modelled `BackupId` shape: pattern
/// `^(backup-[0-9a-f]{8,})$` with `length 12..128`, so the shortest legal id, a mid-length
/// one and a long one.
const ABSENT_BACKUP_IDS: &[&str] = &[
    "backup-deadbeef",
    "backup-00000000deadbeef",
    "backup-ffffffffffffffffffffffffffffffff",
];

/// Create one file system, wait out its clock-owed `CREATING` -> `AVAILABLE` transition,
/// and return the stored `FileSystem`.
async fn available_file_system(router: &MultiServiceRouter, request: &Value) -> Value {
    let created = ok(router, "CreateFileSystem", &with_client_request_token(request)).await;
    let id = created["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "CreateFileSystem must answer with a FileSystemId, got {created}"
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    describe_one(router, &id).await
}

/// Take one backup of `file_system_id` and wait out its transition to `AVAILABLE`: a
/// restore is of a backup that is in service.
async fn available_backup(router: &MultiServiceRouter, file_system_id: &str) -> String {
    let created = ok(
        router,
        "CreateBackup",
        &with_client_request_token(&json!({ "FileSystemId": file_system_id })),
    )
    .await;
    let backup_id = created["Backup"]["BackupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !backup_id.is_empty(),
        "CreateBackup must answer with a BackupId, got {created}"
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    backup_id
}

/// Describe exactly one file system, requiring it to exist.
async fn describe_one(router: &MultiServiceRouter, id: &str) -> Value {
    let body = ok(
        router,
        "DescribeFileSystems",
        &json!({ "FileSystemIds": [id] }),
    )
    .await;
    let systems = body["FileSystems"]
        .as_array()
        .expect("FileSystems must be a list")
        .clone();
    assert_eq!(
        systems.len(),
        1,
        "describing {id} must return exactly one file system, got {body}"
    );
    systems[0].clone()
}

/// The members of a file system's per-type configuration a restore must have inherited:
/// the ones that define its placement-independent shape and its provisioned performance.
fn inherited_shape(file_system: &Value, member: &str) -> Value {
    let configuration = &file_system[member];
    json!({
        "FileSystemType": file_system["FileSystemType"],
        "StorageCapacity": file_system["StorageCapacity"],
        "StorageType": file_system["StorageType"],
        "DeploymentType": configuration["DeploymentType"],
        "ThroughputCapacity": configuration["ThroughputCapacity"],
        "DiskIopsConfiguration": configuration["DiskIopsConfiguration"],
    })
}

/// Dispatch one operation, require a failure, and return `(status, error code, Message)`.
async fn error_of(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, code) = failure(router, operation, body).await;
    let (_, parsed) = call(router, operation, body).await;
    let message = parsed
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// RULE: a restore that states no configuration of its own reports the configuration the
/// backup recorded — type, capacity, storage type, deployment, throughput and the
/// provisioned SSD IOPS — on the create response and on every later read, for both
/// disk-IOPS modes.
#[tokio::test]
async fn a_restore_inherits_the_configuration_the_backup_recorded() {
    let router = emulator().await;
    // (FileSystemType, the per-type configuration member, the create's configuration). The
    // `OPENZFS` rows leave `DiskIopsConfiguration` to the documented AUTOMATIC default; the
    // `WINDOWS` rows provision their own, so the inheritance is asserted in both modes.
    let cases: &[(&str, &str, Value)] = &[
        (
            "OPENZFS",
            "OpenZFSConfiguration",
            json!({
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 64,
                "AutomaticBackupRetentionDays": 0,
            }),
        ),
        (
            "OPENZFS",
            "OpenZFSConfiguration",
            json!({
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 64,
                "DiskIopsConfiguration": { "Mode": "USER_PROVISIONED", "Iops": 400 },
            }),
        ),
        (
            "WINDOWS",
            "WindowsConfiguration",
            json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 32 }),
        ),
        (
            "WINDOWS",
            "WindowsConfiguration",
            json!({
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 32,
                "DiskIopsConfiguration": { "Mode": "USER_PROVISIONED", "Iops": 1200 },
            }),
        ),
        (
            "LUSTRE",
            "LustreConfiguration",
            json!({ "DeploymentType": "PERSISTENT_1", "PerUnitStorageThroughput": 50 }),
        ),
    ];

    for (file_system_type, member, configuration) in cases {
        let case = format!("{file_system_type} {configuration}");
        let subnet = router.subnet_a().to_string();
        let mut create = json!({
            "FileSystemType": file_system_type,
            "StorageCapacity": 1200,
            "StorageType": "SSD",
            "SubnetIds": [subnet],
        });
        create[*member] = configuration.clone();
        let source = available_file_system(&router, &create).await;
        let source_id = source["FileSystemId"].as_str().unwrap_or_default().to_string();
        let backup_id = available_backup(&router, &source_id).await;

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
        let restored_id = restored["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert_ne!(
            restored_id, source_id,
            "case {case:?}: expected a restore to mint a new FileSystemId, got the source's \
             {source_id}"
        );
        assert_eq!(
            restored["Lifecycle"], "CREATING",
            "case {case:?}: expected the documented initial Lifecycle CREATING, got {:?}",
            restored["Lifecycle"]
        );
        assert_eq!(
            inherited_shape(&restored, member),
            inherited_shape(&source, member),
            "case {case:?}: expected the restore to report the backed-up configuration, got \
             {restored}"
        );
        assert_eq!(
            restored["ResourceARN"],
            json!(format!(
                "arn:aws:fsx:{REGION}:{ACCOUNT}:file-system/{restored_id}"
            )),
            "case {case:?}: expected the restored file system's own ARN, got {:?}",
            restored["ResourceARN"]
        );
        assert_eq!(
            restored["VpcId"],
            json!(router.subnet_vpc(&subnet).await),
            "case {case:?}: expected the VPC of the subnet the restore names, got {:?}",
            restored["VpcId"]
        );
        assert_eq!(
            restored["SubnetIds"],
            json!([subnet]),
            "case {case:?}: expected the subnets the restore names, got {:?}",
            restored["SubnetIds"]
        );

        // The stored record, read back: the restore reported what it persisted, and the
        // clock owes it the way to AVAILABLE.
        tokio::time::sleep(PAST_TRANSITION).await;
        let described = describe_one(&router, &restored_id).await;
        assert_eq!(
            described["Lifecycle"], "AVAILABLE",
            "case {case:?}: expected the restored file system to reach AVAILABLE, got {:?}",
            described["Lifecycle"]
        );
        assert_eq!(
            inherited_shape(&described, member),
            inherited_shape(&source, member),
            "case {case:?}: expected DescribeFileSystems to report the backed-up \
             configuration, got {described}"
        );

        // The source is untouched by the restore: a backup records what it was taken from.
        let source_again = describe_one(&router, &source_id).await;
        assert_eq!(
            inherited_shape(&source_again, member),
            inherited_shape(&source, member),
            "case {case:?}: expected the source file system to be unchanged by the restore, \
             got {source_again}"
        );
    }
}

/// RULE: each member a restore states OVERRIDES what the backup recorded, and the members
/// it does not state keep the backup's values — including the disk-IOPS count, which is
/// recomputed from the capacity the restore stored while the mode stays `AUTOMATIC`.
#[tokio::test]
async fn a_restore_overrides_only_the_members_it_states() {
    let router = emulator().await;
    let subnet = router.subnet_a().to_string();
    let source = available_file_system(
        &router,
        &json!({
            "FileSystemType": "OPENZFS",
            "StorageCapacity": 64,
            "StorageType": "SSD",
            "SubnetIds": [subnet],
            "OpenZFSConfiguration": {
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 64,
            },
        }),
    )
    .await;
    let source_id = source["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let backup_id = available_backup(&router, &source_id).await;

    // "Sets the storage capacity of the OpenZFS file system that you're creating from a
    // backup ... the value that you specify must be equal to or greater than the backup's
    // storage capacity value": a restore may grow it, and the AUTOMATIC IOPS follow at 3
    // per GiB (3 * 256).
    let restored = ok(
        &router,
        "CreateFileSystemFromBackup",
        &with_client_request_token(&json!({
            "BackupId": backup_id,
            "SubnetIds": [subnet],
            "StorageCapacity": 256,
            "Tags": [{ "Key": "Name", "Value": "restored" }],
        })),
    )
    .await["FileSystem"]
        .clone();
    assert_eq!(
        restored["StorageCapacity"], 256,
        "case \"grown to 256 GiB\": expected the stated StorageCapacity, got {:?}",
        restored["StorageCapacity"]
    );
    assert_eq!(
        restored["OpenZFSConfiguration"]["DiskIopsConfiguration"],
        json!({ "Mode": "AUTOMATIC", "Iops": 768 }),
        "case \"grown to 256 GiB\": expected the AUTOMATIC rate on the stored capacity, got \
         {:?}",
        restored["OpenZFSConfiguration"]["DiskIopsConfiguration"]
    );
    assert_eq!(
        restored["OpenZFSConfiguration"]["ThroughputCapacity"],
        source["OpenZFSConfiguration"]["ThroughputCapacity"],
        "case \"grown to 256 GiB\": expected the un-stated ThroughputCapacity to be the \
         backup's, got {:?}",
        restored["OpenZFSConfiguration"]["ThroughputCapacity"]
    );
    assert_eq!(
        tag_pairs(restored.get("Tags")),
        vec![("Name".to_string(), "restored".to_string())],
        "case \"grown to 256 GiB\": expected the tags the restore states, got {:?}",
        restored["Tags"]
    );

    // An OpenZFS configuration the restore states of its own replaces the backup's, and
    // the disk-IOPS it provisions is honoured as provisioned.
    let restored = ok(
        &router,
        "CreateFileSystemFromBackup",
        &with_client_request_token(&json!({
            "BackupId": backup_id,
            "SubnetIds": [subnet],
            "OpenZFSConfiguration": {
                "DeploymentType": "SINGLE_AZ_2",
                "ThroughputCapacity": 160,
                "DiskIopsConfiguration": { "Mode": "USER_PROVISIONED", "Iops": 3000 },
            },
        })),
    )
    .await["FileSystem"]
        .clone();
    assert_eq!(
        restored["OpenZFSConfiguration"]["DeploymentType"], "SINGLE_AZ_2",
        "case \"restated configuration\": expected the stated DeploymentType, got {:?}",
        restored["OpenZFSConfiguration"]["DeploymentType"]
    );
    assert_eq!(
        restored["OpenZFSConfiguration"]["DiskIopsConfiguration"],
        json!({ "Mode": "USER_PROVISIONED", "Iops": 3000 }),
        "case \"restated configuration\": expected the provisioned IOPS, got {:?}",
        restored["OpenZFSConfiguration"]["DiskIopsConfiguration"]
    );
    assert_eq!(
        restored["StorageCapacity"], source["StorageCapacity"],
        "case \"restated configuration\": expected the un-stated StorageCapacity to be the \
         backup's, got {:?}",
        restored["StorageCapacity"]
    );

    // `StorageType` and `KmsKeyId` are stated members too, and a restore that states them
    // reports them back.
    let restored = ok(
        &router,
        "CreateFileSystemFromBackup",
        &with_client_request_token(&json!({
            "BackupId": backup_id,
            "SubnetIds": [subnet],
            "StorageType": "INTELLIGENT_TIERING",
            "KmsKeyId": format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/restore-key"),
        })),
    )
    .await["FileSystem"]
        .clone();
    assert_eq!(
        restored["StorageType"], "INTELLIGENT_TIERING",
        "case \"restated storage type\": expected the stated StorageType, got {:?}",
        restored["StorageType"]
    );
    assert_eq!(
        restored["KmsKeyId"],
        json!(format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/restore-key")),
        "case \"restated storage type\": expected the stated KmsKeyId, got {:?}",
        restored["KmsKeyId"]
    );
}

/// One row of the storage-capacity table: a file system backed up at one capacity, and the
/// capacities a restore of that backup admits and refuses.
struct CapacityCase {
    /// The modelled `FileSystemType` value.
    file_system_type: &'static str,
    /// The `FileSystem` member the per-type configuration is reported in.
    member: &'static str,
    /// The create request's per-type configuration.
    configuration: Value,
    /// The `StorageCapacity` the backed-up file system was created with, in GiB.
    capacity: i64,
    /// Capacities a restore of that backup is served at.
    admitted: &'static [i64],
    /// Capacities the documented rule refuses.
    refused: &'static [i64],
}

/// RULE: AWS documents the `StorageCapacity` a restore admits — equal to or greater than
/// the backup's for OpenZFS, and matching the backup's for every other file-system type —
/// and refuses any other value with HTTP 400.
#[tokio::test]
async fn a_restore_refuses_a_storage_capacity_the_backup_does_not_admit() {
    let router = emulator().await;
    let subnet = router.subnet_a().to_string();
    let cases = &[
        CapacityCase {
            file_system_type: "OPENZFS",
            member: "OpenZFSConfiguration",
            configuration: json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 64 }),
            capacity: 128,
            admitted: &[128, 129, 4096],
            refused: &[0, 64, 127],
        },
        CapacityCase {
            file_system_type: "WINDOWS",
            member: "WindowsConfiguration",
            configuration: json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 32 }),
            capacity: 128,
            admitted: &[128],
            refused: &[0, 64, 127, 129, 4096],
        },
    ];

    for CapacityCase {
        file_system_type,
        member,
        configuration,
        capacity,
        admitted,
        refused,
    } in cases
    {
        let mut create = json!({
            "FileSystemType": file_system_type,
            "StorageCapacity": capacity,
            "StorageType": "SSD",
            "SubnetIds": [subnet],
        });
        create[*member] = configuration.clone();
        let source = available_file_system(&router, &create).await;
        let source_id = source["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let backup_id = available_backup(&router, &source_id).await;

        for requested in *admitted {
            let case = format!("{file_system_type} backed up at {capacity}, restored at {requested}");
            let restored = ok(
                &router,
                "CreateFileSystemFromBackup",
                &with_client_request_token(&json!({
                    "BackupId": backup_id,
                    "SubnetIds": [subnet],
                    "StorageCapacity": requested,
                })),
            )
            .await["FileSystem"]
                .clone();
            assert_eq!(
                restored["StorageCapacity"],
                json!(requested),
                "case {case:?}: expected the restore to be served at the stated capacity, got \
                 {:?}",
                restored["StorageCapacity"]
            );
        }

        for requested in *refused {
            let case = format!("{file_system_type} backed up at {capacity}, restored at {requested}");
            let (status, code, message) = error_of(
                &router,
                "CreateFileSystemFromBackup",
                &with_client_request_token(&json!({
                    "BackupId": backup_id,
                    "SubnetIds": [subnet],
                    "StorageCapacity": requested,
                })),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "BadRequest"),
                "case {case:?}: expected HTTP 400 BadRequest, got {status} {code} with message \
                 {message:?}"
            );
            assert!(
                message.contains(&capacity.to_string()),
                "case {case:?}: expected the refusal to name the backup's capacity \
                 {capacity}, got {message:?}"
            );
        }
    }
}

/// RULE: a restore names a backup the account HAS — one it does not is the modelled
/// `BackupNotFound`, naming the id the request carried — and places the file system in
/// subnets the account HAS, one it does not being `InvalidNetworkSettings` naming that id.
#[tokio::test]
async fn a_restore_of_a_resource_the_account_does_not_have_is_refused() {
    let router = emulator().await;
    let subnet = router.subnet_a().to_string();

    for backup_id in ABSENT_BACKUP_IDS {
        let (status, code, message) = error_of(
            &router,
            "CreateFileSystemFromBackup",
            &with_client_request_token(&json!({
                "BackupId": backup_id,
                "SubnetIds": [subnet],
            })),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BackupNotFound"),
            "case {backup_id:?}: expected HTTP 400 BackupNotFound, got {status} {code} with \
             message {message:?}"
        );
        assert_eq!(
            message,
            format!("Backup '{backup_id}' does not exist."),
            "case {backup_id:?}: expected the refusal to name the requested backup, got \
             {message:?}"
        );
    }

    let source = available_file_system(
        &router,
        &json!({
            "FileSystemType": "OPENZFS",
            "StorageCapacity": 64,
            "StorageType": "SSD",
            "SubnetIds": [subnet],
            "OpenZFSConfiguration": {
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 64,
            },
        }),
    )
    .await;
    let backup_id = available_backup(
        &router,
        source["FileSystemId"].as_str().unwrap_or_default(),
    )
    .await;

    // Subnet ids matching the modelled `SubnetId` pattern that name no subnet of the
    // account.
    for absent_subnet in ["subnet-deadbeef", "subnet-00000000deadbeef"] {
        let (status, code, message) = error_of(
            &router,
            "CreateFileSystemFromBackup",
            &with_client_request_token(&json!({
                "BackupId": backup_id,
                "SubnetIds": [absent_subnet],
            })),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNetworkSettings"),
            "case {absent_subnet:?}: expected HTTP 400 InvalidNetworkSettings, got {status} \
             {code} with message {message:?}"
        );
        assert!(
            message.contains(absent_subnet),
            "case {absent_subnet:?}: expected the refusal to name the subnet, got {message:?}"
        );
    }

    // An empty `SubnetIds` satisfies the modelled `length min 0` but places the file system
    // nowhere.
    let (status, code, message) = error_of(
        &router,
        "CreateFileSystemFromBackup",
        &with_client_request_token(&json!({ "BackupId": backup_id, "SubnetIds": [] })),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidNetworkSettings"),
        "case \"no subnets\": expected HTTP 400 InvalidNetworkSettings, got {status} {code} \
         with message {message:?}"
    );
}

/// RULE: the `@idempotencyToken` behaviour AWS documents for this operation — a repeat
/// carrying the same token and the same parameters returns the file system the first call
/// created, and one carrying the same token with different parameters is
/// `IncompatibleParameterError`.
#[tokio::test]
async fn a_restore_replayed_with_one_token_creates_one_file_system() {
    let router = emulator().await;
    let subnet = router.subnet_a().to_string();
    let source = available_file_system(
        &router,
        &json!({
            "FileSystemType": "OPENZFS",
            "StorageCapacity": 64,
            "StorageType": "SSD",
            "SubnetIds": [subnet],
            "OpenZFSConfiguration": {
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 64,
            },
        }),
    )
    .await;
    let backup_id = available_backup(
        &router,
        source["FileSystemId"].as_str().unwrap_or_default(),
    )
    .await;

    let request = json!({
        "BackupId": backup_id,
        "SubnetIds": [subnet],
        "ClientRequestToken": "restore-replay-token",
    });
    let first = ok(&router, "CreateFileSystemFromBackup", &request).await["FileSystem"].clone();
    let replay = ok(&router, "CreateFileSystemFromBackup", &request).await["FileSystem"].clone();
    assert_eq!(
        replay["FileSystemId"], first["FileSystemId"],
        "case \"same token, same parameters\": expected the first file system back, got {:?}",
        replay["FileSystemId"]
    );
    assert_eq!(
        replay, first,
        "case \"same token, same parameters\": expected the stored record unchanged, got \
         {replay}"
    );

    let mut changed = request.clone();
    changed["StorageCapacity"] = json!(256);
    let (status, code, message) = error_of(&router, "CreateFileSystemFromBackup", &changed).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "IncompatibleParameterError"),
        "case \"same token, different parameters\": expected HTTP 400 \
         IncompatibleParameterError, got {status} {code} with message {message:?}"
    );
}

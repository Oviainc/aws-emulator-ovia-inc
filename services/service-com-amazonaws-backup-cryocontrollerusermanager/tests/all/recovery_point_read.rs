// SPDX-License-Identifier: CC0-1.0

//! `DescribeRecoveryPoint`: a recovery point is a STORED resource, and the read
//! reports the point the store holds.
//!
//! The rules under test:
//!
//! * a read addressed by (`BackupVaultName`, `RecoveryPointArn`) to a stored
//!   recovery point reports every member from that stored point — its ARNs, the
//!   protected resource and its derived type, the role, the size, the lifecycle
//!   the backup was stored under and the instants that lifecycle makes due, and
//!   the key it is encrypted under;
//! * the read reports the state the point's backup settled into, and the only
//!   write it performs is materialising the completion the clock had already
//!   made due — so repeated reads do not move it, and a point whose backup is
//!   still running is not addressable yet;
//! * `StorageClass` follows that lifecycle: a point is warm until the transition
//!   its lifecycle makes due has come, and cold from that instant;
//! * this read and the vault's listing project the SAME stored point, so the two
//!   can never disagree about it;
//! * every way the pair can fail to name a stored recovery point is ONE refusal
//!   (400 `ResourceNotFoundException`), a `RecoveryPointArn` that is not an ARN
//!   is a bad parameter value instead, and a vault outside the caller's
//!   visibility — or a `RecoveryPointArn` of another account — is refused 403
//!   `AccessDeniedException` before existence is considered at all.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `backup` SDK client sees. The request harness is the shared
//! one in `vault_and_backup_job.rs`, and the engine-keeping router plus the
//! point-aging seam are the ones in `recovery_point_deletion.rs` — a retention
//! and a cold-storage transition are counted in days, so no test can wait one
//! out.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_point_deletion::{age_point, listed_points, router_with_engine, start_point};
use crate::vault_and_backup_job::{
    call, create_vault, error_code, router, start_job, string_at, ACCOUNT, PAST_SETTLE, REGION,
};
use crate::vault_read_visibility::DENIED_MESSAGE;

/// The role every case backs its resource up with — the same spelling the
/// shared `start_point` sends, so the read can be asserted against it.
fn backup_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// The members `DescribeRecoveryPoint` and `ListRecoveryPointsByBackupVault`
/// both report for one recovery point: the projection the two share
/// (`smithy_query(query="operation",
/// operation="DescribeRecoveryPoint,ListRecoveryPointsByBackupVault")` —
/// `DescribeRecoveryPointOutput` and `RecoveryPointByBackupVault` declare each
/// of these).
const SHARED_MEMBERS: &[&str] = &[
    "RecoveryPointArn",
    "BackupVaultName",
    "BackupVaultArn",
    "ResourceArn",
    "ResourceType",
    "IamRoleArn",
    "Status",
    "CreationDate",
    "CompletionDate",
    "BackupSizeInBytes",
    "Lifecycle",
    "CalculatedLifecycle",
    "EncryptionKeyArn",
    "EncryptionKeyType",
    "IsEncrypted",
    "IsParent",
    "VaultType",
    "IndexStatus",
];

/// `GET /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}`, as
/// the model binds this operation, with the documented `backupVaultAccountId`
/// query parameter when the case names an account.
async fn describe_point(
    router: &MultiServiceRouter,
    vault: &str,
    arn: &str,
    account: Option<&str>,
) -> (u16, Value) {
    let path = match account {
        Some(account) => format!(
            "/backup-vaults/{vault}/recovery-points/{arn}?backupVaultAccountId={account}"
        ),
        None => format!("/backup-vaults/{vault}/recovery-points/{arn}"),
    };
    call(router, "GET", &path, None).await
}

/// A timestamp member as epoch seconds, or `None` when the response omits it.
fn instant_at(body: &Value, member: &str) -> Option<f64> {
    body.get(member).and_then(Value::as_f64)
}

/// One row of the read table: what the backup was stored as, and what the read
/// must report back from the store.
struct ReadCase {
    name: &'static str,
    /// The `CreateBackupVault` body, which decides the key the point is stored
    /// under.
    vault_body: String,
    /// The key the vault was created with, when the row names one.
    vault_key: Option<String>,
    resource_arn: String,
    lifecycle: Option<&'static str>,
    resource_type: &'static str,
    encryption_key_type: &'static str,
    is_encrypted: bool,
    /// Whether the stored lifecycle makes a deletion due.
    delete_at: bool,
    /// Whether it makes a cold-storage transition due.
    move_to_cold_storage_at: bool,
}

/// A stored recovery point reads back through `DescribeRecoveryPoint`, every
/// member computed from the point the backup stored.
///
/// The rows differ along what the rule generalises over: the protected resource
/// (and so the derived `ResourceType`), the lifecycle form the model admits
/// (none at all, a retention, a retention with a cold-storage transition, and
/// the documented keep-indefinitely sentinel) and the key the holding vault
/// encrypts its backups with.
#[tokio::test]
async fn a_stored_recovery_point_reads_back_through_describe() {
    let key = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/1234abcd-12ab-34cd-56ef-1234567890ab");
    let cases: Vec<ReadCase> = vec![
        ReadCase {
            name: "a DynamoDB table with no lifecycle",
            vault_body: "{}".to_string(),
            vault_key: None,
            resource_arn: format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
            lifecycle: None,
            resource_type: "DynamoDB",
            encryption_key_type: "AWS_OWNED_KMS_KEY",
            is_encrypted: false,
            delete_at: false,
            move_to_cold_storage_at: false,
        },
        ReadCase {
            name: "an EFS file system retained for 35 days",
            vault_body: "{}".to_string(),
            vault_key: None,
            resource_arn: format!(
                "arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-012345"
            ),
            lifecycle: Some(r#"{"DeleteAfterDays":35}"#),
            resource_type: "EFS",
            encryption_key_type: "AWS_OWNED_KMS_KEY",
            is_encrypted: false,
            delete_at: true,
            move_to_cold_storage_at: false,
        },
        ReadCase {
            name: "an S3 bucket moved to cold storage then deleted",
            vault_body: "{}".to_string(),
            vault_key: None,
            resource_arn: "arn:aws:s3:::demo-bucket".to_string(),
            lifecycle: Some(r#"{"MoveToColdStorageAfterDays":30,"DeleteAfterDays":120}"#),
            resource_type: "S3",
            encryption_key_type: "AWS_OWNED_KMS_KEY",
            is_encrypted: false,
            delete_at: true,
            move_to_cold_storage_at: true,
        },
        ReadCase {
            name: "an EBS volume kept indefinitely",
            vault_body: "{}".to_string(),
            vault_key: None,
            resource_arn: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-01234567"),
            lifecycle: Some(r#"{"DeleteAfterDays":-1}"#),
            resource_type: "EBS",
            encryption_key_type: "AWS_OWNED_KMS_KEY",
            is_encrypted: false,
            delete_at: false,
            move_to_cold_storage_at: false,
        },
        ReadCase {
            name: "an Aurora cluster in a customer-managed-key vault",
            vault_body: format!(r#"{{"EncryptionKeyArn":"{key}"}}"#),
            vault_key: Some(key.clone()),
            resource_arn: format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:aurora-demo"),
            lifecycle: Some(r#"{"DeleteAfterDays":7}"#),
            resource_type: "Aurora",
            encryption_key_type: "CUSTOMER_MANAGED_KMS_KEY",
            is_encrypted: true,
            delete_at: true,
            move_to_cold_storage_at: false,
        },
        ReadCase {
            name: "a resource whose namespace the type catalog does not name",
            vault_body: "{}".to_string(),
            vault_key: None,
            resource_arn: format!("arn:aws:storagegateway:{REGION}:{ACCOUNT}:gateway/sgw-012345"),
            lifecycle: None,
            resource_type: "storagegateway",
            encryption_key_type: "AWS_OWNED_KMS_KEY",
            is_encrypted: false,
            delete_at: false,
            move_to_cold_storage_at: false,
        },
    ];

    for case in &cases {
        let name = case.name;
        let router = router();
        let (status, created) = create_vault(&router, "read-rp", &case.vault_body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: setup CreateBackupVault should succeed, got {status} {created:?}"
        );
        let arn = start_point(&router, "read-rp", &case.resource_arn, case.lifecycle).await;
        std::thread::sleep(PAST_SETTLE);

        let (status, read) = describe_point(&router, "read-rp", &arn, None).await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (200, "<none>"),
            "case {name:?}: expected DescribeRecoveryPoint 200, got {status} {read:?}"
        );
        for (member, expected) in [
            ("RecoveryPointArn", arn.clone()),
            ("BackupVaultName", "read-rp".to_string()),
            (
                "BackupVaultArn",
                format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:read-rp"),
            ),
            ("ResourceArn", case.resource_arn.clone()),
            ("ResourceType", case.resource_type.to_string()),
            ("IamRoleArn", backup_role()),
            ("Status", "COMPLETED".to_string()),
            ("VaultType", "BACKUP_VAULT".to_string()),
            ("StorageClass", "WARM".to_string()),
            (
                "EncryptionKeyType",
                case.encryption_key_type.to_string(),
            ),
        ] {
            assert_eq!(
                string_at(&read, member),
                expected,
                "case {name:?}: expected {member} {expected:?}, got {read:?}"
            );
        }
        assert_eq!(
            read.get("EncryptionKeyArn").and_then(Value::as_str),
            case.vault_key.as_deref(),
            "case {name:?}: the point reports the key it is stored under, got {read:?}"
        );
        assert_eq!(
            read.get("IsEncrypted").and_then(Value::as_bool),
            Some(case.is_encrypted),
            "case {name:?}: expected IsEncrypted {}, got {read:?}",
            case.is_encrypted
        );
        assert_eq!(
            read.get("IsParent").and_then(Value::as_bool),
            Some(false),
            "case {name:?}: a point started directly is no parent, got {read:?}"
        );
        assert_eq!(
            read.get("BackupSizeInBytes").and_then(Value::as_i64),
            Some(0),
            "case {name:?}: the emulator copies no bytes, so the size is the stored 0, \
             got {read:?}"
        );
        let expected_lifecycle: Option<Value> = case
            .lifecycle
            .map(|body| serde_json::from_str(body).expect("a well-formed Lifecycle literal"));
        assert_eq!(
            read.get("Lifecycle").cloned(),
            expected_lifecycle,
            "case {name:?}: the stored Lifecycle must be reported verbatim, got {read:?}"
        );
        let calculated = read.get("CalculatedLifecycle");
        assert_eq!(
            calculated
                .and_then(|calculated| calculated.get("DeleteAt"))
                .is_some(),
            case.delete_at,
            "case {name:?}: expected DeleteAt present={}, got {read:?}",
            case.delete_at
        );
        assert_eq!(
            calculated
                .and_then(|calculated| calculated.get("MoveToColdStorageAt"))
                .is_some(),
            case.move_to_cold_storage_at,
            "case {name:?}: expected MoveToColdStorageAt present={}, got {read:?}",
            case.move_to_cold_storage_at
        );
        let creation = instant_at(&read, "CreationDate");
        let completion = instant_at(&read, "CompletionDate");
        assert!(
            creation.is_some(),
            "case {name:?}: a stored point reports its creation instant, got {read:?}"
        );
        assert!(
            completion > creation,
            "case {name:?}: the settled point completed after it was created, got {read:?}"
        );
        // Nothing in this slice copies a point from another Region or account,
        // and `SourceBackupVaultArn` is `null` exactly then (its model doc).
        assert!(
            read.get("SourceBackupVaultArn").is_none(),
            "case {name:?}: a point backed up into its own vault reports no source vault, \
             got {read:?}"
        );
        assert!(
            read.get("IndexStatus").is_none(),
            "case {name:?}: no index was asked for, so none is reported, got {read:?}"
        );
    }
}

/// A backup asked to build a search index reports that index's status once the
/// job that builds it has settled: the index is built by the same job, so a
/// settled backup has a queryable index.
#[tokio::test]
async fn an_indexed_recovery_point_reports_its_index_status() {
    let router = router();
    let (status, _) = create_vault(&router, "indexed-rp", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let payload = serde_json::json!({
        "BackupVaultName": "indexed-rp",
        "ResourceArn": "arn:aws:s3:::indexed-bucket",
        "IamRoleArn": backup_role(),
        "Index": "ENABLED",
    });
    let (status, started) = start_job(&router, &payload.to_string()).await;
    assert_eq!(
        status, 200,
        "setup: StartBackupJob with an index should succeed, got {status} {started:?}"
    );
    let arn = string_at(&started, "RecoveryPointArn");

    std::thread::sleep(PAST_SETTLE);
    let (status, active) = describe_point(&router, "indexed-rp", &arn, None).await;
    assert_eq!(
        (status, string_at(&active, "IndexStatus").as_str()),
        (200, "ACTIVE"),
        "once the backup has settled the index is queryable, got {status} {active:?}"
    );

    // The control: a backup that asked for no index reports none, so the member
    // follows the request rather than being filled in for every point.
    let unindexed = start_point(&router, "indexed-rp", "arn:aws:s3:::plain-bucket", None).await;
    std::thread::sleep(PAST_SETTLE);
    let (status, read) = describe_point(&router, "indexed-rp", &unindexed, None).await;
    assert_eq!(status, 200, "the second point must read back, got {read:?}");
    assert!(
        read.get("IndexStatus").is_none(),
        "a backup that asked for no index reports none, got {read:?}"
    );
}

/// A recovery point whose backup has not finished is not addressable yet: the
/// read finds nothing — the same condition, sentence and code a call that ACTS
/// on the point answers — and the same read reports the settled point once the
/// clock has made its completion due.
#[tokio::test]
async fn a_recovery_point_whose_backup_has_not_finished_is_not_found_yet() {
    let resources = [
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
        format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-01234567"),
    ];

    for resource in resources {
        let router = router();
        let (status, _) = create_vault(&router, "creating-rp", "{}").await;
        assert_eq!(
            status, 200,
            "case {resource:?}: setup CreateBackupVault should succeed"
        );
        let arn = start_point(&router, "creating-rp", &resource, None).await;

        // Straight after the start the backup is still running: the read finds
        // nothing, and no number of reads brings the point forward.
        for attempt in 0..3 {
            let (status, refused) = describe_point(&router, "creating-rp", &arn, None).await;
            assert_eq!(
                (
                    status,
                    error_code(&refused).as_str(),
                    string_at(&refused, "Message").as_str()
                ),
                (
                    400,
                    "ResourceNotFoundException",
                    "Cannot find recovery point"
                ),
                "case {resource:?} read {attempt}: a running backup has no addressable point, \
                 got {status} {refused:?}"
            );
        }

        // The same pair is just as unaddressable by a call that ACTS on the
        // point: one condition, one refusal.
        let (status, refused) = call(
            &router,
            "DELETE",
            &format!("/backup-vaults/creating-rp/recovery-points/{arn}"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFoundException"),
            "case {resource:?}: a running backup has no deletable point, got {status} {refused:?}"
        );
        // The point IS stored meanwhile — the vault's listing reports it — so
        // the refusal is about addressing it, not about it being absent.
        let listed = listed_points(&router, "creating-rp").await;
        assert_eq!(
            listed.len(),
            1,
            "case {resource:?}: the reserved point is still stored, got {listed:?}"
        );

        std::thread::sleep(PAST_SETTLE);
        let (status, settled) = describe_point(&router, "creating-rp", &arn, None).await;
        assert_eq!(
            (status, string_at(&settled, "Status").as_str()),
            (200, "COMPLETED"),
            "case {resource:?}: once the clock has made the completion due the read reports \
             it, got {status} {settled:?}"
        );
        assert!(
            instant_at(&settled, "CompletionDate") > instant_at(&settled, "CreationDate"),
            "case {resource:?}: the completion instant follows the creation one, got {settled:?}"
        );
    }
}

/// `StorageClass` follows the stored lifecycle: a point is in warm storage until
/// the cold-storage transition its lifecycle makes due has come, and in cold
/// storage from that instant.
///
/// The rows differ along the lifecycle form and along how much of it has elapsed,
/// including the exact instant the transition comes due and the day either side
/// of it, and a retention that asks for no transition at all.
#[tokio::test]
async fn storage_class_follows_the_cold_storage_transition() {
    const COLD_AND_DELETE: &str = r#"{"MoveToColdStorageAfterDays":30,"DeleteAfterDays":120}"#;
    // 90 days is the shortest stay the lifecycle doc admits in cold storage
    // ("Backups that are transitioned to cold storage must be stored in cold
    // storage for a minimum of 90 days"), so this row keeps the pair apart by it.
    const LATE_COLD: &str = r#"{"MoveToColdStorageAfterDays":90,"DeleteAfterDays":36500}"#;

    // (case, Lifecycle, days elapsed since the backup, expected StorageClass)
    let cases: Vec<(&str, Option<&str>, u64, &str)> = vec![
        ("no lifecycle at all", None, 0, "WARM"),
        ("no lifecycle, ten years on", None, 3_650, "WARM"),
        (
            "a retention that asks for no transition",
            Some(r#"{"DeleteAfterDays":35}"#),
            36,
            "WARM",
        ),
        (
            "the keep-indefinitely sentinel",
            Some(r#"{"DeleteAfterDays":-1}"#),
            365,
            "WARM",
        ),
        ("the day before the transition", Some(COLD_AND_DELETE), 29, "WARM"),
        ("the day the transition comes due", Some(COLD_AND_DELETE), 30, "COLD"),
        ("the day after the transition", Some(COLD_AND_DELETE), 31, "COLD"),
        ("a 90-day transition, a day short", Some(LATE_COLD), 89, "WARM"),
        ("a 90-day transition, a day past", Some(LATE_COLD), 91, "COLD"),
        (
            "a transition the day after the backup",
            Some(r#"{"MoveToColdStorageAfterDays":1,"DeleteAfterDays":91}"#),
            1,
            "COLD",
        ),
    ];

    let resource = "arn:aws:s3:::cold-bucket".to_string();
    for (case, lifecycle, elapsed_days, expected) in cases {
        let (router, engine) = router_with_engine();
        let (status, _) = create_vault(&router, "cold-rp", "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupVault should succeed"
        );
        let arn = start_point(&router, "cold-rp", &resource, lifecycle).await;
        std::thread::sleep(PAST_SETTLE);
        age_point(&engine, &arn, elapsed_days);

        let (status, read) = describe_point(&router, "cold-rp", &arn, None).await;
        assert_eq!(
            (status, string_at(&read, "StorageClass").as_str()),
            (200, expected),
            "case {case:?} ({elapsed_days} days on): expected StorageClass {expected:?}, \
             got {status} {read:?}"
        );
    }
}

/// One stored point, one projection: every member this read and the vault's
/// listing both report carries the same value for the same point.
#[tokio::test]
async fn describe_and_the_vault_listing_project_the_same_point() {
    let router = router();
    let (status, _) = create_vault(&router, "agree-rp", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let started: Vec<(&str, String)> = vec![
        (
            "no lifecycle",
            start_point(
                &router,
                "agree-rp",
                &format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
                None,
            )
            .await,
        ),
        (
            "a retention with a cold-storage transition",
            start_point(
                &router,
                "agree-rp",
                "arn:aws:s3:::agree-bucket",
                Some(r#"{"MoveToColdStorageAfterDays":30,"DeleteAfterDays":120}"#),
            )
            .await,
        ),
        (
            "a customer-named role and a long retention",
            start_point(
                &router,
                "agree-rp",
                &format!("arn:aws:fsx:{REGION}:{ACCOUNT}:file-system/fs-012345"),
                Some(r#"{"DeleteAfterDays":36500}"#),
            )
            .await,
        ),
    ];
    std::thread::sleep(PAST_SETTLE);

    let listed = listed_points(&router, "agree-rp").await;
    assert_eq!(
        listed.len(),
        started.len(),
        "the vault must list every point started in it, got {listed:?}"
    );

    for (case, arn) in &started {
        let entry = listed
            .iter()
            .find(|point| string_at(point, "RecoveryPointArn") == *arn)
            .unwrap_or_else(|| panic!("case {case:?}: the listing must hold {arn}"));
        let (status, read) = describe_point(&router, "agree-rp", arn, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeRecoveryPoint 200, got {status} {read:?}"
        );
        for member in SHARED_MEMBERS {
            assert_eq!(
                read.get(*member),
                entry.get(*member),
                "case {case:?}: the two reads must agree about {member}, \
                 got {:?} from the describe and {:?} from the listing",
                read.get(*member),
                entry.get(*member)
            );
        }
    }
}

/// Every pair that names no settled recovery point of a vault the caller can see
/// is ONE condition: 400 `ResourceNotFoundException` with the same sentence,
/// disclosing nothing about which half of the pair was wrong.
#[tokio::test]
async fn a_pair_that_names_no_stored_recovery_point_is_one_refusal() {
    let router = router();
    for vault in ["holder", "other"] {
        let (status, _) = create_vault(&router, vault, "{}").await;
        assert_eq!(
            status, 200,
            "setup {vault:?}: CreateBackupVault should succeed"
        );
    }
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = start_point(&router, "holder", &resource, Some(r#"{"DeleteAfterDays":7}"#)).await;
    let gone = start_point(&router, "other", &resource, None).await;
    std::thread::sleep(PAST_SETTLE);
    let (status, _) = call(
        &router,
        "DELETE",
        &format!("/backup-vaults/other/recovery-points/{gone}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "setup: the second point should be deletable");

    let fabricated = format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:does-not-exist");
    let other_partition = format!("arn:aws-cn:backup:{REGION}:{ACCOUNT}:recovery-point:1EB3B5E7");
    // An ARN of ANOTHER ACCOUNT is deliberately not a row here: the
    // authorization decision refuses it before the pair is resolved, which
    // `recovery_point_cross_account_authorization.rs` covers.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("an ARN nothing answers to", "holder", fabricated.as_str()),
        (
            "an ARN of another partition",
            "holder",
            other_partition.as_str(),
        ),
        ("an ARN another vault holds", "other", stored.as_str()),
        ("a point already deleted", "other", gone.as_str()),
    ];

    for (case, vault, arn) in cases {
        let (status, refused) = describe_point(&router, vault, arn, None).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (
                400,
                "ResourceNotFoundException",
                "Cannot find recovery point"
            ),
            "case {case:?} ({vault}, {arn}): expected the not-found refusal, got {status} {refused:?}"
        );
    }

    // The positive control: the stored point of the vault that holds it reads
    // back, so the refusals above are about the pair and not about the read.
    let (status, read) = describe_point(&router, "holder", &stored, None).await;
    assert_eq!(
        (status, string_at(&read, "RecoveryPointArn").as_str()),
        (200, stored.as_str()),
        "the stored pair must still read back, got {status} {read:?}"
    );
}

/// A `RecoveryPointArn` that is not an ARN is refused as a bad parameter value —
/// not reported as an absent resource.
#[tokio::test]
async fn a_recovery_point_arn_that_is_not_an_arn_is_a_bad_parameter_value() {
    // The form an ARN takes:
    // `arn:<partition>:<service>:<region>:<account>:<resource>`, as the model's
    // own doc for `RecoveryPointArn` spells it out
    // (smithy_query(query="operation", operation="DescribeRecoveryPoint") types
    // it as `ARN` with no pattern, so the service applies the rule).
    let cases: Vec<(&str, String)> = vec![
        ("no arn scheme", "not-an-arn".to_string()),
        ("too few fields", "arn:aws:backup".to_string()),
        (
            "an empty partition",
            format!("arn::backup:{REGION}:{ACCOUNT}:recovery-point:1EB3B5E7"),
        ),
        (
            "an empty service",
            format!("arn:aws::{REGION}:{ACCOUNT}:recovery-point:1EB3B5E7"),
        ),
        (
            "an empty resource",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:"),
        ),
    ];

    let router = router();
    let (status, _) = create_vault(&router, "bad-arn", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");

    for (case, value) in cases {
        let (status, refused) = describe_point(&router, "bad-arn", &value, None).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({value:?}): expected the parameter refusal, got {status} {refused:?}"
        );
    }
}

/// This read is vault-scoped, so it decides ACCESS before EXISTENCE: a vault the
/// account does not hold, or one addressed with another account's
/// `BackupVaultAccountId`, is refused 403 `AccessDeniedException` whatever the
/// `RecoveryPointArn` names — and the refusal discloses nothing.
///
/// The sibling suite `vault_read_visibility.rs` covers the same rule for the
/// vault reads whose path needs no stored resource; this case covers the read
/// whose path addresses one.
#[tokio::test]
async fn a_read_of_a_vault_outside_the_caller_s_visibility_is_denied() {
    let router = router();
    let (status, _) = create_vault(&router, "visible", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let (status, _) = create_vault(&router, "deleted", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let (status, _) = call(&router, "DELETE", "/backup-vaults/deleted", None).await;
    assert_eq!(status, 200, "setup: DeleteBackupVault should succeed");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = start_point(&router, "visible", &resource, None).await;
    std::thread::sleep(PAST_SETTLE);

    let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "a vault that was never created",
            "never-created",
            stored.as_str(),
            None,
        ),
        ("a vault that was deleted", "deleted", stored.as_str(), None),
        (
            "a stored point addressed through another account",
            "visible",
            stored.as_str(),
            Some("111122223333"),
        ),
        (
            "a stored point addressed through a different other account",
            "visible",
            stored.as_str(),
            Some("210987654321"),
        ),
    ];

    for (case, vault, arn, account) in cases {
        let (status, refused) = describe_point(&router, vault, arn, account).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (403, "AccessDeniedException", DENIED_MESSAGE),
            "case {case:?}: expected the authorization refusal, got {status} {refused:?}"
        );
        let rendered = refused.to_string();
        assert!(
            !rendered.contains(vault) && !rendered.contains("NotFound"),
            "case {case:?}: the refusal must disclose nothing about the vault, got {rendered}"
        );
    }

    // The positive control: the caller's own account, named or left out, reads
    // the point back.
    for account in [None, Some(ACCOUNT)] {
        let (status, read) = describe_point(&router, "visible", &stored, account).await;
        assert_eq!(
            (status, string_at(&read, "RecoveryPointArn").as_str()),
            (200, stored.as_str()),
            "a visible vault's point must read back for {account:?}, got {status} {read:?}"
        );
    }
}

/// A read changes nothing a later caller sees: every repeat answers identically,
/// the vault's listing and its `NumberOfRecoveryPoints` are untouched, and the
/// point is still there to be deleted afterwards.
#[tokio::test]
async fn reading_a_recovery_point_changes_nothing_a_later_caller_sees() {
    const READS: usize = 5;

    let router = router();
    let (status, _) = create_vault(&router, "polled-rp", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let arn = start_point(
        &router,
        "polled-rp",
        &resource,
        Some(r#"{"MoveToColdStorageAfterDays":30,"DeleteAfterDays":120}"#),
    )
    .await;
    std::thread::sleep(PAST_SETTLE);

    let (status, first) = describe_point(&router, "polled-rp", &arn, None).await;
    assert_eq!(status, 200, "the settled point must read back, got {first:?}");
    for attempt in 1..READS {
        let (status, again) = describe_point(&router, "polled-rp", &arn, None).await;
        assert_eq!(
            (status, &again),
            (200, &first),
            "read {attempt}: every read of a settled point must answer identically, \
             got {again:?} after {first:?}"
        );
    }

    let listed = listed_points(&router, "polled-rp").await;
    let arns: Vec<String> = listed
        .iter()
        .map(|point| string_at(point, "RecoveryPointArn"))
        .collect();
    assert_eq!(
        arns,
        vec![arn.clone()],
        "the reads must leave the vault's listing alone, got {listed:?}"
    );
    let (status, vault) = call(&router, "GET", "/backup-vaults/polled-rp", None).await;
    assert_eq!(
        (status, vault.get("NumberOfRecoveryPoints").cloned()),
        (200, Some(Value::from(1))),
        "the reads must leave the vault's count alone, got {status} {vault:?}"
    );

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/backup-vaults/polled-rp/recovery-points/{arn}"),
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&deleted).as_str()),
        (200, "<none>"),
        "the read point must still be deletable, got {status} {deleted:?}"
    );
    let (status, absent) = describe_point(&router, "polled-rp", &arn, None).await;
    assert_eq!(
        (status, error_code(&absent).as_str()),
        (400, "ResourceNotFoundException"),
        "once deleted the same read finds nothing, got {status} {absent:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for `CreateLogicallyAirGappedBackupVault` and the vault
//! lifecycle that follows it.
//!
//! Every case drives real HTTP through the router (the harness is the one
//! `vault_and_backup_job` uses), so what is asserted is the wire behaviour a
//! `backup` SDK client sees. The rules under test:
//!
//! * a create stores the resource it reports, so every later operation on that
//!   name resolves to the STORED vault — describe, list and delete all read it
//!   back, with the `VaultType`, lock flag and retention bounds the create
//!   fixed;
//! * a create whose output models a non-terminal `VaultState` reports
//!   `CREATING` first, and the clock alone moves the stored vault to
//!   `AVAILABLE`;
//! * an operation refused because of a stored resource's STATE answers the
//!   state-dependent modelled error — `InvalidRequestException` with AWS
//!   Backup's `ERROR_9100` for a delete of a vault still being created — never
//!   a refusal that says the resource is not there;
//! * a parameter value the service does not accept is refused with
//!   `InvalidParameterValueException` and stores nothing;
//! * a vault type whose recovery points are always encrypted reports the ARN of
//!   the key encrypting them — the caller's when the create named one,
//!   otherwise the AWS-owned key the service provisioned, stable across reads —
//!   and a vault locked at creation reports the instant that lock became
//!   immutable.

use std::thread::sleep;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{
    call, create_vault, error_code, name_of_length, router, start_job, string_at, ACCOUNT,
    PAST_SETTLE, REGION,
};

/// `VaultType` of a logically air-gapped vault (model enum `VaultType`:
/// `BACKUP_VAULT | LOGICALLY_AIR_GAPPED_BACKUP_VAULT |
/// RESTORE_ACCESS_BACKUP_VAULT`).
const AIR_GAPPED: &str = "LOGICALLY_AIR_GAPPED_BACKUP_VAULT";

/// The `Code` AWS Backup carries on the `InvalidRequestException` that refuses
/// a delete of a vault still in `Creating` state.
const ERROR_9100: &str = "ERROR_9100";

/// A customer-managed KMS key ARN, for the rows that name one.
const KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// One row of the create table: a vault name and the creation parameters.
#[derive(Debug, Clone)]
struct Case {
    name: String,
    min_retention_days: i64,
    max_retention_days: i64,
    encryption_key_arn: Option<&'static str>,
    tags: Option<Value>,
}

impl Case {
    fn body(&self) -> String {
        let mut body = json!({
            "MinRetentionDays": self.min_retention_days,
            "MaxRetentionDays": self.max_retention_days,
        });
        if let Some(key) = self.encryption_key_arn {
            body["EncryptionKeyArn"] = json!(key);
        }
        if let Some(tags) = &self.tags {
            body["BackupVaultTags"] = tags.clone();
        }
        body.to_string()
    }

    /// The `EncryptionKeyType` this row's vault reports: customer-managed
    /// exactly when the create named a key (model enum `EncryptionKeyType`).
    fn encryption_key_type(&self) -> &'static str {
        if self.encryption_key_arn.is_some() {
            "CUSTOMER_MANAGED_KMS_KEY"
        } else {
            "AWS_OWNED_KMS_KEY"
        }
    }
}

async fn create_air_gapped(router: &MultiServiceRouter, name: &str, body: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/logically-air-gapped-backup-vaults/{name}"),
        Some(body),
    )
    .await
}

/// The rows of the create table: they differ in name (both punctuation
/// characters the `BackupVaultName` pattern admits, and both published ends of
/// its `{2,50}` length bound), in the retention window (the documented floor,
/// an equal pair, a wide one), in whether a KMS key is named, and in whether
/// tags are sent.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "air-gapped-vault".to_string(),
            min_retention_days: 7,
            max_retention_days: 365,
            encryption_key_arn: None,
            tags: None,
        },
        Case {
            name: name_of_length(2),
            min_retention_days: 7,
            max_retention_days: 7,
            encryption_key_arn: None,
            tags: Some(json!({"Env": "test"})),
        },
        Case {
            name: name_of_length(50),
            min_retention_days: 30,
            max_retention_days: 36500,
            encryption_key_arn: Some(KEY_ARN),
            tags: None,
        },
        Case {
            name: "MiXeD_Air_09".to_string(),
            min_retention_days: 90,
            max_retention_days: 1000,
            encryption_key_arn: Some(KEY_ARN),
            tags: Some(json!({"Team": "platform", "Tier": "cold"})),
        },
    ]
}

/// The whole lifecycle: a create stores the vault, a describe reads it back,
/// the clock settles it from `CREATING` to `AVAILABLE`, a delete before that is
/// refused for the STATE, and a delete after it succeeds and makes the name
/// absent again.
#[tokio::test]
async fn an_air_gapped_vault_is_stored_settles_to_available_and_then_deletes() {
    for case in cases() {
        let name = case.name.as_str();
        let router = router();
        let (status, created) = create_air_gapped(&router, name, &case.body()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateLogicallyAirGappedBackupVault 200, got {status} {created:?}"
        );
        let expected_arn = format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{name}");
        assert_eq!(
            string_at(&created, "BackupVaultArn"),
            expected_arn,
            "case {case:?}: expected the synthesized vault ARN, got {created:?}"
        );
        assert_eq!(
            string_at(&created, "BackupVaultName"),
            name,
            "case {case:?}: expected the created name echoed from state, got {created:?}"
        );
        assert_eq!(
            created.get("VaultState").and_then(Value::as_str),
            Some("CREATING"),
            "case {case:?}: a create that models VaultState reports the non-terminal state first, got {created:?}"
        );
        assert!(
            created.get("CreationDate").is_some(),
            "case {case:?}: expected a CreationDate, got {created:?}"
        );

        // The vault is READABLE straight away — that is the whole point of the
        // create having stored it — and still in the transient state, because
        // the settle span is non-zero.
        let (status, described) = call(&router, "GET", &format!("/backup-vaults/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBackupVault 200 right after the create, got {status} {described:?}"
        );
        assert_eq!(
            described.get("VaultType").and_then(Value::as_str),
            Some(AIR_GAPPED),
            "case {case:?}: expected VaultType {AIR_GAPPED}, got {described:?}"
        );
        assert_eq!(
            described.get("VaultState").and_then(Value::as_str),
            Some("CREATING"),
            "case {case:?}: expected the stored transient VaultState, got {described:?}"
        );
        assert_eq!(
            described.get("Locked").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: an air-gapped vault is locked by construction, got {described:?}"
        );
        assert_eq!(
            described.get("MinRetentionDays").and_then(Value::as_i64),
            Some(case.min_retention_days),
            "case {case:?}: expected the stored MinRetentionDays, got {described:?}"
        );
        assert_eq!(
            described.get("MaxRetentionDays").and_then(Value::as_i64),
            Some(case.max_retention_days),
            "case {case:?}: expected the stored MaxRetentionDays, got {described:?}"
        );
        assert_eq!(
            described.get("NumberOfRecoveryPoints").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a fresh vault holds no recovery points, got {described:?}"
        );
        assert_eq!(
            described.get("EncryptionKeyType").and_then(Value::as_str),
            Some(case.encryption_key_type()),
            "case {case:?}: expected the key type the create's EncryptionKeyArn implies, got {described:?}"
        );
        // An air-gapped vault's recovery points are always encrypted, so the
        // vault reports a key either way: the one the create named, or the
        // AWS-owned key the service provisioned for it.
        let reported_key = string_at(&described, "EncryptionKeyArn");
        let provisioned_prefix = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/");
        match case.encryption_key_arn {
            Some(key) => assert_eq!(
                reported_key, key,
                "case {case:?}: expected the stored EncryptionKeyArn, got {described:?}"
            ),
            None => assert!(
                reported_key.len() > provisioned_prefix.len()
                    && reported_key.starts_with(&provisioned_prefix),
                "case {case:?}: expected a provisioned KMS key ARN, got {reported_key:?}"
            ),
        }
        assert_eq!(
            described.get("LockDate"),
            described.get("CreationDate"),
            "case {case:?}: an air-gapped vault's lock admits no changeable window, so it \
             became immutable when the vault was created, got {described:?}"
        );
        assert_eq!(
            string_at(&described, "BackupVaultArn"),
            expected_arn,
            "case {case:?}: describe must report the stored ARN, got {described:?}"
        );
        assert_eq!(
            described.get("CreationDate"),
            created.get("CreationDate"),
            "case {case:?}: describe must report the creation instant create returned, got {described:?}"
        );

        // A delete of a vault still being created is refused for its STATE:
        // the modelled InvalidRequestException and AWS Backup's own Code, not
        // a not-found.
        let (status, refused) = call(&router, "DELETE", &format!("/backup-vaults/{name}"), None).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: deleting a CREATING vault must 400 InvalidRequestException, got {status} {refused:?}"
        );
        assert_eq!(
            refused.get("Code").and_then(Value::as_str),
            Some(ERROR_9100),
            "case {case:?}: expected Code {ERROR_9100} on the refusal, got {refused:?}"
        );

        // Nothing but the clock moves the state.
        sleep(PAST_SETTLE);
        let (status, settled) = call(&router, "GET", &format!("/backup-vaults/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBackupVault 200 after the settle span, got {status} {settled:?}"
        );
        assert_eq!(
            settled.get("VaultState").and_then(Value::as_str),
            Some("AVAILABLE"),
            "case {case:?}: the clock owes the vault AVAILABLE, got {settled:?}"
        );
        assert_eq!(
            settled.get("CreationDate"),
            created.get("CreationDate"),
            "case {case:?}: settling must not restate the creation instant, got {settled:?}"
        );
        assert_eq!(
            string_at(&settled, "EncryptionKeyArn"),
            reported_key,
            "case {case:?}: the reported key must be stable across reads, got {settled:?}"
        );

        // The list reports the same stored vault, and the type filter resolves
        // it by the type the create stored.
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/backup-vaults?vaultType={AIR_GAPPED}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListBackupVaults 200, got {status} {listed:?}"
        );
        let entries = listed["BackupVaultList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected the air-gapped vault listed exactly once, got {entries:?}"
        );
        let entry = &entries[0];
        assert_eq!(
            string_at(entry, "EncryptionKeyArn"),
            reported_key,
            "case {case:?}: the list must report the key the describe reported, got {entry:?}"
        );
        assert_eq!(
            entry.get("LockDate"),
            settled.get("LockDate"),
            "case {case:?}: the list must report the lock date the describe reported, got {entry:?}"
        );
        assert_eq!(
            (
                string_at(entry, "BackupVaultName"),
                entry.get("VaultType").and_then(Value::as_str),
                entry.get("VaultState").and_then(Value::as_str),
                entry.get("Locked").and_then(Value::as_bool),
                entry.get("MinRetentionDays").and_then(Value::as_i64),
                entry.get("MaxRetentionDays").and_then(Value::as_i64),
            ),
            (
                name.to_string(),
                Some(AIR_GAPPED),
                Some("AVAILABLE"),
                Some(true),
                Some(case.min_retention_days),
                Some(case.max_retention_days),
            ),
            "case {case:?}: the list must agree with the describe, got {entry:?}"
        );

        let (status, deleted) = call(&router, "DELETE", &format!("/backup-vaults/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBackupVault 200 once the vault is available, got {status} {deleted:?}"
        );
        let (status, gone) = call(&router, "GET", &format!("/backup-vaults/{name}"), None).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: a deleted vault is outside the caller's visibility, so the \
             describe is denied rather than answered, got {status} {gone:?}"
        );
    }
}

/// A retention window the service does not accept is refused with
/// `InvalidParameterValueException`, and nothing is stored: the name stays
/// absent.
///
/// `MinRetentionDays`' floor of 7 is the model's own member documentation ("The
/// minimum value accepted is 7 days"); the rows walk that published bound and
/// one past it, and span a window whose maximum is below its minimum.
#[tokio::test]
async fn a_retention_window_the_service_refuses_stores_nothing() {
    // (case label, MinRetentionDays, MaxRetentionDays, accepted)
    let cases: Vec<(&str, i64, i64, bool)> = vec![
        ("at the documented floor", 7, 7, true),
        ("one past the floor", 8, 9, true),
        ("one below the floor", 6, 365, false),
        ("zero", 0, 365, false),
        ("negative", -1, 365, false),
        ("max one below min", 30, 29, false),
        ("max zero", 7, 0, false),
    ];

    for (label, min, max, accepted) in cases {
        let router = router();
        let name = "retention-window-vault";
        let body = json!({"MinRetentionDays": min, "MaxRetentionDays": max}).to_string();
        let (status, answered) = create_air_gapped(&router, name, &body).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?} ({min}..{max}): expected 200, got {status} {answered:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidParameterValueException"),
            "case {label:?} ({min}..{max}): expected 400 InvalidParameterValueException, got {status} {answered:?}"
        );
        let (status, gone) = call(&router, "GET", &format!("/backup-vaults/{name}"), None).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (403, "AccessDeniedException"),
            "case {label:?} ({min}..{max}): a refused create must store nothing, so the \
             name stays outside the caller's visibility, got {status} {gone:?}"
        );
    }
}

/// A vault name identifies one vault whatever its type: a repeat create is
/// answered from the STORED vault — a replay of the original request when it
/// carries the same `CreatorRequestId` and the same parameters,
/// `AlreadyExistsException` otherwise.
#[tokio::test]
async fn a_repeat_air_gapped_create_collides_unless_it_replays_the_original() {
    let name = "replayed-air-gapped";
    let original =
        json!({"MinRetentionDays": 7, "MaxRetentionDays": 365, "CreatorRequestId": "token-1"})
            .to_string();

    let router = router();
    let (status, created) = create_air_gapped(&router, name, &original).await;
    assert_eq!(
        status, 200,
        "case \"first create\": expected 200, got {status} {created:?}"
    );

    // The same token and the same parameters: the stored vault, not a second one.
    let (status, replayed) = create_air_gapped(&router, name, &original).await;
    assert_eq!(
        status, 200,
        "case \"replayed create\": expected 200, got {status} {replayed:?}"
    );
    assert_eq!(
        (
            string_at(&replayed, "BackupVaultArn"),
            replayed.get("CreationDate").cloned(),
        ),
        (
            string_at(&created, "BackupVaultArn"),
            created.get("CreationDate").cloned(),
        ),
        "case \"replayed create\": expected the stored vault reported again, got {replayed:?}"
    );

    // Every other repeat collides: a different retention window under the same
    // token, no token at all, and an ordinary create of the same name.
    let colliding: Vec<(&str, String)> = vec![
        (
            "same token, different window",
            json!({"MinRetentionDays": 7, "MaxRetentionDays": 366, "CreatorRequestId": "token-1"})
                .to_string(),
        ),
        (
            "different token",
            json!({"MinRetentionDays": 7, "MaxRetentionDays": 365, "CreatorRequestId": "token-2"})
                .to_string(),
        ),
        (
            "no token",
            json!({"MinRetentionDays": 7, "MaxRetentionDays": 365}).to_string(),
        ),
    ];
    for (label, body) in colliding {
        let (status, refused) = create_air_gapped(&router, name, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "AlreadyExistsException"),
            "case {label:?}: expected 400 AlreadyExistsException, got {status} {refused:?}"
        );
    }
    let (status, refused) = create_vault(&router, name, "{}").await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "AlreadyExistsException"),
        "case \"ordinary create of the same name\": expected 400 AlreadyExistsException, got {status} {refused:?}"
    );

    // And the stored vault is unchanged by every one of those refusals.
    let (status, described) = call(&router, "GET", &format!("/backup-vaults/{name}"), None).await;
    assert_eq!(
        (
            status,
            described.get("VaultType").and_then(Value::as_str),
            described.get("MaxRetentionDays").and_then(Value::as_i64),
            string_at(&described, "CreatorRequestId"),
        ),
        (200, Some(AIR_GAPPED), Some(365), "token-1".to_string()),
        "case \"after the collisions\": expected the original vault intact, got {status} {described:?}"
    );
}

/// A vault receives backups only once it is available: while the stored vault
/// is still `CREATING` a `StartBackupJob` naming it is refused for the STATE,
/// and the same request succeeds once the clock has settled the vault.
#[tokio::test]
async fn a_vault_still_creating_cannot_receive_a_backup() {
    let name = "job-destination-vault";
    let router = router();
    let (status, created) = create_air_gapped(
        &router,
        name,
        &json!({"MinRetentionDays": 7, "MaxRetentionDays": 365}).to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create\": expected 200, got {status} {created:?}"
    );

    let job = json!({
        "BackupVaultName": name,
        "ResourceArn": format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/orders"),
        "IamRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole"),
    })
    .to_string();

    let (status, refused) = start_job(&router, &job).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidRequestException"),
        "case \"job into a CREATING vault\": expected 400 InvalidRequestException, got {status} {refused:?}"
    );

    sleep(PAST_SETTLE);
    let (status, started) = start_job(&router, &job).await;
    assert_eq!(
        status, 200,
        "case \"job into an AVAILABLE vault\": expected 200, got {status} {started:?}"
    );
    let (status, described) = call(&router, "GET", &format!("/backup-vaults/{name}"), None).await;
    assert_eq!(
        (
            status,
            described.get("NumberOfRecoveryPoints").and_then(Value::as_i64),
        ),
        (200, Some(1)),
        "case \"after the accepted job\": expected one stored recovery point, got {status} {described:?}"
    );
}

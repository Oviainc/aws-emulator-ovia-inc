Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret rds__secretsmanager.refarch-aur-manual-rotate-succeeds-on-rds-managed-secret rds__secretsmanager.refarch-aur-master-secret-appears-only-when-management-is-turned-on rds__secretsmanager.master-password-rotation-invalidates-the-stale-secret-credential rds__secretsmanager.managed-password-rotation-leaves-the-open-session-and-its-transaction-intact rds__secretsmanager.switching-to-managed-master-password-rejects-the-previous-password rds__secretsmanager.refarch-aur-cluster-config-round-trips-production-settings rds__secretsmanager.refarch-aur-omitted-cluster-settings-return-service-defaults rds__secretsmanager.refarch-aur-never-rotated-secret-omits-every-rotation-field rds__secretsmanager.refarch-aur-serverless-scaling-max-below-min-rejected rds__secretsmanager.refarch-aur-zero-backup-retention-rejected-on-aurora-cluster
Issue-Key: rds__secretsmanager|CreateDBCluster|NotImplemented
Issue-Key: rds__secretsmanager|ModifyDBCluster|NotImplemented
Issue-Key: rds__secretsmanager.master-password-rotation-invalidates-the-stale-secret-credential|run#17|AssertionError|assert secret_arn, (
Issue-Key: rds__secretsmanager.managed-password-rotation-leaves-the-open-session-and-its-transaction-intact|_poll_until#12|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: rds__secretsmanager.switching-to-managed-master-password-rejects-the-previous-password|_my_first#12|Timeout|time.sleep(2)
Issue-Key: rds__secretsmanager.refarch-aur-cluster-config-round-trips-production-settings|run#24|KeyError|key_arn = cluster["KmsKeyId"]
Issue-Key: rds__secretsmanager.refarch-aur-omitted-cluster-settings-return-service-defaults|run#20|AssertionError|assert "PerformanceInsightsEnabled" not in cluster, (
Issue-Key: rds__secretsmanager.refarch-aur-never-rotated-secret-omits-every-rotation-field|run#19|AssertionError|assert present == [], (
Issue-Key: rds__secretsmanager.refarch-aur-serverless-scaling-max-below-min-rejected|_client_error#8|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it, but it returned {result!r}")
Issue-Key: rds__secretsmanager.refarch-aur-zero-backup-retention-rejected-on-aurora-cluster|run#11|AssertionError|assert message == expected_zero_message, (
Issue-Commit: 7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569

# rds__secretsmanager: 7 net-new root causes — Create/ModifyDBCluster refuse ManageMasterUserPassword with 501, CreateDBInstance mints no managed secret, DBCluster envelopes drop KmsKeyId and invent PerformanceInsightsEnabled, DescribeSecret invents RotationEnabled, and two cluster refusals are missing or misworded

> _Note: this issue was identified against the code base at commit `7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Seven distinct net-new root causes behind the unowned rds__secretsmanager failures (the two checks the report marks OWNED by 20261102T114105Z and 20261024T024248Z are skipped).

Cause 1 of 7: CreateDBCluster and ModifyDBCluster refuse the ManageMasterUserPassword member with HTTP 501 NotImplemented.
AWS: Smithy AmazonRDSv19 models CreateDBCluster$ManageMasterUserPassword and ModifyDBCluster$ManageMasterUserPassword (BooleanOptional) plus MasterUserSecretKmsKeyId, and DBCluster$MasterUserSecret: MasterUserSecret {SecretArn, SecretStatus (creating|active|rotating|impaired), KmsKeyId}. Both calls accept it: RDS generates/adopts the master password in Secrets Manager, reports the managed secret on the cluster, and that secret is rotatable through secretsmanager.RotateSecret. Turning the flag on by ModifyDBCluster makes MasterUserSecret appear where it was absent. Neither operation models a NotImplemented error.
Emulator: HTTP 501, code 'NotImplemented', with the emulator's own sentences "CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: …Name MasterUserPassword instead." and "ModifyDBCluster$ManageMasterUserPassword is not implemented in this emulator: …Name MasterUserPassword instead."
Why: requests AWS accepts get a 501; no Aurora cluster can use RDS-managed credentials, and the "secret appears only when management is turned on" contract is unobservable.
Fix: the CreateDBCluster and ModifyDBCluster handlers plus the RDS-to-Secrets-Manager managed-secret minting and the DBCluster MasterUserSecret projection.
Traces: rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret, rds__secretsmanager.refarch-aur-manual-rotate-succeeds-on-rds-managed-secret, rds__secretsmanager.refarch-aur-master-secret-appears-only-when-management-is-turned-on

Cause 2 of 7: CreateDBInstance with ManageMasterUserPassword=true returns no MasterUserSecret and mints no secret.
AWS: DBInstance$MasterUserSecret is reported with SecretArn/SecretStatus/KmsKeyId; SecretStatus advances to 'active', the stored document holds {"username","password"}, and after a rotation the previous password no longer authenticates while the new one does.
Emulator: the instance reports an empty/absent MasterUserSecret ({}), so the ARN that is the only way to reach the credential never exists and every wait-for-'active' loop times out. The same defect is filed for a different check as 20261024T030130Z-kms-rds-secretsmanager-rds-managemasteruserpassword-true-returns-no-masterusersecret-so-no.md, which does not carry the three checks below.
Why: managed master credentials are unusable; credential-rotation consequences (stale password refused, open session unaffected) cannot be reached.
Fix: the CreateDBInstance/ModifyDBInstance handlers and the DBInstance MasterUserSecret projection.
Traces: rds__secretsmanager.master-password-rotation-invalidates-the-stale-secret-credential, rds__secretsmanager.managed-password-rotation-leaves-the-open-session-and-its-transaction-intact, rds__secretsmanager.switching-to-managed-master-password-rejects-the-previous-password

Cause 3 of 7: DescribeDBClusters omits KmsKeyId for an encrypted cluster.
AWS: Smithy AmazonRDSv19 DBCluster$KmsKeyId — "If StorageEncrypted is enabled, the Amazon Web Services KMS key identifier for the encrypted DB cluster… the key ARN, key ID, alias ARN, or alias name for the KMS key." A cluster created with StorageEncrypted=true and no explicit key resolves to the account's default RDS key and reports its identifier, which must name a real, enabled KMS key.
Emulator: the DBCluster envelope has no KmsKeyId member although StorageEncrypted came back true, so reading cluster["KmsKeyId"] raises KeyError.
Why: a client cannot discover which key protects the cluster, so it cannot grant, audit or verify encryption at all.
Fix: the DBCluster projection in the RDS state module — resolve and emit KmsKeyId whenever StorageEncrypted is true.
Traces: rds__secretsmanager.refarch-aur-cluster-config-round-trips-production-settings

Cause 4 of 7: DescribeDBClusters emits PerformanceInsightsEnabled for a cluster that never had Performance Insights configured.
AWS: DBCluster$PerformanceInsightsEnabled is a BooleanOptional with no default ("Indicates whether Performance Insights is enabled for the DB cluster"); for an Aurora Serverless v2 cluster created without any Performance Insights setting the member is absent from the response, not false (recorded behavior only — the check grades its absence).
Emulator: the member is present with value false.
Why: a client that distinguishes "not configured" from "explicitly disabled" — a round-trip/drift comparison of declared versus returned settings — sees a setting the request never made, and anything that treats presence as configuration mis-reports the cluster.
Fix: the DBCluster projection must leave unset optional members out of the envelope instead of defaulting them.
Traces: rds__secretsmanager.refarch-aur-omitted-cluster-settings-return-service-defaults

Cause 5 of 7: DescribeSecret reports RotationEnabled on a secret that never had rotation configured.
AWS: Smithy secretsmanager DescribeSecretResponse$RotationEnabled is an optional boolean with NO default, alongside RotationLambdaARN/RotationRules/LastRotatedDate/NextRotationDate; for a hand-created secret that has never had rotation configured (even after a PutSecretValue) AWS omits every rotation field rather than returning false/null.
Emulator: the response carries RotationEnabled: false.
Why: a client that tests presence to decide whether rotation was ever configured concludes "configured and disabled" for a secret that was never touched by rotation, which changes remediation and drift decisions.
Fix: the DescribeSecret serializer / secret state projection — omit rotation members until rotation is configured.
Traces: rds__secretsmanager.refarch-aur-never-rotated-secret-omits-every-rotation-field

Cause 6 of 7: ModifyDBCluster accepts a Serverless v2 MaxCapacity below MinCapacity.
AWS: ServerlessV2ScalingConfiguration {MinCapacity, MaxCapacity} must satisfy Min <= Max; a modify that inverts them is refused with code InvalidParameterValue and the fixed message "Serverless v2 minimum capacity must be less than or equal to maximum capacity", the live ACU range is left untouched and no PendingModifiedValues entry is queued; the same call with the pair the right way round applies.
Emulator: the call returns HTTP 200 with the full DBCluster envelope, so the invalid range is accepted.
Why: a scaling range AWS rejects is silently taken, so a cluster configured through this emulator cannot be configured the same way in AWS, and the negative control (range unchanged, nothing pending) has nothing to assert.
Fix: the ModifyDBCluster handler's ServerlessV2ScalingConfiguration validation, before any state mutation.
Traces: rds__secretsmanager.refarch-aur-serverless-scaling-max-below-min-rejected

Cause 7 of 7: the BackupRetentionPeriod=0 refusal on an Aurora cluster is worded by the emulator, not by AWS.
AWS: refuses the out-of-range value with the fixed sentence "Invalid backup retention period: <value>. Retention period must be between 1 and 35." — the two sentences are fixed text, the value is taken from the request and 1/35 are the documented bounds.
Emulator: "invalid value 0 for BackupRetentionPeriod: must be a value from 1 to 35" — a generic constraint sentence that matches neither AWS's wording nor its sentence structure.
Why: a client matching on the refusal text (log assertions, retry/branch logic, user-facing error mapping) does not recognise the error.
Fix: the BackupRetentionPeriod validation in the Create/ModifyDBCluster path must emit AWS's sentence.
Traces: rds__secretsmanager.refarch-aur-zero-backup-retention-rejected-on-aurora-cluster

## Evidence
All quoted from the behavioral replay report (suite simparity-behavioral-aws-v1, service group rds__secretsmanager 1/15 passed).

Cause 1 — [error] refarch-aur-managed-master-password-mints-rotating-rds-owned-secret and [error] refarch-aur-manual-rotate-succeeds-on-rds-managed-secret:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDBCluster operation: CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: … Name MasterUserPassword instead.
[error] refarch-aur-master-secret-appears-only-when-management-is-turned-on:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ModifyDBCluster operation: ModifyDBCluster$ManageMasterUserPassword is not implemented in this emulator: … Name MasterUserPassword instead.

Cause 2 — [mismatch] master-password-rotation-invalidates-the-stale-secret-credential:
AssertionError: ManageMasterUserPassword=True must produce a MasterUserSecret with a SecretArn; the instance reports {}
[error] managed-password-rotation-leaves-the-open-session-and-its-transaction-intact:
TimeoutError: db instance sim2real-behav-inst-e2897a6b221e reaching 'available' with an active managed MasterUserSecret: the run budget ended first; not graded
[mismatch] switching-to-managed-master-password-rejects-the-previous-password: Check timed out after 60.0s

Cause 3 — [error] refarch-aur-cluster-config-round-trips-production-settings:
key_arn = cluster["KmsKeyId"] -> KeyError: 'KmsKeyId'  (the cluster was created with StorageEncrypted=True and DescribeDBClusters returned StorageEncrypted true)

Cause 4 — [mismatch] refarch-aur-omitted-cluster-settings-return-service-defaults:
AssertionError: PerformanceInsightsEnabled is an instance-level field and must be absent from a cluster description; got False

Cause 5 — [mismatch] refarch-aur-never-rotated-secret-omits-every-rotation-field:
AssertionError: a hand-passed cluster secret that has never had rotation configured must omit every rotation field; DescribeSecret returned ['RotationEnabled'] with values {'RotationEnabled': False}

Cause 6 — [mismatch] refarch-aur-serverless-scaling-max-below-min-rejected:
AssertionError: the call succeeded; real AWS rejects it, but it returned {'DBCluster': {'DBClusterIdentifier': 'sim2real-aur-acd3556b25eb', 'Status': 'available', 'ServerlessV2ScalingConfiguration': …, …}}
The check sends ModifyDBCluster(ServerlessV2ScalingConfiguration={'MinCapacity': 4.0, 'MaxCapacity': 1.0}) on a 0.5-1.0 ACU cluster and asserts Code == 'InvalidParameterValue' and Message == 'Serverless v2 minimum capacity must be less than or equal to maximum capacity', then that the live range still reads {'MinCapacity': 0.5, 'MaxCapacity': 1.0} with no PendingModifiedValues.

Cause 7 — [mismatch] refarch-aur-zero-backup-retention-rejected-on-aurora-cluster:
AssertionError: expected message 'Invalid backup retention period: 0. Retention period must be between 1 and 35.', got 'invalid value 0 for BackupRetentionPeriod: must be a value from 1 to 35'

## Steps to Reproduce
Build and boot the emulator, then replay the checks in trace_ids. Manually:
1. rds.create_db_cluster(Engine='aurora-postgresql', MasterUsername=…, ManageMasterUserPassword=True) -> expect 200 with MasterUserSecret; observed 501 NotImplemented. Same for rds.modify_db_cluster(ManageMasterUserPassword=True) on an existing cluster.
2. rds.create_db_instance(ManageMasterUserPassword=True) -> expect MasterUserSecret {SecretArn, SecretStatus, KmsKeyId} and SecretStatus reaching 'active'; observed {}.
3. rds.create_db_cluster(..., StorageEncrypted=True) then describe_db_clusters -> expect KmsKeyId naming an enabled KMS key; observed the member is absent.
4. Create an Aurora Serverless v2 cluster with no Performance Insights setting and describe it -> expect no PerformanceInsightsEnabled member; observed False.
5. secretsmanager.create_secret(...) (no rotation ever configured), then describe_secret -> expect no RotationEnabled/RotationLambdaARN/RotationRules/LastRotatedDate/NextRotationDate members; observed RotationEnabled: false.
6. On a cluster with ServerlessV2ScalingConfiguration {0.5, 1.0}, modify_db_cluster with {MinCapacity: 4.0, MaxCapacity: 1.0} -> expect InvalidParameterValue 'Serverless v2 minimum capacity must be less than or equal to maximum capacity' and an unchanged range; observed 200.
7. create/modify an Aurora cluster with BackupRetentionPeriod=0 -> expect 'Invalid backup retention period: 0. Retention period must be between 1 and 35.'; observed 'invalid value 0 for BackupRetentionPeriod: must be a value from 1 to 35'.

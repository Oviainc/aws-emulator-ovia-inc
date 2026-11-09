Issue-Source: trace-sweep-divergence
Issue-Trace: backup-on-demand-dynamodb-backup-job-lands-recovery-point
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-on-demand-dynamodb-backup-job-lands-recovery-point
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.BackupOptions
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.CompletionDate
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.CreationDate
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.ExpectedCompletionDate
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.InitiationDate
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.MessageCategory
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.RecoveryPointLifecycle
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.ResourceName
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.StartBy
Issue-Key: backup|DescribeBackupJob|2xx->2xx|$.VaultType
Issue-Key: backup|StartBackupJob|2xx->2xx|$.CreationDate
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].CalculatedLifecycle
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].CompletionDate
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].CreationDate
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].EncryptionKeyArn
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].EncryptionKeyType
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].InitiationDate
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].IsEncrypted
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].Lifecycle
Issue-Key: backup|ListRecoveryPointsByBackupVault|2xx->2xx|$.RecoveryPoints[*].ResourceName
Issue-Commit: d38d8ab45cd7905c8c6149b13ae22553e33f698d

# AWS Backup job and recovery-point read bodies diverge: members omitted, IsEncrypted/EncryptionKeyArn wrong, sub-millisecond timestamps (3 causes)

> _Note: this issue was identified against the code base at commit `d38d8ab45cd7905c8c6149b13ae22553e33f698d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackupJob

## Description
The AWS Backup emulator's on-demand backup-job read path answers 200 but with bodies that do not match the service: the job and recovery-point projections omit modelled members that AWS always populates for a completed on-demand job, report members AWS leaves unset, mis-report encryption, and serialise epoch timestamps at microsecond precision. Three distinct causes, all in the backup crate's backup-job / recovery-point projection and timestamp serialisation. No existing open issue covers the backup-job or recovery-point read bodies: the nearest ones (20261029T193050Z-backup-aws-backup-plan-state-and-response-bodies-diverge.md and 20261103T061309Z-backup-aws-backup-plan-and-selection-read-bodies-diverge.md) are about plan/selection bodies, and 20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md is about job state transitions and eventing, not the response projection.

Cause 1 of 3: StartBackupJob serialises CreationDate at sub-millisecond precision (fixture backup-on-demand-dynamodb-backup-job-lands-recovery-point, step task#7).
AWS: Smithy StartBackupJob.CreationDate targets com.amazonaws.backup#timestamp (restJson1 epoch-seconds). The model documents the service contract for these epoch-second members explicitly - RecoveryPointByBackupVault.CreationDate/CompletionDate/LastRestoreTime: "The value of CreationDate is accurate to milliseconds. For example, the value 1516925490.087 represents Friday, January 26, 2018 12:11:30.087 AM." Recorded AWS responses across this corpus agree: epoch seconds carry at most three fractional digits (e.g. 1791547415.87, 1789719141.149).
Emulator: emits a float with six fractional digits, so the member fails the recorded shape assertion for an epoch-second timestamp ($.CreationDate: value_mismatch, expected '<shaped-not-asserted>', got a 6-decimal epoch value).
Why: the requirement being violated is the precision of the serialised form, not the instant. A client that round-trips or formats the value, or asserts the documented millisecond accuracy, sees a timestamp AWS never emits; SDK/consumer code that re-serialises to the documented 3-decimal form loses information or mismatches on comparison.
Fix: the backup crate's timestamp serializer for epoch-second members (StartBackupJob/DescribeBackupJob/recovery-point projections) must truncate to millisecond precision.
Traces: backup-on-demand-dynamodb-backup-job-lands-recovery-point#task#7 (StartBackupJob).

Cause 2 of 3: DescribeBackupJob omits five modelled output members and populates three AWS leaves unset (fixture backup-on-demand-dynamodb-backup-job-lands-recovery-point, step task#8).
AWS: Smithy DescribeBackupJobOutput models InitiationDate (timestamp), StartBy (timestamp), ResourceName (string), MessageCategory (string) and RecoveryPointLifecycle (Lifecycle). For a completed on-demand DynamoDB job the recording shows all five present: InitiationDate and StartBy as epoch-second timestamps, ResourceName as the protected resource's non-unique name taken from the request's ResourceArn, MessageCategory as the fixed token 'SUCCESS' for a job that completed without a message, and RecoveryPointLifecycle as a fully-materialised Lifecycle structure whose four modelled members (DeleteAfterDays, DeleteAfterEvent, MoveToColdStorageAfterDays, OptInToArchiveForSupportedResources) are present and null when StartBackupJob carried no Lifecycle. The same recording shows AWS does NOT emit BackupOptions, ExpectedCompletionDate or VaultType for this job.
Emulator: all five of InitiationDate, StartBy, ResourceName, MessageCategory and RecoveryPointLifecycle are absent from the body, while BackupOptions is emitted as an empty map {}, ExpectedCompletionDate is emitted as an epoch timestamp and VaultType is emitted as 'BACKUP_VAULT' - three members AWS omits here.
Why: a client that reads MessageCategory to classify job outcome, ResourceName to label the protected resource, or InitiationDate/StartBy to reason about the backup window gets null and either misreports or throws on the missing field; a client that treats a present BackupOptions/VaultType as "the service supplied options/vault type" branches on emulator-invented data. The values themselves (the resource name, the instants) come from the request and state, not from the recording.
Fix: the DescribeBackupJob handler's output projection in the backup crate - populate the five members from the stored job record (ResourceName derived from ResourceArn, MessageCategory from terminal job state, RecoveryPointLifecycle materialised from the job's lifecycle with explicit nulls) and stop emitting BackupOptions/ExpectedCompletionDate/VaultType when the job carries none.
Traces: backup-on-demand-dynamodb-backup-job-lands-recovery-point#task#8 (DescribeBackupJob).

Cause 3 of 3: ListRecoveryPointsByBackupVault reports an unencrypted recovery point with no key ARN and omits lifecycle/initiation/resource-name members (fixture backup-on-demand-dynamodb-backup-job-lands-recovery-point, step task#9).
AWS: Smithy RecoveryPointByBackupVault models EncryptionKeyArn (ARN), IsEncrypted (Boolean2, default false), CalculatedLifecycle (CalculatedLifecycle{DeleteAt, MoveToColdStorageAt}), Lifecycle (Lifecycle), InitiationDate (timestamp) and ResourceName (string). The recording shows that a recovery point landed in a backup vault is encrypted with the vault's encryption key: IsEncrypted is true and EncryptionKeyArn is the vault's KMS key ARN (an arn:aws:kms:<region>:<account>:key/<key-id>; the specific key id in the recording is environment-specific and is not the requirement). It also shows CalculatedLifecycle and Lifecycle materialised as structures with their modelled members present-and-null when no lifecycle was requested, InitiationDate present as an epoch-second timestamp, and ResourceName present as the protected resource's name; and it shows EncryptionKeyType NOT emitted for this recovery point.
Emulator: IsEncrypted is false and EncryptionKeyArn is absent, so the recovery point reads as unencrypted; CalculatedLifecycle, Lifecycle, InitiationDate and ResourceName are all absent; EncryptionKeyType is emitted as 'AWS_OWNED_KMS_KEY' where AWS emits nothing; and CreationDate/CompletionDate carry the same sub-millisecond precision as Cause 1.
Why: a client auditing encryption posture concludes the vault's recovery points are unencrypted and has no key ARN to resolve against KMS - the exact inverse of the service's behaviour, which is a compliance-relevant wrong answer, not a cosmetic gap. A client reading Lifecycle/CalculatedLifecycle to compute retention or DeleteAt gets null and cannot plan expiry.
Fix: the recovery-point projection used by ListRecoveryPointsByBackupVault in the backup crate - carry the vault's encryption key onto the recovery point at creation (setting IsEncrypted true and EncryptionKeyArn to the vault's key ARN), materialise Lifecycle/CalculatedLifecycle, and populate InitiationDate and ResourceName from the originating backup job.
Traces: backup-on-demand-dynamodb-backup-job-lands-recovery-point#task#9 (ListRecoveryPointsByBackupVault).

## Evidence
Replay of trace set simparity-wire-aws-v1 (92 fixtures, 583 steps, score 47.3%). All three clusters come from fixture backup-on-demand-dynamodb-backup-job-lands-recovery-point and none carries a stub_path or owned_by.

Cause 1 - backup-on-demand-dynamodb-backup-job-lands-recovery-point#task#7, StartBackupJob, expected 200 / got 200:
  $.CreationDate: value_mismatch (expected '<shaped-not-asserted>' epoch-second timestamp, got 1794240816.110162 - six fractional digits)
  Model contract: "The value of CreationDate is accurate to milliseconds. For example, the value 1516925490.087..."

Cause 2 - backup-on-demand-dynamodb-backup-job-lands-recovery-point#task#8, DescribeBackupJob, expected 200 / got 200:
  $.InitiationDate: missing (expected an epoch-second timestamp, got None)
  $.StartBy: missing (expected an epoch-second timestamp, got None)
  $.ResourceName: missing (expected the protected resource's name, got None)
  $.MessageCategory: missing (expected 'SUCCESS', got None)
  $.RecoveryPointLifecycle: missing (expected {'DeleteAfterDays': None, 'DeleteAfterEvent': None, 'MoveToColdStorageAfterDays': None, 'OptInToArchiveForSupportedResources': None}, got None)
  $.BackupOptions: type_mismatch (expected absent, got {})
  $.ExpectedCompletionDate: type_mismatch (expected absent, got an epoch timestamp)
  $.VaultType: type_mismatch (expected absent, got 'BACKUP_VAULT')
  $.CreationDate / $.CompletionDate: value_mismatch (sub-millisecond precision, same defect as Cause 1)

Cause 3 - backup-on-demand-dynamodb-backup-job-lands-recovery-point#task#9, ListRecoveryPointsByBackupVault, expected 200 / got 200:
  $.RecoveryPoints[0].IsEncrypted: value_mismatch (expected True, got False)
  $.RecoveryPoints[0].EncryptionKeyArn: missing (expected the vault's arn:aws:kms:<region>:<account>:key/<key-id>, got None)
  $.RecoveryPoints[0].CalculatedLifecycle: missing (expected {'DeleteAt': None, 'MoveToColdStorageAt': None}, got None)
  $.RecoveryPoints[0].Lifecycle: missing (expected {'DeleteAfterDays': None, 'DeleteAfterEvent': None, 'MoveToColdStorageAfterDays': None, 'OptInToArchiveForSupportedResources': None}, got None)
  $.RecoveryPoints[0].InitiationDate: missing (expected an epoch-second timestamp, got None)
  $.RecoveryPoints[0].ResourceName: missing (expected the protected resource's name, got None)
  $.RecoveryPoints[0].EncryptionKeyType: type_mismatch (expected absent, got 'AWS_OWNED_KMS_KEY')
  $.RecoveryPoints[0].CreationDate / CompletionDate: value_mismatch (sub-millisecond precision, same defect as Cause 1)

Smithy model consulted: services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json - DescribeBackupJob (output members InitiationDate, StartBy, ResourceName, MessageCategory, RecoveryPointLifecycle, BackupOptions, ExpectedCompletionDate, VaultType), StartBackupJob (output CreationDate: timestamp), ListRecoveryPointsByBackupVault (output RecoveryPoints: RecoveryPointByBackupVaultList), and shapes RecoveryPointByBackupVault, Lifecycle, CalculatedLifecycle.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1); or drive fixture backup-on-demand-dynamodb-backup-job-lands-recovery-point on its own.
3. Cause 1 (step task#7): CreateBackupVault, create a DynamoDB table, then StartBackupJob against that table's ARN into the vault. Read $.CreationDate in the 200 response - it carries six fractional digits instead of the documented millisecond precision.
4. Cause 2 (step task#8): DescribeBackupJob with the BackupJobId returned above, after the job settles. The 200 body omits InitiationDate, StartBy, ResourceName, MessageCategory and RecoveryPointLifecycle, and additionally carries BackupOptions {}, ExpectedCompletionDate and VaultType, which AWS does not emit for this job.
5. Cause 3 (step task#9): ListRecoveryPointsByBackupVault on the same vault. The single recovery point reports IsEncrypted false with no EncryptionKeyArn, and omits CalculatedLifecycle, Lifecycle, InitiationDate and ResourceName while emitting EncryptionKeyType.

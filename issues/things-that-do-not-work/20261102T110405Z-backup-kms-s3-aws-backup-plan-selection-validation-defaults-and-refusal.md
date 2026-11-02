Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__kms__s3.refarch-bkp-cold-storage-within-90-days-of-delete-rejected backup__kms__s3.refarch-bkp-conditions-selection-round-trips-alongside-resources backup__kms__s3.refarch-bkp-copy-action-destination-vault-not-validated-at-create backup__kms__s3.refarch-bkp-plan-deletion-rejected-while-selection-exists backup__kms__s3.refarch-bkp-plan-rule-defaults-windows-and-omits-cold-storage backup__kms__s3.refarch-bkp-selection-with-nonexistent-role-rejected backup__kms__s3.refarch-bkp-vault-defaults-to-aws-managed-backup-key backup__kms__s3.refarch-bkp-vault-with-a-customer-key-reports-and-resolves-that-key backup__kms__s3.refarch-bkp-copy-action-to-vault-in-another-region-round-trips backup__kms__s3.refarch-bkp-object-lock-bucket-refuses-to-suspend-versioning
Issue-Key: backup__kms__s3.refarch-bkp-cold-storage-within-90-days-of-delete-rejected|_error_code_and_message#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: backup__kms__s3.refarch-bkp-conditions-selection-round-trips-alongside-resources|run#17|AssertionError|assert sel.get("Conditions") == {
Issue-Key: backup__kms__s3.refarch-bkp-copy-action-destination-vault-not-validated-at-create|run#22|AssertionError|assert bad == ("InvalidParameterValueException", "Invalid backup vault ARN"), (
Issue-Key: backup__kms__s3.refarch-bkp-plan-deletion-rejected-while-selection-exists|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: backup__kms__s3.refarch-bkp-plan-rule-defaults-windows-and-omits-cold-storage|run#19|AssertionError|assert partial.get("ScheduleExpressionTimezone") == "Etc/UTC", (
Issue-Key: backup__kms__s3.refarch-bkp-selection-with-nonexistent-role-rejected|_error_code_and_message#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: backup__kms__s3|DescribeKey|NotFoundException
Issue-Key: backup__kms__s3.refarch-bkp-object-lock-bucket-refuses-to-suspend-versioning|_error#13|AssertionError|raise AssertionError("the call succeeded where real AWS refuses it")
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# AWS Backup plan/selection validation, defaults and refusal messages diverge, and a vault's default KMS key is unknown to KMS; S3 lets an Object Lock bucket suspend versioning (8 causes)

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackupPlan

## Description
Eight distinct root causes, all reached from the backup__kms__s3 reference-architecture checks. Each is numbered with its own failing check ids; all eight must be closed before this issue is resolved.

=== Cause 1 of 8: CreateBackupPlan accepts a Lifecycle whose DeleteAfterDays is less than 90 days after MoveToColdStorageAfterDays ===
Failing check ids: backup__kms__s3.refarch-bkp-cold-storage-within-90-days-of-delete-rejected
AWS: Smithy backup#Lifecycle member DeleteAfterDays documents the rule as a service constraint — "This value must be at least 90 days after the number of days specified in MoveToColdStorageAfterDays" — and the Lifecycle shape doc repeats "Backups transitioned to cold storage must be stored in cold storage for a minimum of 90 days". CreateBackupPlan therefore refuses such a plan with the modeled client error InvalidParameterValueException (Smithy: backup#CreateBackupPlan errors, errorKind client, httpError 400), message "Error in some rules due to : Invalid lifecycle. DeleteAfterDays cannot be less than 90 days apart from MoveToColdStorageAfterDays" — all fixed text for this class (it names no rule, plan or number from the request) — and creates nothing: ListBackupPlans never lists the rejected plan name.
Emulator: CreateBackupPlan with Rules[0].Lifecycle = {MoveToColdStorageAfterDays: 30, DeleteAfterDays: 60} (a 30-day interval) SUCCEEDS; no refusal of any kind is produced.
Why: a client that relies on the service to validate retention gets a plan AWS would never have created; backups then cannot transition as declared, and the error path IaC/CI relies on (reject-and-create-nothing) is untestable against the emulator.
Fix: CreateBackupPlan input validation in the backup crate (the create_backup_plan handler's rule/lifecycle validation), shared with UpdateBackupPlan so both reject the same interval.

=== Cause 2 of 8: GetBackupSelection omits the Conditions operator lists the caller did not send ===
Failing check ids: backup__kms__s3.refarch-bkp-conditions-selection-round-trips-alongside-resources
AWS: Smithy backup#Conditions is a structure with four list members StringEquals / StringNotEquals / StringLike / StringNotLike (target ConditionParameters). A selection created with only StringEquals reads back with ALL FOUR members present, the declared one carrying its condition value-for-value and the other three as empty lists.
Emulator: GetBackupSelection returns Conditions = {'StringEquals': [{'ConditionKey': ..., 'ConditionValue': ...}]} only — StringNotEquals, StringLike and StringNotLike are absent rather than empty lists.
Why: a client that reads sel['Conditions']['StringLike'] to inspect or edit a selection raises KeyError / NPE on a selection AWS serves completely; read-modify-write of a selection loses the distinction between "no conditions of that operator" and "member unknown".
Fix: the GetBackupSelection (and ListBackupSelections / plan read) serializer for the Conditions structure in the backup crate — emit all four operator lists, defaulting unset ones to [].

=== Cause 3 of 8: a malformed DestinationBackupVaultArn is refused with the emulator's own sentence ===
Failing check ids: backup__kms__s3.refarch-bkp-copy-action-destination-vault-not-validated-at-create
AWS: CopyAction.DestinationBackupVaultArn is required and targets ARN (Smithy: backup#CopyAction). A CopyAction whose destination ARN is malformed is refused InvalidParameterValueException with the message "Invalid backup vault ARN" — fixed text, no value interpolated from the request.
Emulator: the same request is refused with the right code but a different sentence: "Parameter DestinationBackupVaultArn is invalid."
Why: the code matches but the message does not; a client or operator matching on AWS's wording (or showing it to a user) sees text AWS never emits, and the generic "Parameter X is invalid." template hides which rule was broken.
Fix: the CopyAction/ARN validation message in the backup crate's create_backup_plan (and update_backup_plan) validation path.

=== Cause 4 of 8: DeleteBackupPlan's "selection still exists" refusal uses an emulator-authored message ===
Failing check ids: backup__kms__s3.refarch-bkp-plan-deletion-rejected-while-selection-exists
AWS: DeleteBackupPlan on a plan that still carries a selection is refused with the modeled client error InvalidRequestException (Smithy: backup#DeleteBackupPlan errors, errorKind client, httpError 400), message "Related backup plan selections must be deleted prior to backup plan deletion" — wholly fixed text: it names neither the plan id nor the number of selections.
Emulator: the code is right (InvalidRequestException) but the message is the crate's own prose with request/state values spliced in: "Backup plan <plan id> still holds <n> resource selection(s); all associated selections of resources must be deleted before the backup plan can be deleted".
Why: a client matching AWS's fixed sentence does not recognize the refusal, and the emulator's message implies the sentence carries a plan id and a count that AWS's does not.
Fix: the refusal message in the backup crate's delete_backup_plan handler.

=== Cause 5 of 8: a rule with no ScheduleExpressionTimezone is not defaulted to Etc/UTC ===
Failing check ids: backup__kms__s3.refarch-bkp-plan-rule-defaults-windows-and-omits-cold-storage
AWS: Smithy backup#BackupRule member ScheduleExpressionTimezone (target Timezone) documents "By default, ScheduleExpressions are in UTC"; a rule created without it reads back from GetBackupPlan with ScheduleExpressionTimezone = "Etc/UTC" (server-defaulted and PRESENT, alongside the StartWindowMinutes=480 / CompletionWindowMinutes=10080 defaults the emulator does supply), while a sibling rule that declared "America/New_York" keeps its own value.
Emulator: the rule that omitted the member reads back with ScheduleExpressionTimezone absent (None).
Why: a client reading the rule cannot tell which timezone the schedule runs in; code that compares or re-submits the rule (read-modify-write) sees an unset member and either crashes or silently re-creates the rule with no timezone.
Fix: rule normalization on CreateBackupPlan/UpdateBackupPlan in the backup crate (where StartWindowMinutes/CompletionWindowMinutes are already defaulted) plus the GetBackupPlan serializer, so the defaulted timezone is stored and returned for both unversioned and VersionId-pinned reads.

=== Cause 6 of 8: CreateBackupSelection accepts an IamRoleArn naming a role that does not exist ===
Failing check ids: backup__kms__s3.refarch-bkp-selection-with-nonexistent-role-rejected
AWS: BackupSelection.IamRoleArn is required (Smithy: backup#BackupSelection, target IAMRoleArn) and is the role "that Backup uses to authenticate when backing up the target resource". A selection whose role does not exist (so Backup cannot assume it) is refused InvalidParameterValueException (Smithy: backup#CreateBackupSelection errors, errorKind client, httpError 400) with the message "IAM Role cannot be assumed by AWS Backup" — fixed text; the role ARN is NOT interpolated into it — and no selection is created (ListBackupSelections stays empty).
Emulator: CreateBackupSelection with an IamRoleArn for a role that was never created SUCCEEDS and the selection is persisted.
Why: the single most common real failure of a backup plan (a missing or untrusting service role) is invisible against the emulator; a client's error handling for it is never exercised, and the selection appears healthy while on AWS it would never have been created.
Fix: IamRoleArn resolution/assume-role validation in the backup crate's create_backup_selection handler (resolve the role in the shared IAM store and refuse with the message above when it is absent or does not trust backup.amazonaws.com).

=== Cause 7 of 8: the KMS key ARN a vault reports for its default key does not exist in KMS, so kms:DescribeKey on it is refused NotFoundException ===
Failing check ids: backup__kms__s3.refarch-bkp-vault-defaults-to-aws-managed-backup-key, backup__kms__s3.refarch-bkp-vault-with-a-customer-key-reports-and-resolves-that-key, backup__kms__s3.refarch-bkp-copy-action-to-vault-in-another-region-round-trips
AWS: a vault created with no EncryptionKeyArn gets the AWS-managed Backup key (alias/aws/backup) of that account and region, and DescribeBackupVault/ListBackupVaults report that key's real ARN. kms:DescribeKey with that ARN therefore returns KeyMetadata with Arn equal to the ARN asked for, KeyManager = "AWS" and KeyState = "Enabled", and kms:ListAliases for it includes alias/aws/backup. KMS's NotFoundException (client, 404) is reserved for a key id that names NO existing key.
Emulator: the Backup crate mints the vault's default key ARN inside its own state and never creates that key (or its alias) in the KMS store, so kms:DescribeKey(KeyId=<vault EncryptionKeyArn>) is refused NotFoundException, message "Key '<the ARN from the request>' does not exist" (fixed text "Key '" ... "' does not exist"; the only variable part is the key id echoed from the request). The ARN value in any one replay is a per-run generated id and is not part of the requirement — the requirement is that whatever ARN the vault publishes resolves in KMS. This also breaks the differential half of the customer-key check: a vault created WITH a customer CMK reports that CMK correctly, but its twin created without one reports an unresolvable key, and the per-region default keys cannot be confirmed AWS-managed for a cross-region copy destination.
Note: open issues 20261027T205606Z-backup-kms-s3-a-backup-vault-s-default-encryptionkeyarn-names-a.md and 20261027T205645Z-backup-a-backup-vault-s-default-encryptionkeyarn-names-a.md describe this same class on the kms:ListAliases path; neither carries the replay keys of the three checks above (which fail on kms:DescribeKey), so they are filed here and should be closed together.
Why: Backup and KMS disagree about the same resource — any "which key protects this vault" review (DescribeBackupVault -> DescribeKey/ListAliases, used to prove the vault is on the AWS-managed key rather than a CMK, or to check state/rotation/policy) gets a 404 for a key Backup just said exists, and a client that reads NotFoundException as "the key was deleted" concludes the vault is broken.
Fix: the Backup vault state/create path — resolve (provisioning on first use) the AWS-managed alias/aws/backup key in the shared KMS store per region and record THAT key's ARN on the vault, instead of minting a private KMS ARN in the Backup crate's own state; report EncryptionKeyType consistently with the key actually resolved.

=== Cause 8 of 8: S3 PutBucketVersioning suspends versioning on an Object Lock bucket instead of refusing InvalidBucketState ===
Failing check ids: backup__kms__s3.refarch-bkp-object-lock-bucket-refuses-to-suspend-versioning
AWS: recorded behavior only for the error (Smithy s3#PutBucketVersioning models no errors). Object Lock requires versioning for the life of the bucket, so PutBucketVersioning with VersioningConfiguration.Status = "Suspended" on a bucket created with ObjectLockEnabledForBucket=true is refused with Error.Code = InvalidBucketState and Error.Message = "An Object Lock configuration is present on this bucket, so the versioning state cannot be changed." — fixed text, no request value interpolated. Hypothesis: AWS serves InvalidBucketState with HTTP 409 (its conflict class); the status is not asserted by the recording, the code and message are. A twin bucket WITHOUT Object Lock suspends normally, so the refusal is specific to the lock.
Emulator: the same PutBucketVersioning on the Object Lock bucket SUCCEEDS (no error), while GetObjectLockConfiguration on that bucket correctly reports ObjectLockEnabled=Enabled and the plain twin suspends as expected.
Why: a bucket that WORM-protects backup data can be taken out of versioning, which on AWS is impossible; a client's guard against accidentally suspending versioning under a lock is never exercised, and the emulator's state becomes one AWS cannot represent (Object Lock enabled with versioning suspended).
Fix: the S3 crate's PutBucketVersioning handler validation (services/service-com-amazonaws-s3-amazons3) — refuse a Suspended request when the bucket has an Object Lock configuration, with the code and message above.

## Evidence
Cause 1 — refarch-bkp-cold-storage-within-90-days-of-delete-rejected:
  Expected: CreateBackupPlan refused, Code InvalidParameterValueException, Message "Error in some rules due to : Invalid lifecycle. DeleteAfterDays cannot be less than 90 days apart from MoveToColdStorageAfterDays"; the plan name absent from ListBackupPlans.
  Actual: AssertionError: the call succeeded; real AWS rejects it, so there is no error code (Lifecycle {MoveToColdStorageAfterDays: 30, DeleteAfterDays: 60}).

Cause 2 — refarch-bkp-conditions-selection-round-trips-alongside-resources:
  Expected: Conditions == {'StringEquals': [{'ConditionKey': 'aws:ResourceTag/daily-backup', 'ConditionValue': 'true'}], 'StringNotEquals': [], 'StringLike': [], 'StringNotLike': []}
  Actual: {'StringEquals': [{'ConditionKey': 'aws:ResourceTag/daily-backup', 'ConditionValue': 'true'}]} (the three unset operator lists are missing; the key/value shown are the check's own declared values).

Cause 3 — refarch-bkp-copy-action-destination-vault-not-validated-at-create:
  Expected: ('InvalidParameterValueException', 'Invalid backup vault ARN')
  Actual:   ('InvalidParameterValueException', 'Parameter DestinationBackupVaultArn is invalid.')

Cause 4 — refarch-bkp-plan-deletion-rejected-while-selection-exists:
  Expected message: 'Related backup plan selections must be deleted prior to backup plan deletion' (Code InvalidRequestException, which the emulator gets right)
  Actual message:   'Backup plan <plan id> still holds <n> resource selection(s); all associated selections of resources must be deleted before the backup plan can be deleted' (the plan id and count are run-specific values spliced into a sentence AWS does not use).

Cause 5 — refarch-bkp-plan-rule-defaults-windows-and-omits-cold-storage:
  AssertionError: a rule with no ScheduleExpressionTimezone must default to 'Etc/UTC'; got None (StartWindowMinutes=480 and CompletionWindowMinutes=10080 defaults already pass).

Cause 6 — refarch-bkp-selection-with-nonexistent-role-rejected:
  Expected: ('InvalidParameterValueException', 'IAM Role cannot be assumed by AWS Backup') and ListBackupSelections == []
  Actual: AssertionError: the call succeeded; real AWS rejects it, so there is no error code.

Cause 7 — the three vault/KMS checks:
  Expected: kms:DescribeKey(KeyId=<DescribeBackupVault.EncryptionKeyArn>) returns HTTP 200 with KeyMetadata.Arn == that ARN, KeyManager == 'AWS', KeyState == 'Enabled'.
  Actual: botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DescribeKey operation: Key 'arn:aws:kms:<region>:<account>:key/<generated id>' does not exist (observed ids such as .../key/18e43553-56b8-44c1-0000-000000000001 are per-run values, not requirements).

Cause 8 — refarch-bkp-object-lock-bucket-refuses-to-suspend-versioning:
  Expected: ('InvalidBucketState', 'An Object Lock configuration is present on this bucket, so the versioning state cannot be changed.')
  Actual: AssertionError: the call succeeded where real AWS refuses it (PutBucketVersioning Status=Suspended on an ObjectLockEnabledForBucket=true bucket).

## Steps to Reproduce
Cause 1: backup:CreateBackupVault; backup:CreateBackupPlan with one rule, Lifecycle {MoveToColdStorageAfterDays: 30, DeleteAfterDays: 60}. Expected InvalidParameterValueException with the 90-day message and nothing created; actual 200. (Control: DeleteAfterDays 120 with the same cold-storage day is accepted and round-trips.)
Cause 2: create a plan, then backup:CreateBackupSelection with Resources=[<an ARN>] and Conditions={'StringEquals': [{'ConditionKey': 'aws:ResourceTag/daily-backup', 'ConditionValue': 'true'}]}; backup:GetBackupSelection and read BackupSelection.Conditions. Expected all four operator members (three empty); actual only StringEquals.
Cause 3: backup:CreateBackupPlan with a rule whose CopyActions[0].DestinationBackupVaultArn is a malformed ARN. Expected InvalidParameterValueException 'Invalid backup vault ARN'; actual 'Parameter DestinationBackupVaultArn is invalid.'
Cause 4: create a plan, create a selection on it, then backup:DeleteBackupPlan. Expected InvalidRequestException 'Related backup plan selections must be deleted prior to backup plan deletion'; actual the emulator's plan-id/count sentence.
Cause 5: backup:CreateBackupPlan with a rule that declares only RuleName/TargetBackupVaultName/ScheduleExpression/Lifecycle, then backup:GetBackupPlan. Expected the rule to report ScheduleExpressionTimezone 'Etc/UTC' (and the same under a VersionId-pinned read); actual the member is absent.
Cause 6: create a plan, then backup:CreateBackupSelection with IamRoleArn = arn:aws:iam::<account>:role/<a role that does not exist> and a ListOfTags condition. Expected InvalidParameterValueException 'IAM Role cannot be assumed by AWS Backup' and ListBackupSelections empty; actual the selection is created.
Cause 7: backup:CreateBackupVault with no EncryptionKeyArn; backup:DescribeBackupVault and take EncryptionKeyArn; kms:DescribeKey(KeyId=<that ARN>). Expected 200 with KeyManager 'AWS' (and kms:ListAliases showing alias/aws/backup); actual NotFoundException "Key '<that ARN>' does not exist". Same for the per-region default keys of a cross-region copy destination and for the no-key twin of a CMK vault.
Cause 8: s3:CreateBucket with ObjectLockEnabledForBucket=true; confirm s3:GetBucketVersioning reports Enabled; s3:PutBucketVersioning(VersioningConfiguration={'Status': 'Suspended'}). Expected InvalidBucketState with the Object Lock message; actual 200 (a twin bucket without Object Lock correctly suspends).

Or replay the behavioral checks listed in trace_ids.

Issue-Source: trace-sweep-divergence
Issue-Trace: backup-plan-rule-defaults-windows-and-omits-cold-storage
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-plan-rule-defaults-windows-and-omits-cold-storage backup-rbx-traj-update-adds-cross-region-copy-action backup-rbx-traj-update-plan-refusals backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|GetBackupPlan|2xx->2xx|$.VersionId
Issue-Key: backup|GetBackupPlan|2xx->2xx|$.BackupPlan.Rules[*].CopyActions
Issue-Key: backup|GetBackupPlan|2xx->2xx|$.DeletionDate
Issue-Key: backup|GetBackupPlan|2xx->2xx|$.BackupPlan.Rules[*].StartWindowMinutes
Issue-Key: backup|GetBackupSelection|2xx->2xx|$.BackupSelection.Resources
Issue-Related: 20261029T193050Z-backup-aws-backup-plan-state-and-response-bodies-diverge.md
Issue-Commit: 2b752dc009b2ad79b0f982086ab178b11cbad40c

# AWS Backup plan and selection read bodies diverge: defaulted members omitted, VersionId shape, refused update persisted (5 causes)

> _Note: this issue was identified against the code base at commit `2b752dc009b2ad79b0f982086ab178b11cbad40c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackupPlan

## Description
Five net-new wire divergences in the 200-OK read bodies of AWS Backup's plan and selection readers. Every one is a successful call whose payload differs from AWS's: a member AWS materialises is absent, a generated identifier has a shape AWS never emits, or the stored rule carries a value AWS refused to store. The open issues 20261029T193050Z-backup-aws-backup-plan-state-and-response-bodies-diverge.md and 20261102T110405Z-backup-kms-s3-aws-backup-plan-selection-validation-defaults-and-refusal.md cover neighbouring plan-body gaps but carry none of the replay keys below, so these five are filed here. FIVE causes must all be closed.

Cause 1 of 5 - GetBackupPlan omits the materialised ScheduleExpressionTimezone default and emits a non-AWS VersionId shape (fixture backup-plan-rule-defaults-windows-and-omits-cold-storage, step task#3).
AWS: Smithy backup#BackupRule.ScheduleExpressionTimezone (target Timezone) is documented "The timezone in which the schedule expression is set. By default, ScheduleExpressions are in UTC" - AWS materialises the default on read, and the recording shows it as the literal string 'Etc/UTC' on every rule of a plan created without the member. GetBackupPlanOutput.VersionId is a service-generated opaque string; the recording asserts it only by shape (a long random URL-safe token), not by value.
Emulator: ScheduleExpressionTimezone is absent from BackupPlan.Rules[0] (got None), and VersionId is emitted as a UUID-shaped value of the form '<hex>-<hex>-<hex>-0000-00000000000N' (one recorded instance: '18e473dd-47d4-9ee2-0000-000000000003'), which fails the shape assertion.
Why: a client that reads back a plan to render or re-submit its rules sees no timezone and must guess one, and anything that validates or round-trips VersionId against AWS's token shape rejects the emulator's UUID. The concrete digits are per-run; the shape is the requirement.
Fix: the GetBackupPlan handler's plan-rule serializer (materialise the ScheduleExpressionTimezone default) and the plan-version id generator in the backup crate's state.
Traces: backup-plan-rule-defaults-windows-and-omits-cold-storage#task#3.

Cause 2 of 5 - GetBackupPlan does not report the CopyActions added by UpdateBackupPlan (fixture backup-rbx-traj-update-adds-cross-region-copy-action, step task#4).
AWS: Smithy backup#BackupRule.CopyActions is a list of backup#CopyAction {Lifecycle, DestinationBackupVaultArn (required)}; after an UpdateBackupPlan that adds a copy action to a rule, GetBackupPlan returns that rule with CopyActions populated, each entry carrying the destination vault ARN from the request and the Lifecycle it was given (the recording shows one entry with a cross-Region destination vault ARN and Lifecycle {MoveToColdStorageAfterDays, DeleteAfterDays} and the unset Lifecycle members present as null).
Emulator: BackupPlan.Rules[0].CopyActions is missing entirely (got None); the same response also omits ScheduleExpressionTimezone and emits the UUID-shaped VersionId of cause 1.
Why: a client that adds cross-Region copy to a plan and reads it back to confirm sees a rule with no copy action, so the configuration appears silently discarded.
Fix: UpdateBackupPlan's rule-merge in the backup crate's state plus the GetBackupPlan rule serializer (CopyActions must round-trip).
Traces: backup-rbx-traj-update-adds-cross-region-copy-action#task#4.

Cause 3 of 5 - GetBackupPlan omits DeletionDate for a plan that has been deleted (fixture backup-rbx-traj-update-adds-cross-region-copy-action, step task#5).
AWS: GetBackupPlanOutput has a modelled DeletionDate timestamp (Smithy backup#GetBackupPlanOutput.DeletionDate). When a specific version of a deleted plan is read, AWS returns 200 with DeletionDate set to the time the plan was deleted; the recording asserts the member is present (its instant is a per-run value).
Emulator: the 200 body omits DeletionDate (got None) - and again omits ScheduleExpressionTimezone.
Why: a client cannot tell a live plan from a deleted-but-still-readable version, so delete confirmation and cleanup logic that keys on DeletionDate never completes.
Fix: the backup crate's plan state must record the deletion instant and the GetBackupPlan serializer must emit it.
Traces: backup-rbx-traj-update-adds-cross-region-copy-action#task#5.

Cause 4 of 5 - a rule update that AWS refused is persisted, so GetBackupPlan reports the rejected StartWindowMinutes (fixture backup-rbx-traj-update-plan-refusals, step task#8).
AWS: in the recorded refusal trajectory the UpdateBackupPlan attempt is rejected, so the plan keeps its original rule and GetBackupPlan reports BackupPlan.Rules[0].StartWindowMinutes as the originally created value (60 in the recording; the value comes from the create request).
Emulator: GetBackupPlan reports StartWindowMinutes 480 - the value from the update AWS refused - so the refused mutation was applied to state. The same body omits ScheduleExpressionTimezone and emits the UUID-shaped VersionId.
Why: this is a state divergence, not a formatting one: the emulator accepts an update AWS rejects and every later read of the plan is wrong, so a client's retry/rollback logic diverges permanently from AWS.
Fix: UpdateBackupPlan's validation in the backup crate must reject before mutating state (validate-then-commit), not commit then report.
Traces: backup-rbx-traj-update-plan-refusals#task#8.

Cause 5 of 5 - GetBackupSelection omits the materialised empty members of the selection document (fixture backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key, step task#7).
AWS: Smithy backup#BackupSelection has Resources (ResourceArns), NotResources (ResourceArns), ListOfTags and Conditions (backup#Conditions, supporting StringEquals/StringLike/StringNotEquals/StringNotLike). For a tag-based selection AWS returns the unset members materialised as empties: Resources [], NotResources [], and Conditions with all four condition lists present and empty.
Emulator: BackupSelection.Conditions, BackupSelection.NotResources and BackupSelection.Resources are all absent from the 200 body (got None). (The sibling cluster for backup-create-backup-selection-lifecycle#task#6 shows the same omission including ListOfTags.)
Why: SDK consumers that iterate selection.Conditions.StringEquals or selection.Resources get a null dereference or an "unset" branch where AWS gives an empty collection, so selection-inspection code that works against AWS fails here.
Fix: the GetBackupSelection serializer in the backup crate must materialise the selection document's empty lists and the Conditions structure.
Traces: backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

1) backup-plan-rule-defaults-windows-and-omits-cold-storage#task#3 GetBackupPlan 200->200: $.BackupPlan.Rules[0].ScheduleExpressionTimezone missing (expected 'Etc/UTC', got None); $.VersionId value_mismatch (expected '<shaped-not-asserted>', got '18e473dd-47d4-9ee2-0000-000000000003').
2) backup-rbx-traj-update-adds-cross-region-copy-action#task#4 GetBackupPlan 200->200: $.BackupPlan.Rules[0].CopyActions missing (expected [{'DestinationBackupVaultArn': 'arn:aws:backup:us-east-1:000000000000:backup-vault:sim2real-probe-rbx-dest-30fcb0f7', 'Lifecycle': {'DeleteAfterDays': 120, 'DeleteAfterEvent': None, 'MoveToColdStorageAfterDays': 30, 'OptInToArchiveForSupportedResources': None}}], got None); ScheduleExpressionTimezone missing; $.VersionId mismatch.
3) backup-rbx-traj-update-adds-cross-region-copy-action#task#5 GetBackupPlan 200->200: $.DeletionDate missing (expected a timestamp, got None); ScheduleExpressionTimezone missing. Also affected: backup-rbx-traj-update-plan-refusals#task#9.
4) backup-rbx-traj-update-plan-refusals#task#8 GetBackupPlan 200->200: $.BackupPlan.Rules[0].StartWindowMinutes value_mismatch (expected 60, got 480); ScheduleExpressionTimezone missing; $.VersionId mismatch.
5) backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key#task#7 GetBackupSelection 200->200: $.BackupSelection.Conditions missing (expected {'StringEquals': [], 'StringLike': [], 'StringNotEquals': [], 'StringNotLike': []}, got None); $.BackupSelection.NotResources missing (expected [], got None); $.BackupSelection.Resources missing (expected [], got None).

Model checked: backup#BackupRule.ScheduleExpressionTimezone/CopyActions/StartWindowMinutes, backup#CopyAction, backup#BackupSelection, backup#GetBackupPlanOutput.DeletionDate/VersionId.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Cause 1: replay backup-plan-rule-defaults-windows-and-omits-cold-storage and inspect step task#3 - CreateBackupPlan with a rule that sets no ScheduleExpressionTimezone, then GetBackupPlan: the rule comes back with no ScheduleExpressionTimezone and VersionId is UUID-shaped.
3. Cause 2: replay backup-rbx-traj-update-adds-cross-region-copy-action step task#4 - UpdateBackupPlan adding a CopyAction with a cross-Region DestinationBackupVaultArn, then GetBackupPlan: Rules[0].CopyActions is absent.
4. Cause 3: same fixture, step task#5 - read a plan version after DeleteBackupPlan: 200 body has no DeletionDate.
5. Cause 4: replay backup-rbx-traj-update-plan-refusals step task#8 - after the refused UpdateBackupPlan, GetBackupPlan reports StartWindowMinutes 480 instead of the created 60.
6. Cause 5: replay backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key step task#7 - GetBackupSelection on a tag-based selection: Conditions/Resources/NotResources are omitted instead of materialised empties.

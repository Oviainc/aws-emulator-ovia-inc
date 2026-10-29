Issue-Source: trace-sweep-divergence
Issue-Trace: backup-backup-plan-and-selection-lifecycle
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-backup-plan-and-selection-lifecycle backup-backup-plan-list-pagination-and-versions backup-backup-plan-templates-and-export backup-backup-plan-lifecycle-and-template-export backup-backup-plan-advanced-multi-rule backup-backup-plan-from-custom-json backup-backup-plan-from-template-catalog backup-backup-selection-errors backup-create-backup-selection-lifecycle
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|GetBackupPlan|4xx->2xx|$.Code
Issue-Key: backup|GetBackupPlan|4xx->2xx|$.Message
Issue-Key: backup|GetBackupPlan|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|GetBackupPlan|4xx->2xx|status
Issue-Key: backup|GetBackupPlan|4xx->2xx|$
Issue-Key: backup|CreateBackupPlan|2xx->2xx|$.VersionId
Issue-Key: backup|GetBackupPlan|2xx->2xx|$.BackupPlan.Rules[*].ScheduleExpressionTimezone
Issue-Key: backup|GetBackupSelection|2xx->2xx|$.BackupSelection.Conditions
Issue-Key: backup|GetBackupSelection|2xx->2xx|$.BackupSelection.ListOfTags
Issue-Key: backup|GetBackupSelection|2xx->2xx|$.BackupSelection.NotResources
Issue-Key: backup|ListBackupSelections|2xx->2xx|$.BackupSelectionsList[*]
Issue-Key: backup|DeleteBackupPlan|2xx->4xx|$.BackupPlanArn
Issue-Key: backup|DeleteBackupPlan|2xx->4xx|$.BackupPlanId
Issue-Key: backup|DeleteBackupPlan|2xx->4xx|$.DeletionDate
Issue-Key: backup|DeleteBackupPlan|2xx->4xx|$.VersionId
Issue-Key: backup|DeleteBackupPlan|2xx->4xx|status
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# AWS Backup plan state and response bodies diverge: VersionId ignored, deleted plans readable, defaulted members omitted (8 causes)

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackupPlan

## Description
This issue bundles 8 distinct causes in the AWS Backup emulator's plan/selection read path and stored state. They share one fault family: what the emulator reports about a plan or selection does not match what AWS reports — versions are not tracked, deletion is not honored by reads, defaulted response members are dropped, and state the service should not hold is served back. All 8 causes are in the `backup` service and must each be closed before submitting.

Cause 1 of 8: GetBackupPlan ignores the VersionId query parameter.
AWS: GetBackupPlan takes VersionId as an httpQuery member 'versionId' (Smithy: GetBackupPlanInput.VersionId, httpQuery 'versionId') and reads exactly that stored version. A VersionId that names no stored version of the plan — including the version of a plan that has since been deleted — is refused with HTTP 400 ResourceNotFoundException, Code 'ERROR_2202', Message 'Failed reading Backup plan with provided version' (fixed text for this error class), x-amzn-errortype 'ResourceNotFoundException'.
Emulator: answers HTTP 200 and serves the current plan, with no Code, no Message and no errortype header — the VersionId is not matched against anything.
Why: version pinning silently degrades to 'latest', so a client that reads a specific plan version gets different content than it asked for and never learns the version is gone.
Fix: GetBackupPlan's handler and the plan-version state in the backup crate — store plan versions and resolve the requested VersionId, refusing with the ERROR_2202 ResourceNotFoundException when it is unknown.
Traces: backup-backup-plan-and-selection-lifecycle#task#11 (also backup-backup-plan-list-pagination-and-versions#task#7 and #task#9, backup-backup-plan-templates-and-export#task#8).

Cause 2 of 8: a deleted backup plan stays readable forever.
AWS: after DeleteBackupPlan succeeds, a GetBackupPlan for that plan's version converges on HTTP 400 ResourceNotFoundException with Code 'ERROR_2202' and Message 'Failed reading Backup plan with provided version'.
Emulator: keeps answering HTTP 200 with the full BackupPlan body (BackupPlanName, Rules, BackupPlanId/Arn, VersionId, CreatorRequestId, CreationDate) — the recorded poll gave up after 61 attempts over 60s with the status still 200. Notably the emulator does set DeletionDate on that body, so the deletion is recorded in state but not enforced on reads.
Why: deletion is not observable; teardown/convergence loops that wait for a plan to disappear never terminate, and a client can go on reading and using a plan AWS has removed.
Fix: GetBackupPlan's handler / plan state in the backup crate — once a plan (or plan version) carries a DeletionDate, reads of it must refuse rather than return the body.
Traces: backup-backup-plan-lifecycle-and-template-export#teardown#9.

Cause 3 of 8: the minted backup-plan VersionId does not have AWS's shape.
AWS: recorded behavior only — Smithy types CreateBackupPlanOutput.VersionId as an unconstrained string, and the recording asserts its shape class rather than a literal. AWS mints an opaque, random, long version token that is not derivable from the plan id; each plan version gets an unrelated value.
Emulator: returns a UUID-shaped id whose leading fields repeat the plan's own id and whose tail is a zero-padded sequence counter (observed form '<plan-prefix>-0000-00000000000N'), which does not match the recorded shape class.
Why: the version id is guessable and derivable from the plan id, so clients (and tests) can forge or predict version tokens that AWS would never issue; a client that round-trips version ids opaquely is fine, but anything validating their form against AWS's rejects them.
Fix: the backup crate's id minting for plan versions — generate an opaque random token of AWS's form, independent of the plan id and of a global counter.
Traces: backup-backup-plan-advanced-multi-rule#task#2 (9 steps affected across the plan fixtures).

Cause 4 of 8: GetBackupPlan omits each rule's defaulted ScheduleExpressionTimezone (multi-rule plan).
AWS: BackupRule.ScheduleExpressionTimezone defaults to UTC when the caller does not set it (Smithy: BackupRule.ScheduleExpressionTimezone, 'By default, ScheduleExpressions are in UTC'), and GetBackupPlan echoes it back explicitly as 'Etc/UTC' for every rule that has a ScheduleExpression — for each rule independently, not only the first.
Emulator: the member is absent from every rule in the returned BackupPlan.Rules.
Why: a client reading a plan cannot tell which timezone the schedule is interpreted in, and a read-modify-write of a plan (GetBackupPlan then UpdateBackupPlan) drops a field AWS considers set.
Fix: the CreateBackupPlan/UpdateBackupPlan rule normalization in the backup crate — materialize the default timezone at store time so every rule serializes it.
Traces: backup-backup-plan-advanced-multi-rule#task#3 (both Rules[0] and Rules[1] missing it).

Cause 5 of 8: the same omission on single-rule plans.
AWS: as cause 4 — Rules[0].ScheduleExpressionTimezone is 'Etc/UTC'.
Emulator: absent.
Why: as cause 4; listed separately because it is a distinct replay cluster covering the single-rule plan path across four fixtures, so a fix validated only on the multi-rule fixture leaves it unverified.
Fix: same normalization point as cause 4; verify on this cluster too.
Traces: backup-backup-plan-and-selection-lifecycle#task#3 (also backup-backup-plan-from-custom-json#task#6, backup-backup-plan-from-template-catalog#task#5, backup-backup-plan-lifecycle-and-template-export#task#3).

Cause 6 of 8: GetBackupSelection omits the defaulted empty members of BackupSelection.
AWS: GetBackupSelection returns BackupSelection with its collection members materialized even when the create request omitted them (Smithy: BackupSelection members ListOfTags, NotResources, Conditions) — ListOfTags [], NotResources [], and Conditions as an object carrying all four operator keys empty: StringEquals [], StringLike [], StringNotEquals [], StringNotLike [].
Emulator: all three members are absent from the response.
Why: a client that reads a selection and writes it back loses the shape AWS guarantees, and code that indexes Conditions.StringEquals or iterates ListOfTags without a null check breaks against the emulator but not against AWS.
Fix: the GetBackupSelection serializer / selection state in the backup crate — default these members at store or serialize time rather than skipping empties.
Traces: backup-create-backup-selection-lifecycle#task#6.

Cause 7 of 8: ListBackupSelections reports a selection that should not exist.
AWS: the preceding CreateBackupSelection was refused (unassumable IAM role — filed as cause 2 of the companion refusal bundle), so ListBackupSelections for the plan answers HTTP 200 with an empty BackupSelectionsList.
Emulator: answers 200 with one element describing the selection it accepted anyway (SelectionId, SelectionName, BackupPlanId, CreationDate, CreatorRequestId, IamRoleArn).
Why: the listing is the client's view of what AWS actually accepted; reporting a phantom selection means callers believe a resource exists that AWS would never have created, and plan teardown then behaves differently. Hypothesis: this is the stored-state consequence of the missing IamRoleArn validation, so it is expected to clear once that refusal lands — but it is a separate replay cluster and must be re-replayed to confirm.
Fix: CreateBackupSelection's validation (do not persist a rejected selection) plus ListBackupSelections' read of selection state, both in the backup crate.
Traces: backup-backup-plan-and-selection-lifecycle#task#7.

Cause 8 of 8: DeleteBackupPlan refuses a plan that AWS deletes.
AWS: DeleteBackupPlan on an existing plan answers HTTP 200 with BackupPlanId, BackupPlanArn, VersionId and DeletionDate (Smithy: DeleteBackupPlanOutput members BackupPlanId, BackupPlanArn, DeletionDate, VersionId; http code 200). The BackupPlanId/VersionId are the ones the service holds for that plan, the ARN is built from the request's partition/region/account plus that plan id, and DeletionDate is the deletion instant — none of these are literals from any single run.
Emulator: answers HTTP 400 and emits none of those four members. Hypothesis: the stray selection from cause 7 is still attached, so the handler's 'plan still has selections' guard fires; whatever the trigger, the recorded requirement is the 200 and the four-member body.
Why: the plan cannot be deleted at all, so every lifecycle that creates and tears down a plan leaks state and the fixture's teardown fails.
Fix: DeleteBackupPlan's handler and plan state in the backup crate — delete the plan and return the full output body; re-check the selection guard against AWS's behavior.
Traces: backup-backup-plan-and-selection-lifecycle#task#10 (also backup-backup-selection-errors#teardown#7).

Note: no existing open issue covers these. The open backup issues concern unimplemented stubs, vault lock/tiering refusals, recovery-point 501s, vault EncryptionKeyArn and backup eventing; none carries a replay key for the plan-version, plan-deletion or selection-body steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (84 fixtures, 481 steps, 302 mismatched, score 33.3%) produced these 8 backup clusters:
- GetBackupPlan backup-backup-plan-and-selection-lifecycle#task#11 (4 steps affected): expected 400 / Code ERROR_2202 / Message 'Failed reading Backup plan with provided version' / x-amzn-errortype ResourceNotFoundException; got 200 with none of those members.
- GetBackupPlan backup-backup-plan-lifecycle-and-template-export#teardown#9: poll capped at 60s — 'status 200 not in [400] after 61 attempt(s)'; the body returned is the full deleted plan (including DeletionDate) instead of the ERROR_2202 refusal.
- CreateBackupPlan backup-backup-plan-advanced-multi-rule#task#2 (9 steps affected): $.VersionId value_mismatch, expected the recorded shape class ('<shaped-not-asserted>'), got a plan-id-derived sequential UUID form.
- GetBackupPlan backup-backup-plan-advanced-multi-rule#task#3: $.BackupPlan.Rules[0].ScheduleExpressionTimezone and Rules[1].ScheduleExpressionTimezone missing (expected 'Etc/UTC').
- GetBackupPlan backup-backup-plan-and-selection-lifecycle#task#3 (4 steps affected): $.BackupPlan.Rules[0].ScheduleExpressionTimezone missing (expected 'Etc/UTC').
- GetBackupSelection backup-create-backup-selection-lifecycle#task#6: $.BackupSelection.Conditions missing (expected {'StringEquals': [], 'StringLike': [], 'StringNotEquals': [], 'StringNotLike': []}), $.BackupSelection.ListOfTags missing (expected []), $.BackupSelection.NotResources missing (expected []).
- ListBackupSelections backup-backup-plan-and-selection-lifecycle#task#7: $.BackupSelectionsList[0] extra — expected no element, got a full selection record.
- DeleteBackupPlan backup-backup-plan-and-selection-lifecycle#task#10 (2 steps affected): expected 200 with BackupPlanId/BackupPlanArn/VersionId/DeletionDate; got 400 with all four missing.
Model ground: smithy_query operation GetBackupPlan (VersionId httpQuery 'versionId'; ResourceNotFoundException), DeleteBackupPlan (output BackupPlanId/BackupPlanArn/DeletionDate/VersionId, code 200), GetBackupSelection, ListBackupSelections; shape BackupRule (ScheduleExpressionTimezone, default UTC) and BackupSelection (ListOfTags, NotResources, Conditions).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Representative: fixture backup-backup-plan-and-selection-lifecycle, step task#11 — GET /backup/plans/{BackupPlanId}?versionId=<a version that is not stored>; expect 400 ResourceNotFoundException Code ERROR_2202, observe 200 with the current plan.
3. Also replay, one per cause: backup-backup-plan-lifecycle-and-template-export#teardown#9 (poll GetBackupPlan after DeleteBackupPlan until it refuses), backup-backup-plan-advanced-multi-rule#task#2 (inspect the minted VersionId's form), backup-backup-plan-advanced-multi-rule#task#3 and backup-backup-plan-and-selection-lifecycle#task#3 (GetBackupPlan, check every rule's ScheduleExpressionTimezone), backup-create-backup-selection-lifecycle#task#6 (GetBackupSelection, check Conditions/ListOfTags/NotResources), backup-backup-plan-and-selection-lifecycle#task#7 (ListBackupSelections after a selection create that AWS refused), backup-backup-plan-and-selection-lifecycle#task#10 (DeleteBackupPlan on the plan just created).
4. Compare status and the named members against each recorded response.

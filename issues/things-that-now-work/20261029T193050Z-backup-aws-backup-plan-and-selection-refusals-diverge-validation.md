Issue-Source: trace-sweep-divergence
Issue-Trace: backup-backup-plan-error-paths
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-backup-plan-error-paths backup-backup-plan-and-selection-lifecycle backup-backup-selection-errors
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|CreateBackupPlan|4xx->2xx|$.Code
Issue-Key: backup|CreateBackupPlan|4xx->2xx|$.Context
Issue-Key: backup|CreateBackupPlan|4xx->2xx|$.Message
Issue-Key: backup|CreateBackupPlan|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|CreateBackupPlan|4xx->2xx|status
Issue-Key: backup|CreateBackupSelection|4xx->2xx|$.Code
Issue-Key: backup|CreateBackupSelection|4xx->2xx|$.Message
Issue-Key: backup|CreateBackupSelection|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|CreateBackupSelection|4xx->2xx|status
Issue-Key: backup|GetBackupPlan|4xx->4xx|$.Code
Issue-Key: backup|GetBackupPlan|4xx->4xx|$.Message
Issue-Key: backup|DeleteBackupPlan|4xx->4xx|$.Message
Issue-Key: backup|GetBackupSelection|4xx->4xx|$.Code
Issue-Key: backup|GetBackupSelection|4xx->4xx|$.Message
Issue-Key: backup|GetBackupSelection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DeleteBackupSelection|4xx->4xx|$.Code
Issue-Key: backup|DeleteBackupSelection|4xx->4xx|$.Message
Issue-Key: backup|DeleteBackupSelection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DeleteBackupSelection|2xx->4xx|$
Issue-Key: backup|DeleteBackupSelection|2xx->4xx|status
Issue-Key: backup|ListBackupSelections|2xx->4xx|$.BackupSelectionsList
Issue-Key: backup|ListBackupSelections|2xx->4xx|status
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# AWS Backup plan and selection refusals diverge: validation missing, Code member omitted, not-found misclassified (9 causes)

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackupPlan

## Description
This issue bundles 9 distinct causes in the AWS Backup emulator's refusal path for backup plans and backup selections. They share one fault family: the emulator either does not refuse what AWS refuses, refuses what AWS accepts, or renders the refusal with the wrong error shape, error code or message text. All 9 causes are in the `backup` service and must each be closed before submitting.

Cause 1 of 9: CreateBackupPlan accepts an invalid cron ScheduleExpression.
AWS: CreateBackupPlan validates every rule's ScheduleExpression and refuses a syntactically invalid cron expression with HTTP 400 InvalidParameterValueException (Smithy: CreateBackupPlan errors include InvalidParameterValueException, errorKind client, httpError 400; BackupRule.ScheduleExpression targets CronExpression). The modeled error structure carries Code, Message, Type and Context members (Smithy: InvalidParameterValueException members Code/Message/Type/Context), and the recorded refusal fills them as Code 'ERROR_3015', Context 'Cron expression is not valid: Invalid cron expression', Message 'Error in some rules due to : Cron expression is not valid: Invalid cron expression', with header x-amzn-errortype 'InvalidParameterValueException'. The fixed text of that message is 'Error in some rules due to : ' followed by the per-rule validation reason, which for a bad cron is 'Cron expression is not valid: ' plus the parse failure; the Context member carries that same per-rule reason alone. Nothing here is drawn from the request except the parse failure text.
Emulator: answers HTTP 200 and creates the plan, emitting no error body at all ($.Code, $.Context, $.Message and the x-amzn-errortype header are all absent).
Why: a client that relies on AWS rejecting a malformed schedule gets a plan that looks healthy but can never produce a backup job, and an IaC/CLI caller's error handling for ERROR_3015 is never exercised.
Fix: CreateBackupPlan's input validation in the backup crate — add cron/rate expression parsing for BackupRule.ScheduleExpression and raise InvalidParameterValueException with the Code/Context/Message triple above.
Traces: backup-backup-plan-error-paths#task#7.

Cause 2 of 9: CreateBackupSelection accepts an IAM role AWS Backup cannot assume.
AWS: CreateBackupSelection validates BackupSelection.IamRoleArn (Smithy: BackupSelection.IamRoleArn required, target IAMRoleArn) and refuses a role whose trust policy does not let AWS Backup assume it with HTTP 400 InvalidParameterValueException, Code 'ERROR_3017', Message 'IAM Role cannot be assumed by AWS Backup', header x-amzn-errortype 'InvalidParameterValueException'. That message is entirely fixed text for this error class — it interpolates nothing from the request.
Emulator: answers HTTP 200 and stores the selection; no Code, no Message, no x-amzn-errortype.
Why: the client believes the selection is usable; downstream backup jobs would silently never run, and the emulator's state now contains a selection AWS never created, which corrupts the rest of the fixture (see the state-fidelity bundle for the listing and plan-deletion fallout).
Fix: CreateBackupSelection's validation in the backup crate — check the referenced IAM role's existence and its assumability by the backup service principal before minting a SelectionId.
Traces: backup-backup-plan-and-selection-lifecycle#task#6 (also backup-backup-selection-errors#task#6).

Cause 3 of 9: GetBackupPlan's version-not-found refusal omits Code and uses the wrong message.
AWS: GetBackupPlan with a VersionId that names no stored version of the plan refuses with HTTP 400 ResourceNotFoundException (Smithy: GetBackupPlan errors include ResourceNotFoundException; VersionId is an httpQuery member 'versionId'), Code 'ERROR_2202' and Message 'Failed reading Backup plan with provided version'. Both are fixed text for this error class; the requested plan id and version are not interpolated into the message.
Emulator: status 400 and errortype ResourceNotFoundException are right, but $.Code is absent and $.Message is the generic 'Cannot find Backup plan' — AWS's wording for a missing plan, not for a missing version of an existing plan.
Why: a client cannot distinguish 'no such plan' from 'no such version of this plan', and any handler keyed on the Code member sees no code at all.
Fix: GetBackupPlan's error construction in the backup crate — populate ResourceNotFoundException.Code and emit the version-specific message on the version lookup path.
Traces: backup-backup-plan-error-paths#task#2.

Cause 4 of 9: DeleteBackupPlan's not-found message is the generic one instead of AWS's.
AWS: DeleteBackupPlan on a plan id that does not exist refuses with HTTP 400 and Message 'Failed reading latest version of backup plan <backup-plan ARN> - backup plan does not exist!'. The fixed text is 'Failed reading latest version of backup plan ' ... ' - backup plan does not exist!'; the single variable part is the backup-plan ARN, built from the partition, region and account of the request and the BackupPlanId the caller passed (Smithy: DeleteBackupPlan.BackupPlanId is a required httpLabel; errors include ResourceNotFoundException).
Emulator: answers 400 with Message 'Cannot find Backup plan' — the ARN-bearing template is not rendered at all.
Why: callers and operators lose the identity of the plan that was not found, and message-matching clients break.
Fix: DeleteBackupPlan's error construction in the backup crate — render the ARN-interpolated message template for the latest-version read failure.
Traces: backup-backup-plan-error-paths#task#3.

Cause 5 of 9: GetBackupSelection's selection-not-found refusal omits the Code member.
AWS: GetBackupSelection for a SelectionId that does not exist under an existing plan refuses with HTTP 400 ResourceNotFoundException and Code 'ERROR_2303' (Smithy: ResourceNotFoundException carries Code/Message/Type/Context). The status, errortype and message the emulator sends already match the recording; only Code is missing.
Emulator: 400 ResourceNotFoundException with the right message but $.Code absent.
Why: AWS Backup's numeric Code is the stable discriminator between its many 400s; a client switching on it sees null and falls through to a generic failure path.
Fix: the backup crate's shared error serializer — every Backup error structure must carry its Code (this is the same omission as causes 3, 6 and 7, and fixing it centrally covers them).
Traces: backup-backup-plan-and-selection-lifecycle#task#8.

Cause 6 of 9: GetBackupSelection misclassifies an unknown BACKUP PLAN as a missing selection.
AWS: when the BackupPlanId itself names no plan, GetBackupSelection refuses with HTTP 400 InvalidParameterValueException (x-amzn-errortype 'InvalidParameterValueException'), Code 'ERROR_2304', Message 'Cannot find Backup plan with ID' — fixed text for this error class. AWS distinguishes the unknown-plan case (InvalidParameterValue/ERROR_2304) from the unknown-selection case (ResourceNotFound/ERROR_2303, cause 5).
Emulator: answers ResourceNotFoundException with Message 'Cannot find Backup selection' and no Code, i.e. it reports the wrong resource and the wrong error class.
Why: a client retrying/branching on ResourceNotFoundException vs InvalidParameterValueException takes the wrong branch, and the message names the wrong resource, so the real cause (the plan id) is invisible.
Fix: GetBackupSelection's handler in the backup crate — resolve the plan first and raise the plan-scoped InvalidParameterValueException before looking up the selection.
Traces: backup-backup-selection-errors#task#3.

Cause 7 of 9: DeleteBackupSelection misclassifies an unknown BACKUP PLAN the same way.
AWS: with a BackupPlanId that names no plan, DeleteBackupSelection refuses with HTTP 400 InvalidParameterValueException, Code 'ERROR_2302', Message 'Cannot find Backup plan with ID' (fixed text; Smithy: DeleteBackupSelection errors include InvalidParameterValueException and ResourceNotFoundException).
Emulator: answers ResourceNotFoundException with Message 'Cannot find Backup selection' and no Code.
Why: same as cause 6 — wrong error class, wrong resource named, and no Code to discriminate on.
Fix: DeleteBackupSelection's handler in the backup crate — plan resolution ordering plus the plan-scoped error class and Code.
Traces: backup-backup-selection-errors#task#4.

Cause 8 of 9: DeleteBackupSelection refuses an unknown selection id that AWS accepts.
AWS: when the plan exists, DeleteBackupSelection is a no-op success for a SelectionId that is not present — HTTP 200 with an empty JSON body (Smithy: output_shape Unit, http code 200). AWS reserves the 400 for an unknown plan (cause 7); the selection id itself is treated idempotently.
Emulator: answers HTTP 400 with {'__type': 'ResourceNotFoundException', 'Message': 'Cannot find Backup selection'}.
Why: a client that deletes defensively, or retries a delete after a timeout, gets a hard failure where AWS returns success, so idempotent teardown and cleanup loops abort.
Fix: DeleteBackupSelection's handler in the backup crate — make the selection removal idempotent once the plan is resolved.
Traces: backup-backup-plan-and-selection-lifecycle#task#9.

Cause 9 of 9: ListBackupSelections refuses an unknown plan that AWS answers with an empty list.
AWS: recorded behavior only (Smithy models ResourceNotFoundException on ListBackupSelections but the recording shows AWS does not use it here): ListBackupSelections for a BackupPlanId with no selections answers HTTP 200 with BackupSelectionsList present and empty.
Emulator: answers HTTP 400 and omits BackupSelectionsList entirely.
Why: paginating clients that enumerate selections per plan crash on a refusal where AWS hands back an empty page, and the 'absent vs empty list' difference breaks SDK deserialization expectations.
Fix: ListBackupSelections' handler in the backup crate — return a 200 with an always-present (possibly empty) BackupSelectionsList for this case.
Traces: backup-backup-selection-errors#task#5.

Note: no existing open issue covers these. The open backup issues concern stubs (CreateFramework, StartCopyJob, ...), vault-lock/tiering refusals, recovery-point 501s and backup eventing; none carries a replay key for the plan/selection refusal steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (84 fixtures, 481 steps, score 33.3%) produced these 9 backup clusters:
- CreateBackupPlan backup-backup-plan-error-paths#task#7: expected 400 / Code ERROR_3015 / Context 'Cron expression is not valid: Invalid cron expression' / Message 'Error in some rules due to : Cron expression is not valid: Invalid cron expression' / x-amzn-errortype InvalidParameterValueException; got 200 with none of those members.
- CreateBackupSelection backup-backup-plan-and-selection-lifecycle#task#6 (2 steps affected): expected 400 / Code ERROR_3017 / Message 'IAM Role cannot be assumed by AWS Backup' / x-amzn-errortype InvalidParameterValueException; got 200.
- GetBackupPlan backup-backup-plan-error-paths#task#2: 400 vs 400, $.Code missing (expected 'ERROR_2202'), $.Message 'Cannot find Backup plan' vs expected 'Failed reading Backup plan with provided version'.
- DeleteBackupPlan backup-backup-plan-error-paths#task#3: 400 vs 400, $.Message 'Cannot find Backup plan' vs expected 'Failed reading latest version of backup plan <plan-arn> - backup plan does not exist!'.
- GetBackupSelection backup-backup-plan-and-selection-lifecycle#task#8: 400 vs 400, $.Code missing (expected 'ERROR_2303').
- GetBackupSelection backup-backup-selection-errors#task#3: $.Code missing (expected 'ERROR_2304'), $.Message 'Cannot find Backup selection' vs 'Cannot find Backup plan with ID', x-amzn-errortype 'ResourceNotFoundException' vs 'InvalidParameterValueException'.
- DeleteBackupSelection backup-backup-selection-errors#task#4: $.Code missing (expected 'ERROR_2302'), $.Message 'Cannot find Backup selection' vs 'Cannot find Backup plan with ID', x-amzn-errortype 'ResourceNotFoundException' vs 'InvalidParameterValueException'.
- DeleteBackupSelection backup-backup-plan-and-selection-lifecycle#task#9: expected 200 with {}; got 400 {'__type': 'ResourceNotFoundException', 'Message': 'Cannot find Backup selection'}.
- ListBackupSelections backup-backup-selection-errors#task#5: expected 200 with BackupSelectionsList []; got 400 with the member missing.
Model ground: smithy_query operation CreateBackupPlan/CreateBackupSelection/GetBackupPlan/DeleteBackupPlan/GetBackupSelection/DeleteBackupSelection/ListBackupSelections and shape InvalidParameterValueException/ResourceNotFoundException (members Code, Message, Type, Context; httpError 400).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Representative: fixture backup-backup-plan-error-paths, step task#7 — CreateBackupPlan with a rule whose ScheduleExpression is not a valid cron expression; expect 400 InvalidParameterValueException Code ERROR_3015, observe 200.
3. Also replay, one per cause: backup-backup-plan-and-selection-lifecycle#task#6 (CreateBackupSelection with a role AWS Backup cannot assume), backup-backup-plan-error-paths#task#2 (GetBackupPlan?versionId=<unknown>), backup-backup-plan-error-paths#task#3 (DeleteBackupPlan on an unknown plan id), backup-backup-plan-and-selection-lifecycle#task#8 (GetBackupSelection, unknown selection under a real plan), backup-backup-selection-errors#task#3 and #task#4 (GetBackupSelection / DeleteBackupSelection under an unknown plan id), backup-backup-plan-and-selection-lifecycle#task#9 (DeleteBackupSelection, unknown selection under a real plan, expect 200 {}), backup-backup-selection-errors#task#5 (ListBackupSelections on a plan with no selections, expect 200 with []).
4. Compare status, x-amzn-errortype, $.Code, $.Context and $.Message against each recorded response.

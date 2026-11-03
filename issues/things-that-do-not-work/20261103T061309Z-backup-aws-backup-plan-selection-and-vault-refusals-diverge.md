Issue-Source: trace-sweep-divergence
Issue-Trace: backup-rbx-traj-copy-action-destination-refusals
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-rbx-traj-copy-action-destination-refusals backup-rbx-traj-dependency-order-on-removal backup-rbx-traj-plan-and-vault-identifier-forms backup-rbx-traj-second-selection-mistakes-and-recovery
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|CreateBackupPlan|4xx->4xx|$.Code
Issue-Key: backup|CreateBackupPlan|4xx->4xx|$.Context
Issue-Key: backup|CreateBackupPlan|4xx->4xx|$.Message
Issue-Key: backup|CreateBackupPlan|2xx->4xx|$.BackupPlanArn
Issue-Key: backup|CreateBackupPlan|2xx->4xx|$.BackupPlanId
Issue-Key: backup|CreateBackupPlan|2xx->4xx|$.CreationDate
Issue-Key: backup|CreateBackupPlan|2xx->4xx|$.VersionId
Issue-Key: backup|CreateBackupPlan|2xx->4xx|status
Issue-Key: backup|DeleteBackupPlan|4xx->4xx|$.Code
Issue-Key: backup|DeleteBackupVault|4xx->4xx|status
Issue-Key: backup|GetBackupPlan|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DescribeBackupVault|4xx->4xx|$.Message
Issue-Key: backup|DescribeBackupVault|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DescribeBackupVault|4xx->4xx|status
Issue-Key: backup|CreateBackupSelection|4xx->4xx|$.Code
Issue-Key: backup|CreateBackupSelection|4xx->4xx|$.Message
Issue-Key: backup|CreateBackupSelection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|CreateBackupSelection|2xx->4xx|$.BackupPlanId
Issue-Key: backup|CreateBackupSelection|2xx->4xx|$.CreationDate
Issue-Key: backup|CreateBackupSelection|2xx->4xx|$.SelectionId
Issue-Key: backup|CreateBackupSelection|2xx->4xx|status
Issue-Key: backup|ListBackupSelections|2xx->2xx|$.BackupSelectionsList[*].IamRoleArn
Issue-Key: backup|ListBackupSelections|2xx->2xx|$.BackupSelectionsList[*].SelectionId
Issue-Related: 20261029T193050Z-backup-aws-backup-plan-and-selection-refusals-diverge-validation.md
Issue-Commit: 2b752dc009b2ad79b0f982086ab178b11cbad40c

# AWS Backup plan, selection and vault refusals diverge: wrong status, missing Code, emulator-authored messages, valid requests refused (10 causes)

> _Note: this issue was identified against the code base at commit `2b752dc009b2ad79b0f982086ab178b11cbad40c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackupPlan

## Description
Ten net-new wire divergences on AWS Backup's validation and refusal paths for plans, selections and vaults. The shared fault is that the emulator decides refusals on its own rules and renders them in its own words: it omits the Code member AWS puts in every InvalidParameterValueException body, substitutes emulator-authored sentences for AWS's fixed texts, classifies not-found/authorization cases as the wrong exception and status, and both refuses requests AWS accepts and accepts ones AWS refuses. The open issues 20261029T193050Z-backup-aws-backup-plan-and-selection-refusals-diverge-validation.md and 20261102T110405Z-backup-kms-s3-aws-backup-plan-selection-validation-defaults-and-refusal.md cover other steps of the same family but carry none of the replay keys below. TEN causes must all be closed.

Cause 1 of 10 - CreateBackupPlan refuses a malformed copy-action destination vault ARN with the emulator's own code, context and sentence (fixture backup-rbx-traj-copy-action-destination-refusals, step task#2).
AWS: Smithy backup#CopyAction.DestinationBackupVaultArn is a required ARN; CreateBackupPlan models InvalidParameterValueException (400). The recorded refusal body is Code 'ERROR_2307', Context null, Message 'Invalid backup vault ARN' - the message is fixed text with no variable part.
Emulator: 400 with Code 'INVALID_PARAMETER_VALUE', Context 'DestinationBackupVaultArn' and Message 'Parameter DestinationBackupVaultArn is invalid.'
Why: clients branch on Code (ERROR_2307) and match AWS's sentence; both differ, and the extra Context member is a field AWS leaves null here.
Fix: CreateBackupPlan's copy-action validation and the InvalidParameterValueException renderer in the backup crate.
Traces: backup-rbx-traj-copy-action-destination-refusals#task#2.

Cause 2 of 10 - CreateBackupPlan refuses a plan AWS accepts (fixture backup-rbx-traj-copy-action-destination-refusals, step task#4).
AWS: after the refusal above, the trajectory re-submits CreateBackupPlan with a well-formed destination backup-vault ARN; AWS answers 200 with BackupPlanId, BackupPlanArn, VersionId and CreationDate (all server-generated, asserted by shape only). A copy-action destination may name a vault in another Region, and AWS does not require it to resolve locally at create time.
Emulator: 400 - the create is refused and no plan is returned.
Why: the whole cross-Region copy workflow is unreachable; a client that recovers from the first refusal by fixing the ARN still cannot create its plan.
Fix: CreateBackupPlan's copy-action validation must accept a syntactically valid backup-vault ARN (including a foreign-Region one) instead of requiring local resolution.
Traces: backup-rbx-traj-copy-action-destination-refusals#task#4.

Cause 3 of 10 - DeleteBackupPlan's "selections still attached" refusal omits Code and uses the emulator's sentence (fixture backup-rbx-traj-dependency-order-on-removal, step task#6).
AWS: deleting a plan that still has backup selections is refused 400 with Code 'ERROR_9201' and the fixed Message 'Related backup plan selections must be deleted prior to backup plan deletion' (DeleteBackupPlan models InvalidRequestException/InvalidParameterValueException, both 400).
Emulator: 400 with no Code member and the emulator-authored sentence 'Backup plan <planId> still holds N resource selection(s); all associated selections of resources must be deleted before the backup plan can be deleted' (the plan id and count are substituted per run; AWS's text has no variable part at all).
Why: a client keying on ERROR_9201 or on AWS's sentence does not recognise the dependency refusal and cannot drive the delete-selections-first retry.
Fix: DeleteBackupPlan's dependency check and its error renderer in the backup crate.
Traces: backup-rbx-traj-dependency-order-on-removal#task#6.

Cause 4 of 10 - DeleteBackupVault answers 400 where AWS answers 403 (fixture backup-rbx-traj-dependency-order-on-removal, step teardown#14).
AWS: recorded behavior only - the recording asserts HTTP status 403 for this DeleteBackupVault attempt (an authorization refusal) and asserts no body members.
Emulator: 400.
Why: 403 and 400 are different classes to every SDK retry/permission layer: a caller that distinguishes "not allowed" from "bad request" takes the wrong branch, and permission-probing teardown logic never terminates.
Fix: DeleteBackupVault's authorization check in the backup crate must run before parameter/state validation and refuse with the 403 access-denied class.
Traces: backup-rbx-traj-dependency-order-on-removal#teardown#14.

Cause 5 of 10 - GetBackupPlan misclassifies an ARN-shaped plan identifier as not-found instead of invalid (fixture backup-rbx-traj-plan-and-vault-identifier-forms, steps task#3 and task#4).
AWS: GetBackupPlanInput.BackupPlanId is an httpLabel plan id, not an ARN. Given an ARN-shaped identifier AWS refuses 400 InvalidParameterValueException with Code 'ERROR_3208' and the fixed Message 'Invalid Backup plan ID'.
Emulator: 400 with no Code, Message 'Cannot find Backup plan' and x-amzn-errortype 'ResourceNotFoundException'.
Why: the client cannot tell "you passed the wrong identifier form" from "the plan is gone" - the first is a bug to fix in the request, the second is a retry/skip decision.
Fix: GetBackupPlan's identifier validation (reject non-plan-id forms before the state lookup) and the error renderer that drops Code.
Traces: backup-rbx-traj-plan-and-vault-identifier-forms#task#3.

Cause 6 of 10 - DescribeBackupVault answers a vault-ARN identifier with its own regex-violation sentence at 400 instead of AWS's 403 (fixture backup-rbx-traj-plan-and-vault-identifier-forms, step task#7).
AWS: DescribeBackupVaultInput.BackupVaultName is an httpLabel. When an ARN is supplied in that label AWS refuses with HTTP 403 and the fixed Message 'Insufficient privileges to perform this action.'; the recording does not pin the errortype header.
Emulator: 400 with x-amzn-errortype 'InvalidParameterValueException' and the emulator-authored sentence 'BackupVaultName "<value>" must satisfy regular expression pattern: ^[a-zA-Z0-9\\-\\_]{2,50}$' (the quoted value is the request's, the regex text is the emulator's own wording; AWS never emits this sentence for this case).
Why: wrong status class and wrong message for a very common client mistake (passing an ARN where a name is modelled), so permission-vs-validation handling diverges.
Fix: DescribeBackupVault's label handling/authorization ordering in the backup crate, and suppress the pattern-echo sentence.
Traces: backup-rbx-traj-plan-and-vault-identifier-forms#task#7.

Cause 7 of 10 - CreateBackupSelection reports a duplicate-name conflict where AWS reports an IAM-role authorization failure (fixture backup-rbx-traj-second-selection-mistakes-and-recovery, step task#5).
AWS: Smithy backup#BackupSelection.IamRoleArn is required and is the role Backup assumes; when that role cannot call the tag APIs AWS refuses 400 InvalidParameterValueException with Code 'ERROR_3018' and the fixed Message 'IAM Role is not authorized to call tag:GetResources'.
Emulator: 400 AlreadyExistsException with Message 'Backup selection with the same name already exists' and no Code - it never evaluates the role and instead trips its own name-uniqueness rule.
Why: the caller is told to rename its selection when the real defect is the role's permissions, so the recovery path AWS expects (fix the role) is never taken.
Fix: CreateBackupSelection's validation order in the backup crate - validate the IAM role before the name-uniqueness check - plus the Code/errortype renderer.
Traces: backup-rbx-traj-second-selection-mistakes-and-recovery#task#5.

Cause 8 of 10 - CreateBackupSelection refuses a selection AWS creates (fixture backup-rbx-traj-second-selection-mistakes-and-recovery, steps task#7 and task#8).
AWS: after the role is corrected, CreateBackupSelection with the same SelectionName succeeds: 200 with SelectionId (server-generated, shape-asserted), BackupPlanId and CreationDate. AWS does not key uniqueness on SelectionName alone.
Emulator: 400 - no selection is created.
Why: a client that fixes the role and retries is permanently blocked, and the SelectionId it needs for every later selection call is never issued.
Fix: CreateBackupSelection's uniqueness rule in the backup crate's state (do not reject on name alone, and do not record state for a refused create).
Traces: backup-rbx-traj-second-selection-mistakes-and-recovery#task#7.

Cause 9 of 10 - the duplicate-selection refusal names the wrong conflict (fixture backup-rbx-traj-second-selection-mistakes-and-recovery, step task#11).
AWS: re-submitting an identical selection document is refused 400 with Code 'ERROR_1301' and the fixed Message 'Backup selection with the same selection document already exists'.
Emulator: 400 with no Code and Message 'Backup selection with the same name already exists'.
Why: AWS's rule is document identity, the emulator's is name identity; the message tells the client to change the wrong thing and the Code it switches on is absent.
Fix: CreateBackupSelection's conflict detection (compare the selection document) and its error renderer.
Traces: backup-rbx-traj-second-selection-mistakes-and-recovery#task#11.

Cause 10 of 10 - ListBackupSelections reports the selection AWS refused, with the un-authorized role (fixture backup-rbx-traj-second-selection-mistakes-and-recovery, step task#13).
AWS: after the trajectory above, the listed selections carry the IamRoleArn of the role that was actually accepted; the selection built on the rejected role is never persisted, so no listed entry carries it.
Emulator: BackupSelectionsList[0].IamRoleArn is the rejected role's ARN (recorded run: an '...-absent-...' role ARN where AWS listed the corrected role ARN), i.e. the create the emulator should have refused was persisted and the one AWS accepted was not. (The SelectionId mismatches in the same cluster are unresolved per-run placeholders and are not part of this requirement.)
Why: this is the state-visible consequence of causes 7-9: every subsequent read of the plan's selections describes a selection AWS does not have, bound to a role that cannot back anything up.
Fix: the backup crate's selection state must only commit a selection whose role and document passed validation.
Traces: backup-rbx-traj-second-selection-mistakes-and-recovery#task#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up), expected vs actual per cluster:

1) backup-rbx-traj-copy-action-destination-refusals#task#2 CreateBackupPlan 400->400: $.Code expected 'ERROR_2307', got 'INVALID_PARAMETER_VALUE'; $.Context expected None, got 'DestinationBackupVaultArn'; $.Message expected 'Invalid backup vault ARN', got 'Parameter DestinationBackupVaultArn is invalid.'
2) backup-rbx-traj-copy-action-destination-refusals#task#4 CreateBackupPlan: status expected 200, got 400; BackupPlanArn/BackupPlanId/CreationDate/VersionId all missing.
3) backup-rbx-traj-dependency-order-on-removal#task#6 DeleteBackupPlan 400->400: $.Code expected 'ERROR_9201', got None; $.Message expected 'Related backup plan selections must be deleted prior to backup plan deletion', got 'Backup plan 18e473dd-bcae-037a-0000-000000000002 still holds 1 resource selection(s); all associated selections of resources must be deleted before the backup plan can be deleted'.
4) backup-rbx-traj-dependency-order-on-removal#teardown#14 DeleteBackupVault: status expected 403, got 400 (no body asserted).
5) backup-rbx-traj-plan-and-vault-identifier-forms#task#3 (and #task#4) GetBackupPlan 400->400: $.Code expected 'ERROR_3208', got None; $.Message expected 'Invalid Backup plan ID', got 'Cannot find Backup plan'; x-amzn-errortype got 'ResourceNotFoundException' where the recording does not pin it.
6) backup-rbx-traj-plan-and-vault-identifier-forms#task#7 DescribeBackupVault: status expected 403, got 400; $.Message expected 'Insufficient privileges to perform this action.', got 'BackupVaultName "arn:aws:backup:us-west-2:000000000000:backup-vault:..." must satisfy regular expression pattern: ^[a-zA-Z0-9\\-\\_]{2,50}$'; errortype got 'InvalidParameterValueException'.
7) backup-rbx-traj-second-selection-mistakes-and-recovery#task#5 CreateBackupSelection 400->400: $.Code expected 'ERROR_3018', got None; $.Message expected 'IAM Role is not authorized to call tag:GetResources', got 'Backup selection with the same name already exists'; errortype got 'AlreadyExistsException'.
8) backup-rbx-traj-second-selection-mistakes-and-recovery#task#7 (and #task#8) CreateBackupSelection: status expected 200, got 400; SelectionId/BackupPlanId/CreationDate missing.
9) backup-rbx-traj-second-selection-mistakes-and-recovery#task#11 CreateBackupSelection 400->400: $.Code expected 'ERROR_1301', got None; $.Message expected 'Backup selection with the same selection document already exists', got 'Backup selection with the same name already exists'.
10) backup-rbx-traj-second-selection-mistakes-and-recovery#task#13 ListBackupSelections 200->200: $.BackupSelectionsList[0].IamRoleArn expected the accepted role ARN, got the rejected '...-absent-...' role ARN.

Model checked: backup#CreateBackupPlan/CreateBackupSelection/GetBackupPlan/DeleteBackupPlan/DescribeBackupVault/DeleteBackupVault/ListBackupSelections signatures and error sets (InvalidParameterValueException, InvalidRequestException, ResourceNotFoundException, AlreadyExistsException - all 400), backup#CopyAction, backup#BackupSelection.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Causes 1-2: replay backup-rbx-traj-copy-action-destination-refusals - step task#2 (CreateBackupPlan with a malformed copy-action destination vault ARN) and step task#4 (same create with a well-formed cross-Region destination ARN, which must succeed 200).
3. Causes 3-4: replay backup-rbx-traj-dependency-order-on-removal - step task#6 (DeleteBackupPlan while a selection is still attached) and step teardown#14 (DeleteBackupVault, must be 403).
4. Causes 5-6: replay backup-rbx-traj-plan-and-vault-identifier-forms - step task#3 (GetBackupPlan with an ARN-shaped BackupPlanId) and step task#7 (DescribeBackupVault with a vault ARN in the BackupVaultName label, must be 403).
5. Causes 7-10: replay backup-rbx-traj-second-selection-mistakes-and-recovery - step task#5 (CreateBackupSelection whose IamRoleArn cannot call tag:GetResources), step task#7 (retry with the corrected role, must be 200), step task#11 (identical selection document re-submitted), step task#13 (ListBackupSelections must show only the accepted selection's role).

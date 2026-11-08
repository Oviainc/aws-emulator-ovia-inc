Issue-Source: trace-sweep-divergence
Issue-Trace: backup-rbx-traj-copy-action-destination-refusals
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-rbx-traj-copy-action-destination-refusals backup-rbx-traj-dependency-order-on-removal backup-rbx-traj-plan-and-vault-identifier-forms backup-rbx-traj-second-selection-mistakes-and-recovery
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|CreateBackupPlan|4xx->2xx|$.Code
Issue-Key: backup|CreateBackupPlan|4xx->2xx|$.Context
Issue-Key: backup|CreateBackupPlan|4xx->2xx|$.Message
Issue-Key: backup|CreateBackupPlan|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|CreateBackupPlan|4xx->2xx|status
Issue-Key: backup|DeleteBackupPlan|4xx->4xx|$.Message
Issue-Key: backup|GetBackupPlan|4xx->4xx|$.Code
Issue-Key: backup|GetBackupPlan|4xx->4xx|$.Message
Issue-Key: backup|CreateBackupSelection|4xx->2xx|$.Code
Issue-Key: backup|CreateBackupSelection|4xx->2xx|$.Message
Issue-Key: backup|CreateBackupSelection|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|CreateBackupSelection|4xx->2xx|status
Issue-Key: backup|GetBackupSelection|4xx->4xx|$.Code
Issue-Key: backup|GetBackupSelection|4xx->4xx|$.Message
Issue-Key: backup|GetBackupSelection|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261103T061309Z-backup-aws-backup-plan-selection-and-vault-refusals-diverge.md
Issue-Commit: 513decae8af1d9c2ed82349ae5607f8b16d30278

# AWS Backup plan and selection refusals diverge: missing lifecycle/tag-permission/empty-selection validation, omitted Code and emulator-authored messages (6 causes)

> _Note: this issue was identified against the code base at commit `513decae8af1d9c2ed82349ae5607f8b16d30278`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackupPlan

## Description
Six net-new causes on the AWS Backup plan/selection write and read paths. They share one root fault: the emulator's plan- and selection-level refusal layer is incomplete and self-authored — three request shapes AWS refuses are accepted with 200, and three refusals that do fire carry the emulator's own error class, omit the service's numeric `Code` member, or use emulator wording instead of the service's fixed text. All six are on service `backup`, which is bootstrapped.

NOTE ON EXISTING ISSUES: the replay report attributes none of these six clusters to an open issue. The nearest open issues are 20261103T061309Z-backup-aws-backup-plan-selection-and-vault-refusals-diverge.md, 20261102T110405Z-backup-kms-s3-aws-backup-plan-selection-validation-defaults-and-refusal.md and 20261029T193050Z-backup-aws-backup-plan-state-and-response-bodies-diverge.md; they cover different steps of these same fixtures and do not carry the replay keys below, so these causes must be closed in addition to them.

--- Cause 1 of 6: CreateBackupPlan does not validate the cold-storage/delete lifecycle spread (fixture backup-rbx-traj-copy-action-destination-refusals, step task#3) ---
AWS: Smithy: CreateBackupPlan errors include InvalidParameterValueException (httpError 400, members Code/Message/Type/Context). Shape Lifecycle documents the rule explicitly: DeleteAfterDays "must be at least 90 days after the number of days specified in MoveToColdStorageAfterDays". A plan rule (or copy-action) lifecycle that violates it is refused HTTP 400 with Code "ERROR_3011", Message "Invalid lifecycle. DeleteAfterDays cannot be less than 90 days apart from MoveToColdStorageAfterDays" and Context carrying the required spread. The whole Message is fixed text for this error class — no request value is interpolated into it; the only variable part is Context, which carries the minimum spread in days (90) the service enforces.
Emulator: answers HTTP 200 and creates the plan. No Code, no Message, no x-amzn-errortype header at all.
Why: a client that submits an invalid retention/tiering lifecycle gets a plan the real service would never have created. Infrastructure code and tests that assert the guard (expecting 400 InvalidParameterValueException) pass against the emulator and fail in AWS, and the emulator's stored plan has a lifecycle that cannot exist in the service.
Fix: CreateBackupPlan's input validation — add the lifecycle spread check over every rule's Lifecycle and every rule's CopyActions[].Lifecycle before the plan is committed to state, raising InvalidParameterValueException with the Code/Message/Context above.
Traces: backup-rbx-traj-copy-action-destination-refusals#task#3.

--- Cause 2 of 6: DeleteBackupPlan's selection-dependency refusal omits Code and rewords the message (fixture backup-rbx-traj-dependency-order-on-removal, step task#6) ---
AWS: Smithy: DeleteBackupPlan errors include InvalidRequestException (httpError 400, members Code/Message/Type/Context). Deleting a plan that still has backup selections is refused 400 with Code "ERROR_9201" and Message "Related backup plan selections must be deleted prior to backup plan deletion". That Message is entirely fixed text for the error class: AWS names neither the plan id nor the selection count in it.
Emulator: returns 400 but with no `Code` member at all, and Message "Backup plan <plan-id> still holds <n> resource selection(s); all associated selections of resources must be deleted before the backup plan can be deleted" — emulator-authored prose that additionally interpolates the plan id and a selection count AWS does not include.
Why: clients and operator tooling branch on the numeric Code (ERROR_9201) to distinguish "delete the selections first" from other InvalidRequestException causes; with Code absent they cannot classify the refusal, and message-matching clients break on the reworded text.
Fix: the DeleteBackupPlan handler's dependency guard — emit Code "ERROR_9201" and the service's fixed Message, dropping the interpolated plan id and count.
Traces: backup-rbx-traj-dependency-order-on-removal#task#6.

--- Cause 3 of 6: GetBackupPlan misclassifies a malformed plan identifier as a version read failure (fixture backup-rbx-traj-plan-and-vault-identifier-forms, step task#3; also task#4) ---
AWS: Smithy: GetBackupPlan input BackupPlanId is a required httpLabel; errors include InvalidParameterValueException and ResourceNotFoundException (both httpError 400, members Code/Message/Type/Context). A BackupPlanId that is not a well-formed plan id (e.g. a plan ARN supplied where the bare id is required) is refused 400 with Code "ERROR_3208" and Message "Invalid Backup plan ID" — fixed text, with no request value interpolated.
Emulator: returns 400 with Code "ERROR_2202", Message "Failed reading Backup plan with provided version" and x-amzn-errortype "ResourceNotFoundException" — i.e. it treats a malformed identifier as an existing-plan/unknown-version lookup miss.
Why: the two conditions are not interchangeable. A client retrying a not-found read, or one that validates its own identifier handling by asserting the invalid-ID refusal, is told the plan version could not be read rather than that the identifier it sent is unusable, so it retries or fails over instead of correcting the argument.
Fix: GetBackupPlan's input validation, ahead of the state lookup — validate BackupPlanId's form first and refuse InvalidParameterValueException with Code "ERROR_3208"/Message "Invalid Backup plan ID", reserving the ERROR_2202 ResourceNotFoundException path for a well-formed id whose plan/version is genuinely absent.
Traces: backup-rbx-traj-plan-and-vault-identifier-forms#task#3, backup-rbx-traj-plan-and-vault-identifier-forms#task#4.

--- Cause 4 of 6: CreateBackupSelection does not check the selection role's tag:GetResources permission (fixture backup-rbx-traj-second-selection-mistakes-and-recovery, step task#5) ---
AWS: Smithy: CreateBackupSelection errors include InvalidParameterValueException (httpError 400, members Code/Message/Type/Context). A tag-based selection whose IamRoleArn cannot call tag:GetResources is refused at create time, HTTP 400, Code "ERROR_3018", Message "IAM Role is not authorized to call tag:GetResources" — fixed text for the class, with no role name interpolated.
Emulator: answers HTTP 200 and stores the selection; no Code, Message or x-amzn-errortype is emitted.
Why: AWS validates the selection role up front precisely so a plan never silently resolves zero resources. A client that relies on the create-time refusal to catch an under-privileged role sees success against the emulator and then a plan that protects nothing in AWS.
Fix: CreateBackupSelection's validation — when the selection carries ListOfTags, verify the supplied IamRoleArn's tag:GetResources permission and refuse InvalidParameterValueException with the Code/Message above before the selection reaches state.
Traces: backup-rbx-traj-second-selection-mistakes-and-recovery#task#5.

--- Cause 5 of 6: CreateBackupSelection accepts a selection with neither ListOfTags nor Resources (fixture backup-rbx-traj-second-selection-mistakes-and-recovery, step task#10) ---
AWS: Smithy: CreateBackupSelection input BackupSelection is required; errors include InvalidParameterValueException (httpError 400). A BackupSelection whose ListOfTags and Resources are both empty is refused HTTP 400 with Message "Invalid backup selection. Either 'ListOfTags' or 'Resources' section must be non-empty." — fixed text naming the two member names from the model, with nothing taken from the request.
Emulator: answers HTTP 200 and creates an empty selection; no Message and no x-amzn-errortype.
Why: an empty selection is a plan that silently backs up nothing. Clients depend on this refusal to surface a mis-built selection document at create time; the emulator reports success, so the defect only appears as missing backups.
Fix: CreateBackupSelection's validation — reject a BackupSelection with both ListOfTags and Resources empty, with the quoted Message, before committing it.
Traces: backup-rbx-traj-second-selection-mistakes-and-recovery#task#10.

--- Cause 6 of 6: GetBackupSelection misclassifies a malformed SelectionId as not-found (fixture backup-rbx-traj-second-selection-mistakes-and-recovery, step task#12) ---
AWS: Smithy: GetBackupSelection input SelectionId is a required httpLabel; errors include InvalidParameterValueException and ResourceNotFoundException (both httpError 400, members Code/Message/Type/Context). A SelectionId that is not a well-formed selection id is refused 400 with Code "ERROR_3314" and Message "Invalid Backup selection ID" — fixed text, no request value interpolated.
Emulator: returns 400 with Code "ERROR_2303", Message "Cannot find Backup selection" and x-amzn-errortype "ResourceNotFoundException".
Why: same class confusion as cause 3, on the selection read path: a client told the selection does not exist retries or recreates, where AWS told it the identifier it sent was invalid. Error-code-driven handling (ERROR_3314 vs ERROR_2303) takes the wrong branch.
Fix: GetBackupSelection's input validation, ahead of the state lookup — validate SelectionId's form and refuse InvalidParameterValueException with Code "ERROR_3314"/Message "Invalid Backup selection ID", keeping ResourceNotFoundException for a well-formed id that is genuinely absent.
Traces: backup-rbx-traj-second-selection-mistakes-and-recovery#task#12.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). Six net-new divergence clusters, none carrying stub_path or owned_by:

1) CreateBackupPlan — backup-rbx-traj-copy-action-destination-refusals#task#3
   expected status 400, got 200
   $.Code: missing (expected 'ERROR_3011', got None)
   $.Context: missing (expected '90', got None)
   $.Message: missing (expected 'Invalid lifecycle. DeleteAfterDays cannot be less than 90 days apart from MoveToColdStorageAfterDays', got None)
   $headers.x-amzn-errortype: missing

2) DeleteBackupPlan — backup-rbx-traj-dependency-order-on-removal#task#6 (status 400 both sides)
   $.Code: missing (expected 'ERROR_9201', got None)
   $.Message: expected 'Related backup plan selections must be deleted prior to backup plan deletion',
              got 'Backup plan 18e5f0e7-0761-d0a6-0000-000000000002 still holds 1 resource selection(s); all associated selections of resources must be deleted before the backup plan can be deleted'

3) GetBackupPlan — backup-rbx-traj-plan-and-vault-identifier-forms#task#3 (+ #task#4) (status 400 both sides)
   $.Code: expected 'ERROR_3208', got 'ERROR_2202'
   $.Message: expected 'Invalid Backup plan ID', got 'Failed reading Backup plan with provided version'
   $headers.x-amzn-errortype: expected <shaped>, got 'ResourceNotFoundException'

4) CreateBackupSelection — backup-rbx-traj-second-selection-mistakes-and-recovery#task#5
   expected status 400, got 200
   $.Code: missing (expected 'ERROR_3018')
   $.Message: missing (expected 'IAM Role is not authorized to call tag:GetResources')

5) CreateBackupSelection — backup-rbx-traj-second-selection-mistakes-and-recovery#task#10
   expected status 400, got 200
   $.Message: missing (expected "Invalid backup selection. Either 'ListOfTags' or 'Resources' section must be non-empty.")

6) GetBackupSelection — backup-rbx-traj-second-selection-mistakes-and-recovery#task#12 (status 400 both sides)
   $.Code: expected 'ERROR_3314', got 'ERROR_2303'
   $.Message: expected 'Invalid Backup selection ID', got 'Cannot find Backup selection'
   $headers.x-amzn-errortype: expected <shaped>, got 'ResourceNotFoundException'

Model grounding (services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json):
- CreateBackupPlan / CreateBackupSelection errors: AlreadyExistsException, InvalidParameterValueException, LimitExceededException, MissingParameterValueException (all httpError 400).
- DeleteBackupPlan errors: InvalidParameterValueException, InvalidRequestException, MissingParameterValueException, ResourceNotFoundException (all 400).
- GetBackupPlan / GetBackupSelection errors: InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException (all 400).
- All four error shapes carry Code, Message, Type and Context members — so an omitted Code is a dropped modelled member, not an optional extra.
- Shape Lifecycle doc: "DeleteAfterDays ... must be at least 90 days after the number of days specified in MoveToColdStorageAfterDays" — the rule behind cause 1.

Per-run values above (plan ids, selection ids, generated names) are illustrative; the requirement is the status, the error class, the Code and the fixed message text.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout and inspect these steps:

1. backup-rbx-traj-copy-action-destination-refusals, step task#3 — CreateBackupPlan with a rule/copy-action Lifecycle whose DeleteAfterDays is fewer than 90 days above MoveToColdStorageAfterDays. Emulator: 200 + plan created. AWS: 400 InvalidParameterValueException, Code ERROR_3011, Context 90.
2. backup-rbx-traj-dependency-order-on-removal, step task#6 — create a plan, add a backup selection, then DeleteBackupPlan. Emulator: 400 with no Code and emulator wording. AWS: 400 Code ERROR_9201, Message 'Related backup plan selections must be deleted prior to backup plan deletion'.
3. backup-rbx-traj-plan-and-vault-identifier-forms, steps task#3 and task#4 — GetBackupPlan with a malformed BackupPlanId (plan ARN in place of the bare id). Emulator: Code ERROR_2202 / ResourceNotFoundException. AWS: Code ERROR_3208, Message 'Invalid Backup plan ID'.
4. backup-rbx-traj-second-selection-mistakes-and-recovery, step task#5 — CreateBackupSelection with a tag-based ListOfTags selection whose IamRoleArn lacks tag:GetResources. Emulator: 200. AWS: 400 Code ERROR_3018.
5. backup-rbx-traj-second-selection-mistakes-and-recovery, step task#10 — CreateBackupSelection with both ListOfTags and Resources empty. Emulator: 200. AWS: 400 "Invalid backup selection. Either 'ListOfTags' or 'Resources' section must be non-empty."
6. backup-rbx-traj-second-selection-mistakes-and-recovery, step task#12 — GetBackupSelection with a malformed SelectionId. Emulator: Code ERROR_2303 / 'Cannot find Backup selection'. AWS: Code ERROR_3314 / 'Invalid Backup selection ID'.

All six must be closed before this issue is submitted; re-replay each fixture#step listed in Issue-Trace/Issue-Step and trace_ids to confirm.

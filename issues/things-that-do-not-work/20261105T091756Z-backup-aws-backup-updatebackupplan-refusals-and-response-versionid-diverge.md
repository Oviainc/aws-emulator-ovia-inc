Issue-Source: trace-sweep-divergence
Issue-Trace: backup-rbx-traj-update-plan-refusals
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-rbx-traj-update-plan-refusals backup-rbx-traj-update-adds-cross-region-copy-action
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|UpdateBackupPlan|4xx->2xx|$.Code
Issue-Key: backup|UpdateBackupPlan|4xx->2xx|$.Message
Issue-Key: backup|UpdateBackupPlan|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|UpdateBackupPlan|4xx->2xx|status
Issue-Key: backup|UpdateBackupPlan|4xx->4xx|$.Code
Issue-Key: backup|UpdateBackupPlan|4xx->4xx|$.Message
Issue-Key: backup|UpdateBackupPlan|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|UpdateBackupPlan|2xx->2xx|$.VersionId
Issue-Commit: 717842165548985788beee9e72dfb452182d2228

# AWS Backup UpdateBackupPlan refusals and response VersionId diverge: vault validation missing, Code omitted, wrong error class and wording (4 causes)

> _Note: this issue was identified against the code base at commit `717842165548985788beee9e72dfb452182d2228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateBackupPlan

## Description
Four net-new wire divergences, all on AWS Backup's UpdateBackupPlan. Three are refusal-fidelity gaps (one request AWS rejects is accepted outright; two rejections carry the emulator's own error class, no Code member and the emulator's own sentence) and one is the shape of the VersionId the successful response returns. The open issues 20261103T061309Z-backup-aws-backup-plan-and-selection-read-bodies-diverge.md (GetBackupPlan's VersionId shape and a refused update being persisted) and 20261103T061309Z-backup-aws-backup-plan-selection-and-vault-refusals-diverge.md (CreateBackupPlan/CreateBackupSelection refusals) cover neighbouring plan gaps but carry none of the four replay keys below, and 20261029T193050Z-backup-aws-backup-plan-state-and-response-bodies-diverge.md owns the CreateBackupPlan VersionId cluster, not the UpdateBackupPlan one. FOUR causes must all be closed before this issue is submitted.

Cause 1 of 4 - UpdateBackupPlan accepts a rule whose target backup vault does not exist, returning 200 instead of refusing (fixture backup-rbx-traj-update-plan-refusals, step task#5).
AWS: Smithy backup#UpdateBackupPlan takes BackupPlanId (httpLabel, required) and BackupPlan (BackupPlanInput, required) and models InvalidParameterValueException (client, httpError 400) among its errors; that shape carries Code, Message, Type and Context members. AWS resolves each rule's target vault before committing the new plan version and, when the named vault does not exist in the caller's account and Region, refuses the whole update. Recorded response: HTTP 400, x-amzn-errortype 'InvalidParameterValueException', $.Code 'ERROR_3201', $.Message 'A Backup vault does not exist'. That message is entirely fixed text - it interpolates nothing from the request - and the Code is the fixed sentinel for this error class.
Emulator: answers HTTP 200 with a normal UpdateBackupPlanOutput, so no x-amzn-errortype, no $.Code and no $.Message are emitted at all, and the unresolvable rule is committed as a new plan version.
Why: a client that mistypes or deletes a target vault and relies on AWS to reject the update believes the plan now backs up to a vault that does not exist. Every subsequent read of the plan reports a configuration AWS would never hold, and the client's error-handling branch for ERROR_3201 is dead code here.
Fix: UpdateBackupPlan's validation in the backup crate - resolve every rule's TargetBackupVaultName against vault state and refuse with InvalidParameterValueException/ERROR_3201 before the state mutation, not after.
Traces: backup-rbx-traj-update-plan-refusals#task#5.

Cause 2 of 4 - UpdateBackupPlan on a plan that does not exist (or was deleted) omits $.Code and uses the emulator's own not-found sentence (fixture backup-rbx-traj-update-plan-refusals, step task#3).
AWS: Smithy backup#UpdateBackupPlan models ResourceNotFoundException (client, httpError 400, doc "A resource that is required for the action doesn't exist"), whose members include Code and Message. Recorded response: HTTP 400 with $.Code 'ERROR_2204' and $.Message 'Cannot update the Backup plan. Backup plan does not exist or has been deleted'. The whole sentence is fixed text for this error class; it names no plan id, so there is nothing per-run in it.
Emulator: the status and the error class are right (400), but $.Code is absent entirely and $.Message is the emulator's own shorter wording 'Cannot find Backup plan'.
Why: AWS Backup clients routinely branch on the numeric Code sentinel rather than on prose, and the generic 'Cannot find Backup plan' is also used by this crate for other plan paths, so a client cannot distinguish 'the plan is gone' from the other plan not-found conditions, and any Code-keyed retry/cleanup logic sees no Code to key on.
Fix: the UpdateBackupPlan handler's not-found path in the backup crate - populate ResourceNotFoundException.Code with ERROR_2204 and emit AWS's fixed sentence.
Traces: backup-rbx-traj-update-plan-refusals#task#3.

Cause 3 of 4 - UpdateBackupPlan with an empty Rules list is refused as the wrong error class, with no $.Code and the emulator's own wording (fixture backup-rbx-traj-update-plan-refusals, step task#4).
AWS: Smithy backup#BackupPlanInput.Rules (target BackupRulesInput) is @required, and UpdateBackupPlan models MissingParameterValueException (client, httpError 400, doc 'Indicates that a required parameter is missing') alongside InvalidParameterValueException. A plan document that supplies Rules as an empty list is treated as the required parameter being absent, not as an out-of-range value. Recorded response: HTTP 400, x-amzn-errortype 'MissingParameterValueException', $.Code 'ERROR_4202', $.Message 'No rules are set' - again wholly fixed text.
Emulator: HTTP 400 but x-amzn-errortype 'InvalidParameterValueException', no $.Code, and $.Message 'Rules must contain at least one backup rule' - the emulator's own sentence.
Why: the error class is part of the wire contract - an SDK deserialises MissingParameterValueException into a different modelled exception type than InvalidParameterValueException, so a client catching the missing-parameter type never catches this, and a client catching the invalid-value type catches something AWS would not raise. With no Code and non-AWS prose there is nothing left to disambiguate on.
Fix: UpdateBackupPlan's input validation in the backup crate - classify an empty/absent Rules as MissingParameterValueException with Code ERROR_4202 and AWS's message.
Traces: backup-rbx-traj-update-plan-refusals#task#4.

Cause 4 of 4 - UpdateBackupPlan's successful response returns a VersionId in a shape AWS never emits (fixture backup-rbx-traj-update-adds-cross-region-copy-action, step task#3).
AWS: Smithy backup#UpdateBackupPlanOutput.VersionId is a service-generated opaque string ('unique, randomly generated, Unicode, UTF-8 encoded strings'); the recording asserts it by shape only - a long random URL-safe token - because its characters are per-run.
Emulator: emits a UUID-like, monotonically counted value of the form '<hex>-<hex>-<hex>-0000-00000000000N' (one recorded instance: '18e51b5f-48ca-d23c-0000-000000000005'), which fails the shape assertion. The concrete digits are per-run and are not the requirement; the token shape is. The sibling GetBackupPlan/CreateBackupPlan clusters for the same generator are already owned by 20261103T061309Z-backup-aws-backup-plan-and-selection-read-bodies-diverge.md and 20261029T193050Z-backup-aws-backup-plan-state-and-response-bodies-diverge.md, but neither carries this UpdateBackupPlan key.
Why: anything that validates, stores in a fixed-width column, or round-trips VersionId against AWS's token shape rejects the emulator's value, and the embedded sequence counter leaks ordering a client can wrongly depend on.
Fix: the plan-version identifier generator in the backup crate's state (shared by Create/Update/Get), so UpdateBackupPlan's serializer returns an AWS-shaped opaque token.
Traces: backup-rbx-traj-update-adds-cross-region-copy-action#task#3 (also backup-rbx-traj-update-plan-refusals#task#6 and #task#7).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up true, /__admin/health 200, data listener accepting). Four divergence clusters, none carrying stub_path or owned_by:

1) backup-rbx-traj-update-plan-refusals#task#5 - UpdateBackupPlan, status expected 400, got 200. $.Code missing (expected 'ERROR_3201', got None); $.Message missing (expected 'A Backup vault does not exist', got None); $headers.x-amzn-errortype missing (expected 'InvalidParameterValueException', got None). divergence_key service=backup|UpdateBackupPlan|status=400->200|keys=$.Code,$.Message,$headers.x-amzn-errortype,status.

2) backup-rbx-traj-update-plan-refusals#task#3 - UpdateBackupPlan, 400->400. $.Code missing (expected 'ERROR_2204', got None); $.Message value_mismatch (expected 'Cannot update the Backup plan. Backup plan does not exist or has been deleted', got 'Cannot find Backup plan'). divergence_key service=backup|UpdateBackupPlan|status=400->400|keys=$.Code,$.Message.

3) backup-rbx-traj-update-plan-refusals#task#4 - UpdateBackupPlan, 400->400. $.Code missing (expected 'ERROR_4202', got None); $.Message value_mismatch (expected 'No rules are set', got 'Rules must contain at least one backup rule'); $headers.x-amzn-errortype shape_mismatch (expected 'MissingParameterValueException', got 'InvalidParameterValueException'). divergence_key service=backup|UpdateBackupPlan|status=400->400|keys=$.Code,$.Message,$headers.x-amzn-errortype.

4) backup-rbx-traj-update-adds-cross-region-copy-action#task#3 - UpdateBackupPlan, 200->200. $.VersionId value_mismatch (expected '<shaped-not-asserted>', got '18e51b5f-48ca-d23c-0000-000000000005'). Cluster also affects backup-rbx-traj-update-plan-refusals#task#6 and backup-rbx-traj-update-plan-refusals#task#7. divergence_key service=backup|UpdateBackupPlan|status=200->200|keys=$.VersionId.

Model checked (services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json): backup#UpdateBackupPlan (POST /backup/plans/{BackupPlanId}, 200; errors InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException, ServiceUnavailableException), backup#UpdateBackupPlanOutput.VersionId, backup#BackupPlanInput.Rules (@required), backup#InvalidParameterValueException / #MissingParameterValueException / #ResourceNotFoundException (each with Code, Message, Type, Context; httpError 400).

## Steps to Reproduce
1. git_clone, then boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture backup-rbx-traj-update-plan-refusals and inspect step task#5 - UpdateBackupPlan whose rule targets a backup vault that was never created. Expect 400 InvalidParameterValueException with Code ERROR_3201 and Message 'A Backup vault does not exist'; observe 200 and a committed new plan version.
3. Cause 2: same fixture, step task#3 - UpdateBackupPlan against a BackupPlanId that does not exist. Expect 400 ResourceNotFoundException with Code ERROR_2204 and Message 'Cannot update the Backup plan. Backup plan does not exist or has been deleted'; observe 400 with no Code and Message 'Cannot find Backup plan'.
4. Cause 3: same fixture, step task#4 - UpdateBackupPlan with BackupPlan.Rules empty. Expect 400 MissingParameterValueException with Code ERROR_4202 and Message 'No rules are set'; observe 400 InvalidParameterValueException with no Code and Message 'Rules must contain at least one backup rule'.
5. Cause 4: replay fixture backup-rbx-traj-update-adds-cross-region-copy-action, step task#3 - a successful UpdateBackupPlan. Inspect VersionId in the 200 body: it is a UUID-with-counter value instead of an AWS-shaped long random opaque token. Same generator shows at backup-rbx-traj-update-plan-refusals#task#6 and #task#7.
6. Re-replay all four fixture/step pairs after the fix; all four must pass.

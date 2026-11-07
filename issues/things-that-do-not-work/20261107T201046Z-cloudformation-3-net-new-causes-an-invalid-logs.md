Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cfn-topstacks-rolled-back-create-leaves-the-active-set cloudformation.deep-cloudformation-2-express-disable-rollback-conflict-rejected cloudformation.deletion-and-update-replace-policy-retain
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cfn-topstacks-rolled-back-create-leaves-the-active-set|_create#5|AssertionError|assert status == expect, (
Issue-Key: cloudformation.deep-cloudformation-2-express-disable-rollback-conflict-rejected|run#6|AssertionError|raise AssertionError(
Issue-Key: cloudformation.deletion-and-update-replace-policy-retain|run#21|AssertionError|assert _param_exists(ssm, rolling_v1), (
Issue-Commit: 47509ee38907103620e4b84df19d39bf76901d44

# cloudformation: 3 net-new causes — an invalid Logs RetentionInDays is accepted so a create never rolls back, conflicting DisableRollback values are accepted, and UpdateReplacePolicy:Retain destroys the replaced resource

> _Note: this issue was identified against the code base at commit `47509ee38907103620e4b84df19d39bf76901d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new root causes in the CloudFormation provisioning / update path, each grounded in one failing behavioral check that the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 3: an AWS::Logs::LogGroup whose RetentionInDays is not an allowed value is provisioned instead of failing, so a create that must roll back reaches CREATE_COMPLETE.
AWS: Smithy (cloudwatch logs) PutRetentionPolicy takes retentionInDays and models InvalidParameterException (HTTP 400); the accepted values are the documented set 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653 — the value 2 is not among them, so the AWS::Logs::LogGroup resource fails to create. With rollback enabled (the default) the stack therefore ends ROLLBACK_COMPLETE, leaves the active ListStacks status filter, is listed by a ROLLBACK_COMPLETE filter, and still reports every declared logical resource as DELETE_COMPLETE; the sibling stack built from the same template with a legal retention value stays active.
Emulator: the log group is created, the stack reaches CREATE_COMPLETE, and it stays in the active set, so neither the rolled-back status nor the DELETE_COMPLETE resource summaries ever appear.
Why: a client that uses an intentionally-bad template (or hits a genuinely invalid property) is told the deployment succeeded. Nothing distinguishes a good deploy from a bad one, rollback is never exercised, and the stack-status filters report a stack that AWS would have excluded.
Fix: the AWS::Logs::LogGroup resource provider's property validation in the CloudFormation provisioner, and the create-failure/rollback path it must trigger.
Traces: cloudformation.cfn-topstacks-rolled-back-create-leaves-the-active-set. Open issue 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md describes CREATE_COMPLETE for logs/cloudwatch/ecs resources in general but does not carry this check, so it does not cover this cause.

Cause 2 of 3: CreateStack accepts conflicting top-level DisableRollback and DeploymentConfig.DisableRollback values and creates the stack.
AWS: Smithy: CreateStack input carries both DisableRollback (boolean) and DeploymentConfig, whose DisableRollback member is documented "Specifies whether to disable rollback of the stack if the stack operation fails. Default: false". When the two switches are set to DIFFERENT values the call is refused pre-flight with error code ValidationError (HTTP 400) and the message "Conflicting values for DisableRollback: the top-level DisableRollback and DeploymentConfig.DisableRollback must not be set to different values." — that sentence is entirely fixed text, it has no variable parts. The refusal has no effect: DescribeStacks for that stack name then answers ValidationError with "Stack with id <StackName> does not exist", whose only variable part is the stack name from the request. (Message and refusal: recorded behavior; the model states the two members but not the conflict rule.) With the two values AGREEING the same call is accepted.
Emulator: the call succeeds and returns a StackId, i.e. the conflicting request creates a stack.
Why: a client relying on the pre-flight refusal gets a real stack provisioned under rollback semantics it never consistently requested, and the ambiguity is resolved silently by the emulator rather than reported; code that catches ValidationError to correct its request instead has to clean up a stack it did not expect to exist.
Fix: CreateStack's request validation in the CloudFormation CreateStack handler (reject before any stack record is minted).
Traces: cloudformation.deep-cloudformation-2-express-disable-rollback-conflict-rejected.

Cause 3 of 3: UpdateReplacePolicy: Retain is ignored — a replacing update destroys the predecessor resource.
AWS: UpdateReplacePolicy is a template attribute (not modeled in Smithy; AWS CloudFormation UpdateReplacePolicy documentation). When an update changes a create-only property (here AWS::SSM::Parameter Name) the resource is REPLACED; with UpdateReplacePolicy: Retain the old physical resource survives the UPDATE_COMPLETE with its original property values while the stack tracks the new physical id, and UpdateReplacePolicy does NOT imply DeletionPolicy — a later DeleteStack still destroys the current resource but leaves the earlier retained one. DeletionPolicy: Retain is independent and keeps its resource past DeleteStack.
Emulator: after the replacing update reaches UPDATE_COMPLETE the replaced predecessor no longer exists — ssm:GetParameter on it answers ParameterNotFound.
Why: data a template explicitly marked to retain is deleted. The retention attribute exists precisely so a replacement does not destroy state, so a client following AWS's contract loses the only copy of that resource, unrecoverably.
Fix: the CloudFormation update/replacement path — the post-replacement cleanup step must honour the resource's UpdateReplacePolicy before deleting the old physical resource.
Traces: cloudformation.deletion-and-update-replace-policy-retain. Open issue 20261103T061815Z-cloudformation-ssm-provisioning-and-update-path-fn-if.md mentions Retain being ignored on rollback for cloudformation__ssm, but does not carry this check or the replacing-update path, so it does not cover this cause.

## Evidence
From the behavioral replay (all three checks listed as failing with neither an ALREADY FILED nor an OWNED marker):

Cause 1 — cloudformation.cfn-topstacks-rolled-back-create-leaves-the-active-set [mismatch]:
  AssertionError: stack sim2real-scn-topstk-<id>-bad reached 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE' -- read its stack events
  (the 'bad' stack declares AWS::Logs::LogGroup with RetentionInDays: 2, which AWS refuses; the control stack uses a legal value.)

Cause 2 — cloudformation.deep-cloudformation-2-express-disable-rollback-conflict-rejected [mismatch]:
  AssertionError: conflicting DisableRollback values were accepted and created stack 'arn:aws:cloudformation:<region>:<account>:stack/sim2real-behav-drconf-<id>/<uuid>'
  Request: CreateStack(StackName=..., TemplateBody=..., DeploymentConfig={"Mode": "EXPRESS", "DisableRollback": False}, DisableRollback=True).
  Expected: ClientError ValidationError, message "Conflicting values for DisableRollback: the top-level DisableRollback and DeploymentConfig.DisableRollback must not be set to different values."; Actual: HTTP 200 with a StackId.

Cause 3 — cloudformation.deletion-and-update-replace-policy-retain [mismatch]:
  AssertionError: UpdateReplacePolicy:Retain must leave the replaced resource /sim2real-behav-retain-roll1-<id> in place; it was destroyed
  (the template's "Rolling" AWS::SSM::Parameter carries UpdateReplacePolicy: Retain and the update changes its Name, forcing a replacement; the replacement exists, the predecessor does not.)

## Steps to Reproduce
Replay the behavioral suite and re-run the three checks in trace_ids.
Cause 1: CreateStack with a template containing AWS::CloudFormation::WaitConditionHandle resources plus an AWS::Logs::LogGroup with RetentionInDays: 2; poll DescribeStacks for a terminal status and compare with the same template using a legal retention value (expect ROLLBACK_COMPLETE vs CREATE_COMPLETE), then ListStacks with the active filter and with StackStatusFilter=[ROLLBACK_COMPLETE] and ListStackResources on the failed stack.
Cause 2: CreateStack(StackName=N, TemplateBody=T, DisableRollback=True, DeploymentConfig={"Mode":"EXPRESS","DisableRollback":False}) — expect ValidationError and no stack; then DescribeStacks(StackName=N) expecting ValidationError "Stack with id N does not exist". Repeat with both values True to confirm acceptance.
Cause 3: create a stack whose AWS::SSM::Parameter "Rolling" has UpdateReplacePolicy: Retain, then UpdateStack with a different Name for that parameter; after UPDATE_COMPLETE call ssm:GetParameter on the ORIGINAL name (must exist with its original value) and ListStackResources (PhysicalResourceId must be the new name).

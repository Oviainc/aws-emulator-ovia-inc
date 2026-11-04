Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.source-output-variable-resolves-into-the-downstream-object-key codepipeline.override-stage-condition-not-overridable-rejected codepipeline.job-output-variables-precision-preserved
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.source-output-variable-resolves-into-the-downstream-object-key|run#29|AssertionError|assert resolved.get("ObjectKey") == f"v/{version_id}/out.zip", (
Issue-Key: codepipeline.override-stage-condition-not-overridable-rejected|run#13|AssertionError|assert refusal == (
Issue-Key: codepipeline.job-output-variables-precision-preserved|_probe#21|Timeout|return cp.list_action_executions(
Issue-Commit: 3ba5c12746fc70a78eb107fd40203067191e0c4a

# CodePipeline: 3 net-new root causes — ListActionExecutions omits resolvedConfiguration, ConditionNotOverridableException names the execution id instead of the pipeline, and a Lambda-invoke action never settles

> _Note: this issue was identified against the code base at commit `3ba5c12746fc70a78eb107fd40203067191e0c4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListActionExecutions

## Description
THREE net-new root causes in the CodePipeline execution read path, the OverrideStageCondition refusal wording and the Lambda-invoke action dispatch. A fixer must close ALL THREE before submitting; each cause names its own failing check ids below.

None of the three failing checks grounding this issue is marked ALREADY FILED or OWNED in this replay, so no open issue carries their replay keys. Related but NOT covering: 20261103T173413Z-codepipeline-4-net-new-root-causes-conditionnotoverridableexception-names.md lists `codepipeline.override-stage-condition-not-overridable-rejected` and `codepipeline.job-output-variables-precision-preserved` among its trace ids, but it recorded BOTH only as poll-budget stalls (replay keys `…|_poll_until#10|Timeout` and `…|run#21|Timeout`); in THIS replay the first check gets all the way to the OverrideStageCondition call and fails on the message template (Cause 2 here), a different replay key, and the second now fails against the suite's own 60 s ceiling rather than that issue's recorded key (Cause 3 here). 20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md and 20261029T064353Z-codepipeline-execution-plane-an-explicitly-started-execution-of.md describe the same FAMILY as Cause 3 for other check ids. 20261102T084502Z-codepipeline-execution-read-path-5-net-new-root.md touches the execution read path but for triggerDetail/artifactRevisions/empty members, not for resolvedConfiguration. A fixer closing any of those must still close these three causes and re-replay the ids in trace_ids.

=== Cause 1 of 3: ListActionExecutions returns an action's ActionExecutionInput with NO resolvedConfiguration, even when the variable reference was in fact resolved ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models ListActionExecutions -> actionExecutionDetails[].input : ActionExecutionInput, whose member `resolvedConfiguration` (target ResolvedActionConfigurationMap) is documented "Configuration data for an action execution with all variable references replaced with their real values for the execution." So for every action whose declared `configuration` contains a #{Namespace.Variable} reference, the recorded action execution carries BOTH maps: `configuration` with the template as declared, and `resolvedConfiguration` with the same keys and the substituted values. For an S3 Deploy action declared with ObjectKey = "v/#{SourceVariables.VersionId}/out.zip", resolvedConfiguration.ObjectKey is the key the action actually wrote — the template with the source action's VersionId output variable substituted (variable parts: the namespace/variable value taken from the upstream action's outputVariables and from the request's declared template; fixed text: the literal parts of the declared template).
Emulator: the execution itself resolves the variable correctly — the delivered object really is under v/<versionId>/out.zip, the execution reaches Succeeded, and the Source action's output.outputVariables.VersionId matches S3's version id (all of those assertions PASS). Only the recorded input is incomplete: the Deploy action's `input.resolvedConfiguration` is absent, so the check reads `{}` where the substituted map belongs.
Why: resolvedConfiguration is the only API-visible record of what an action was actually run with. A client auditing or debugging a pipeline (or a deployment tool that diffs declared vs effective configuration) cannot tell whether a variable resolved, what it resolved to, or whether the placeholder shipped through unresolved — the response looks exactly like an action that had no resolved configuration at all.
Fix: the ActionExecutionInput serializer and the action-execution records it reads in the codepipeline crate's execution plane — persist the post-substitution configuration map alongside the declared one when the action is dispatched, and emit it as `resolvedConfiguration` from ListActionExecutions (the same member is read for the ProjectName of a CodeBuild Build action, so the fix must cover every action category, not just S3 Deploy).
Traces: codepipeline.source-output-variable-resolves-into-the-downstream-object-key.

=== Cause 2 of 3: OverrideStageCondition's ConditionNotOverridableException interpolates the pipelineExecutionId where AWS names the PIPELINE ===
AWS: Smithy models OverrideStageCondition(pipelineName, stageName, pipelineExecutionId, conditionType: BEFORE_ENTRY|ON_SUCCESS — all required) with ConditionNotOverridableException (client, HTTP 400, member `message`). Overriding a condition that cannot be overridden — here a beforeEntry condition with result SKIP whose rule did not hold, so the stage settled Skipped — is refused ConditionNotOverridableException with the message `The condition type <conditionType> in stage <stageName> cannot be overridden now. Either it contains no failed or in-progress rule, the execution '<pipelineName>' is already superseded, or the pipeline definition has changed since the rules(s) failed.` Fixed text: everything outside the three substitutions. Variable parts: conditionType and stageName from the request, and — recorded behavior — the PIPELINE NAME in the single-quoted "the execution '…'" slot, not the execution id.
Emulator: the error code (ConditionNotOverridableException, HTTP 400) and every word of the fixed text are right, but the single-quoted slot is filled with the request's pipelineExecutionId instead of the pipeline name.
Why: the message is part of the refusal contract. An operator log parser or test that extracts the pipeline name from AWS's wording reads a UUID instead and does not match; because the id is per-run, nothing keying off the message is reproducible.
Fix: the message construction for the not-overridable refusal in the OverrideStageCondition handler of the codepipeline crate — one fixed template filled with the request's conditionType, stageName and pipelineName.
Traces: codepipeline.override-stage-condition-not-overridable-rejected.

=== Cause 3 of 3: an AWS/Lambda invoke action is never driven to a terminal state, so the execution hangs and its outputVariables are never published ===
AWS: recorded behavior only (the model types the calls, not the timing). The service DRIVES the execution: after StartPipelineExecution the Lambda-invoke stage publishes a latestExecution in GetPipelineState within seconds, the function is invoked with the CodePipeline.job payload, its PutJobSuccessResult(outputVariables=…) completes the job, and ListActionExecutions reports that action terminal (Succeeded) with output.outputVariables byte-identical to what the worker reported — no numeric coercion of long integer/decimal strings and no mangling of non-ASCII text.
Emulator: the check exhausts the suite's 60 s ceiling (`Error: Check timed out after 60.0s`) while polling GetPipelineState for the InvokeStage to publish any latestExecution, then ListActionExecutions for that action to reach Succeeded/Failed. No state is ever observed, so the output-variable fidelity behind it is unobservable.
Why: the client-visible effect is a pipeline that hangs — a started execution never reaches the documented next state, so any consumer that waits for a Lambda-invoke action to settle (or reads the variables it produced) blocks forever.
Fix: the codepipeline crate's execution plane — the AWS/Lambda invoke action dispatch and its job-completion path, plus the state it publishes to GetPipelineState and ListActionExecutions (including passing the reported outputVariables through unchanged as strings).
Traces: codepipeline.job-output-variables-precision-preserved.

## Evidence
All strings below are verbatim from this replay's failing-check details. Bucket/pipeline names, S3 version ids and execution ids are generated per run: they FILL the templates and are not themselves requirements.

Cause 1 — codepipeline.source-output-variable-resolves-into-the-downstream-object-key [mismatch], at check_codepipeline_source_output_variable_resolves_into_the_downstream_object_key.py line 319:
  resolved = deploy_details[0].get("input", {}).get("resolvedConfiguration", {}) if deploy_details else {}
  assert resolved.get("ObjectKey") == f"v/{version_id}/out.zip", (
      f"the Deploy action's recorded resolvedConfiguration must hold the substituted key, not the template: {resolved!r}")
  AssertionError: the Deploy action's recorded resolvedConfiguration must hold the substituted key, not the template: {}
  Everything before it PASSED: the execution reached 'Succeeded'; the only object under v/ is exactly [f"v/{version_id}/out.zip"]; the Source action's output.outputVariables.VersionId equals the version id PutObject returned. So the substitution works — only the recorded resolvedConfiguration map is missing (empty dict, i.e. the member is absent from ActionExecutionInput).
  Pipeline shape: versioned artifact bucket, S3 Source action with namespace 'SourceVariables', S3 Deploy action declared with ObjectKey 'v/#{SourceVariables.VersionId}/out.zip'.
  Expected: actionExecutionDetails[Deploy].input.resolvedConfiguration == {'ObjectKey': 'v/<the consumed VersionId>/out.zip', …} alongside input.configuration holding the un-substituted template.

Cause 2 — codepipeline.override-stage-condition-not-overridable-rejected [mismatch], at check_codepipeline_override_stage_condition_not_overridable_rejected.py line 63 (the check's own comment: "AWS names the pipeline, not the execution id, in the 'execution ...' slot"):
  assert refusal == (
      "ConditionNotOverridableException",
      f"The condition type BEFORE_ENTRY in stage GateSkip cannot be overridden now. Either it contains no "
      f"failed or in-progress rule, the execution '{name}' is already superseded, or the "
      f"pipeline definition has changed since the rules(s) failed.")
  AssertionError: ('ConditionNotOverridableException', "The condition type BEFORE_ENTRY in stage GateSkip cannot be overridden now. Either it contains no failed or in-progress rule, the execution 'ba74c83c-67ef-41a5-8ddf-787e0d90201b' is already superseded, or the pipeline definition has changed since the rules(s) failed.")
  The error CODE matches and all fixed text matches; the quoted slot carries the pipelineExecutionId ('ba74c83c-…') where the pipeline name belongs. The stage reaching 'Skipped' (polled before the call) was observed in this run, so the condition state itself is correct.

Cause 3 — codepipeline.job-output-variables-precision-preserved [mismatch]:
  Error: Check timed out after 60.0s
  The check creates an artifact bucket + source zip + a Lambda function that calls cp.put_job_success_result(jobId=…, outputVariables=json.loads(UserParameters)), a pipeline role, and a pipeline whose 'InvokeStage' holds an AWS/Lambda invoke action; it then polls cp.get_pipeline_state(name=…) for InvokeStage to publish a latestExecution and cp.list_action_executions(pipelineName=…, filter={'pipelineExecutionId': exec_id}) for the 'InvokeLambda' action to reach Succeeded/Failed.
  Expected: the action settles Succeeded and lambda_detail['output']['outputVariables'] == {'BigInteger': '123456789012345678901234567890123456789', 'PreciseDecimal': '3.14159265358979323846264338327950288', 'UnicodeText': 'héllo-世界-café'} byte-identical.
  Observed: no latestExecution and no action execution before the ceiling.

Run context: codepipeline per-service pass rate in this replay 113/160 (71%), 47 failed; every other codepipeline failure in this run is marked OWNED by an existing open issue.

## Steps to Reproduce
Boot the emulator and point botocore at it: cp = boto3.client('codepipeline'), s3 = boto3.client('s3'), iam = boto3.client('iam'), lam = boto3.client('lambda').

Cause 1 (codepipeline.source-output-variable-resolves-into-the-downstream-object-key):
1. s3.create_bucket + put_bucket_versioning(Status='Enabled'); s3.put_object(Bucket=B, Key='source.zip', Body=<a zip>) and keep VersionId V. Create a pipeline role trusted by codepipeline.amazonaws.com with S3 read/write on B.
2. cp.create_pipeline with artifactStore {type:'S3', location:B}, a Source stage holding an AWS/S3 Source action (S3Bucket=B, S3ObjectKey='source.zip', PollForSourceChanges='false', namespace='SourceVariables', outputArtifacts=[{name:'SourceOutput'}]) and a Deploy stage holding an AWS/S3 Deploy action with configuration {BucketName: B, ObjectKey: 'v/#{SourceVariables.VersionId}/out.zip', Extract: 'false'} and inputArtifacts=[{name:'SourceOutput'}].
3. Let CreatePipeline's own execution run; poll cp.list_pipeline_executions / cp.get_pipeline_execution until it is 'Succeeded'. Confirm s3.list_objects_v2(Bucket=B, Prefix='v/') shows exactly 'v/<V>/out.zip'.
4. details = cp.list_action_executions(pipelineName=P, filter={'pipelineExecutionId': E})['actionExecutionDetails']; take the entry with actionName == 'Deploy'.
   EXPECT: entry['input']['resolvedConfiguration']['ObjectKey'] == 'v/<V>/out.zip' (and entry['input']['configuration']['ObjectKey'] still the un-substituted template).
   OBSERVE: 'resolvedConfiguration' absent from entry['input'] (reads as {}).
5. Same check for a CodeBuild Build action: entry['input']['resolvedConfiguration']['ProjectName'] must equal the configured project.

Cause 2 (codepipeline.override-stage-condition-not-overridable-rejected):
6. Create an artifact bucket with a source object and a pipeline role. cp.create_pipeline (pipelineType='V2') with an S3 Source stage (namespace 'SourceVars') and a stage 'GateSkip' carrying beforeEntry = {conditions: [{result: 'SKIP', rules: [{name: 'MismatchCheck', ruleTypeId: {category:'Rule', owner:'AWS', provider:'VariableCheck', version:'1'}, configuration: {Variable: '#{SourceVars.ETag}', Value: 'definitely-mismatched-value-xyz', Operator: 'EQ'}}]}]} plus a manual-approval action.
7. exec_id = cp.start_pipeline_execution(name=P)['pipelineExecutionId']; poll cp.get_pipeline_state until GateSkip's latestExecution.status == 'Skipped'.
8. cp.override_stage_condition(pipelineName=P, stageName='GateSkip', pipelineExecutionId=exec_id, conditionType='BEFORE_ENTRY').
   EXPECT: ConditionNotOverridableException (code already correct) with message exactly "The condition type BEFORE_ENTRY in stage GateSkip cannot be overridden now. Either it contains no failed or in-progress rule, the execution '<PIPELINE NAME>' is already superseded, or the pipeline definition has changed since the rules(s) failed."
   OBSERVE: the same sentence with the pipelineExecutionId in the quoted slot.
9. Repeat with conditionType='ON_SUCCESS' (a type the stage does not declare): same template, same substitution rule, and the stage must stay 'Skipped'.

Cause 3 (codepipeline.job-output-variables-precision-preserved):
10. Create a versioned artifact bucket with a source zip; lam.create_function for a handler that reads event['CodePipeline.job'], parses data.actionConfiguration.configuration.UserParameters as JSON and calls cp.put_job_success_result(jobId=…, outputVariables=that dict); create a pipeline role allowed to lambda:InvokeFunction it.
11. cp.create_pipeline with an S3 Source stage and an 'InvokeStage' holding an AWS/Lambda invoke action (configuration {FunctionName: F, UserParameters: json.dumps({'BigInteger': '123456789012345678901234567890123456789', 'PreciseDecimal': '3.14159265358979323846264338327950288', 'UnicodeText': 'héllo-世界-café'})}); cp.start_pipeline_execution(name=P).
12. Poll cp.get_pipeline_state(name=P) for the InvokeStage stageState to carry a latestExecution, read its pipelineExecutionId, then poll cp.list_action_executions(pipelineName=P, filter={'pipelineExecutionId': that id}) for the 'InvokeLambda' entry.
   EXPECT within seconds: the entry's status 'Succeeded' and output.outputVariables equal to the three strings above, byte-identical (no numeric coercion, unicode preserved).
   OBSERVE: no latestExecution and no action execution at all; the wait never completes.

Then re-replay the three check ids listed in trace_ids.

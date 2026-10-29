Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-advanced-prompt-optimization-jobs
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-advanced-prompt-optimization-jobs bedrock-automated-reasoning-policy-build-workflow bedrock-evaluation-job-lifecycle bedrock-foundation-model-agreement-and-access bedrock-model-invocation-jobs bedrock-model-invocation-logging-config
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|CreateAdvancedPromptOptimizationJob|4xx->2xx|$.message
Issue-Key: bedrock|CreateAdvancedPromptOptimizationJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock|CreateAdvancedPromptOptimizationJob|4xx->2xx|status
Issue-Key: bedrock|StartAutomatedReasoningPolicyBuildWorkflow|4xx->2xx|$.message
Issue-Key: bedrock|StartAutomatedReasoningPolicyBuildWorkflow|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock|StartAutomatedReasoningPolicyBuildWorkflow|4xx->2xx|status
Issue-Key: bedrock|ListAutomatedReasoningPolicyBuildWorkflows|2xx->2xx|$.automatedReasoningPolicyBuildWorkflowSummaries[*]
Issue-Key: bedrock|CreateEvaluationJob|4xx->2xx|$.message
Issue-Key: bedrock|CreateEvaluationJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock|CreateEvaluationJob|4xx->2xx|status
Issue-Key: bedrock|PutUseCaseForModelAccess|4xx->2xx|$
Issue-Key: bedrock|PutUseCaseForModelAccess|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock|PutUseCaseForModelAccess|4xx->2xx|status
Issue-Key: bedrock|GetUseCaseForModelAccess|4xx->2xx|$.message
Issue-Key: bedrock|GetUseCaseForModelAccess|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock|GetUseCaseForModelAccess|4xx->2xx|status
Issue-Key: bedrock|CreateModelInvocationJob|4xx->2xx|$.message
Issue-Key: bedrock|CreateModelInvocationJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock|CreateModelInvocationJob|4xx->2xx|status
Issue-Key: bedrock|PutModelInvocationLoggingConfiguration|4xx->2xx|$
Issue-Key: bedrock|PutModelInvocationLoggingConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock|PutModelInvocationLoggingConfiguration|4xx->2xx|status
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock create, start and put operations accept input AWS refuses, returning success instead of 400

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAdvancedPromptOptimizationJob

## Description
EIGHT causes, one shared fault: the control-plane write handlers do not run the pre-conditions AWS checks before admitting a request, so requests AWS refuses with 400 ValidationException are accepted with a 2xx and a resource is minted. Two of the eight are the downstream read/list that then reports the wrongly-created state.

Cause 1 of 8: CreateAdvancedPromptOptimizationJob with an inputConfig/outputConfig S3 location whose bucket does not exist.
AWS: 400 ValidationException. Message template (recorded behavior; the model only declares ValidationException httpError 400 for this op): 'Cannot access S3 location: <bucket>/<prefix>/. Error: The specified bucket does not exist (Service: S3, Status Code: 404, Request ID: <id>, Extended Request ID: <id>) (SDK Attempt Count: <n>)'. Fixed text: 'Cannot access S3 location: ', '. Error: The specified bucket does not exist (Service: S3, Status Code: 404, Request ID: ', ', Extended Request ID: ', ') (SDK Attempt Count: ', ')'. Variable: the bucket/prefix from the request and the per-run S3 request ids and attempt count.
Emulator: 200 with a jobArn; no message, no x-amzn-errortype.
Why: a client that relies on create-time S3 reachability validation gets a job that can never read its input, and discovers the misconfiguration only as a silently failing job (or never, since the emulator's jobs complete).
Fix: CreateAdvancedPromptOptimizationJob's handler pre-condition — resolve inputConfig/outputConfig S3 locations against the S3 emulator before admitting the job.
Traces: bedrock-advanced-prompt-optimization-jobs#task#4.

Cause 2 of 8: StartAutomatedReasoningPolicyBuildWorkflow with sourceContent that does not match the buildWorkflowType.
AWS: 400 ValidationException, message 'Invalid content for workflow type.' (fixed text, no variable part). Smithy: StartAutomatedReasoningPolicyBuildWorkflow takes buildWorkflowType as a required @httpLabel enum and sourceContent as the required @httpPayload union; ValidationException is modelled with httpError 400.
Emulator: 200 with policyArn + buildWorkflowId; the mismatched content is accepted.
Why: callers never learn their build source is wrong for the requested workflow type, and get a workflow id for a build that cannot be meaningful.
Fix: StartAutomatedReasoningPolicyBuildWorkflow validation — cross-check the sourceContent union variant against buildWorkflowType.
Traces: bedrock-automated-reasoning-policy-build-workflow#task#2.

Cause 3 of 8: ListAutomatedReasoningPolicyBuildWorkflows after the refusal in cause 2.
AWS: 200 with automatedReasoningPolicyBuildWorkflowSummaries empty — the rejected start created nothing.
Emulator: 200 but the list carries a summary for the workflow it should never have created (policyArn, buildWorkflowId, status SCHEDULED, buildWorkflowType, createdAt, updatedAt).
Why: the list view shows a workflow that does not exist on AWS, so any poll-until-done or count assertion built on it is wrong.
Fix: the build-workflow state store, once cause 2's validation refuses the start (nothing to insert).
Traces: bedrock-automated-reasoning-policy-build-workflow#task#3.

Cause 4 of 8: CreateEvaluationJob whose inferenceConfig names a model identifier that does not resolve.
AWS: 400 ValidationException, message 'The provided model identifier is invalid.' (fixed text, no variable part). Smithy: CreateEvaluationJob has http code 202 on success and models ValidationException httpError 400.
Emulator: 202 Accepted with a jobArn.
Why: an evaluation job is created for a nonexistent model; the caller's error path for a typo'd/unentitled model never runs.
Fix: CreateEvaluationJob validation — resolve every model identifier in inferenceConfig against the model catalogue before minting the job.
Traces: bedrock-evaluation-job-lifecycle#task#2.

Cause 5 of 8: PutUseCaseForModelAccess with a formData blob that is not a valid use-case form.
AWS: 400 ValidationException, message 'Invalid form data.' (fixed text). Smithy: PutUseCaseForModelAccess input formData is a required blob, @length 10..16384; success code is 201; ValidationException httpError 400 is modelled.
Emulator: 201 with an empty body; the junk form is stored.
Why: the model-access form is accepted in a state AWS rejects, so the whole agreement/access flow proceeds from an invalid form.
Fix: PutUseCaseForModelAccess validation of the formData payload.
Traces: bedrock-foundation-model-agreement-and-access#task#4.

Cause 6 of 8: GetUseCaseForModelAccess when no valid form has been submitted.
AWS: 404 ResourceNotFoundException, message 'You have not filled out the request form. Fill out the form before getting access.' (fixed text). Smithy: GetUseCaseForModelAccess models ResourceNotFoundException httpError 404 and its output formData is required.
Emulator: 200 (serving the junk form admitted in cause 5), no message, no x-amzn-errortype.
Why: callers that branch on 404 to drive the user through the form skip it entirely.
Fix: GetUseCaseForModelAccess handler — the not-filled-out pre-condition, plus the state fix from cause 5 so nothing invalid is stored.
Traces: bedrock-foundation-model-agreement-and-access#task#5.

Cause 7 of 8: CreateModelInvocationJob whose modelId does not resolve.
AWS: 400 ValidationException, message 'The provided model identifier is invalid.' (fixed text). Smithy: CreateModelInvocationJob modelId is required with a pattern that admits bare ids as well as ARNs; ValidationException httpError 400 is modelled.
Emulator: 200 with a jobArn.
Why: a batch inference job is minted against a model that cannot serve it; the caller's invalid-model path never executes.
Fix: CreateModelInvocationJob validation — resolve modelId against the catalogue/custom-model store.
Traces: bedrock-model-invocation-jobs#task#4.

Cause 8 of 8: PutModelInvocationLoggingConfiguration naming a CloudWatch log group / role the service cannot use.
AWS: 400 ValidationException. Message template (recorded behavior): 'Failed to validate permissions for log group: <logGroupName>, with role: <roleArn>. Verify the IAM role permissions are correct.' Fixed text: 'Failed to validate permissions for log group: ', ', with role: ', '. Verify the IAM role permissions are correct.'; variable: the log group name and role ARN from the request's loggingConfig. Smithy: PutModelInvocationLoggingConfiguration requires loggingConfig and models ValidationException httpError 400.
Emulator: 200 with an empty body; the unusable configuration is stored.
Why: logging is reported as configured while nothing can be delivered, and the caller never sees the permission error AWS raises at configuration time.
Fix: PutModelInvocationLoggingConfiguration validation — verify the named log group exists and the role can be assumed/write to it before storing.
Traces: bedrock-model-invocation-logging-config#task#4.

Related open issues that do NOT cover these steps (no replay key for them): 20261029T172312Z-bedrock-control-plane-is-over-permissive-enforced-guardrail.md, 20261029T172312Z-bedrock-control-plane-applies-no-authorization-gate-an.md and 20261025T164541Z-bedrock-automated-reasoning-policy-build-workflow-goes-scheduled-completed.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 — bedrock-advanced-prompt-optimization-jobs#task#4 CreateAdvancedPromptOptimizationJob: expected 400, got 200; $.message expected 'Cannot access S3 location: <bucket>/apo-input/. Error: The specified bucket does not exist (Service: S3, Status Code: 404, Request ID: ..., Extended Request ID: ...) (SDK Attempt Count: 1)', got None; $headers.x-amzn-errortype expected an error shape, got None.

Cause 2 — bedrock-automated-reasoning-policy-build-workflow#task#2 StartAutomatedReasoningPolicyBuildWorkflow: expected 400, got 200; $.message expected 'Invalid content for workflow type.', got None; x-amzn-errortype missing.

Cause 3 — bedrock-automated-reasoning-policy-build-workflow#task#3 ListAutomatedReasoningPolicyBuildWorkflows: expected 200 with no summaries, got 200 with $.automatedReasoningPolicyBuildWorkflowSummaries[0] = {policyArn, buildWorkflowId, status 'SCHEDULED', buildWorkflowType 'INGEST_CONTENT', createdAt, updatedAt} (reported as 'extra (expected None)').

Cause 4 — bedrock-evaluation-job-lifecycle#task#2 CreateEvaluationJob: expected 400, got 202; $.message expected 'The provided model identifier is invalid.', got None; x-amzn-errortype missing.

Cause 5 — bedrock-foundation-model-agreement-and-access#task#4 PutUseCaseForModelAccess: expected 400 with body {'message': 'Invalid form data.'}, got 201 with an empty body; x-amzn-errortype missing.

Cause 6 — bedrock-foundation-model-agreement-and-access#task#5 GetUseCaseForModelAccess: expected 404 with $.message 'You have not filled out the request form. Fill out the form before getting access.', got 200 with no message and no x-amzn-errortype.

Cause 7 — bedrock-model-invocation-jobs#task#4 CreateModelInvocationJob: expected 400, got 200; $.message expected 'The provided model identifier is invalid.', got None; x-amzn-errortype missing.

Cause 8 — bedrock-model-invocation-logging-config#task#4 PutModelInvocationLoggingConfiguration: expected 400 with body {'message': 'Failed to validate permissions for log group: <logGroupName>, with role: <roleArn>. Verify the IAM role permissions are correct.'}, got 200 with an empty body; x-amzn-errortype missing.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay bedrock-advanced-prompt-optimization-jobs#task#4; bedrock-automated-reasoning-policy-build-workflow#task#2 and #task#3; bedrock-evaluation-job-lifecycle#task#2; bedrock-foundation-model-agreement-and-access#task#4 and #task#5; bedrock-model-invocation-jobs#task#4; bedrock-model-invocation-logging-config#task#4.
3. Each write step returns 2xx where the recording has 400 (and, for cause 6, 200 where the recording has 404); cause 3 then lists a build workflow that should not exist.
4. Equivalently: CreateAdvancedPromptOptimizationJob pointing at a nonexistent S3 bucket; StartAutomatedReasoningPolicyBuildWorkflow with sourceContent of the wrong variant for buildWorkflowType; CreateEvaluationJob / CreateModelInvocationJob with an unresolvable model identifier; PutUseCaseForModelAccess with junk formData followed by GetUseCaseForModelAccess; PutModelInvocationLoggingConfiguration naming a nonexistent log group and role.

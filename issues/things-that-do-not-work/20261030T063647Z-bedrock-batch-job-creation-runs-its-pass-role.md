Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-evaluation-job-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-evaluation-job-lifecycle bedrock-model-invocation-jobs
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|CreateEvaluationJob|4xx->4xx|$.message
Issue-Key: bedrock|CreateEvaluationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|CreateEvaluationJob|4xx->4xx|status
Issue-Key: bedrock|CreateModelInvocationJob|4xx->4xx|$.message
Issue-Key: bedrock|CreateModelInvocationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|CreateModelInvocationJob|4xx->4xx|status
Issue-Commit: aa75f5d199f9816d8a358732bf51b7bd305abb42

# bedrock batch job creation runs its pass-role gate before model-identifier validation

> _Note: this issue was identified against the code base at commit `aa75f5d199f9816d8a358732bf51b7bd305abb42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEvaluationJob

## Description
TWO causes, one shared fault. Both of bedrock's batch-job create operations evaluate an IAM pass-role/cross-account authorization gate BEFORE they validate the model identifier carried in the request. AWS orders these the other way round: an un-resolvable model identifier is an input defect and is refused 400 ValidationException with the fixed text 'The provided model identifier is invalid.' before any authorization decision is reached. Because the emulator reverses the order and additionally fails the role check for a role AWS accepts, both operations answer 403 AccessDeniedException with the emulator-authored message 'Cross-account pass role is not allowed.'. The HTTP status, the error code (x-amzn-errortype) and the message all diverge in both causes.

Cause 1 of 2: CreateEvaluationJob called with an inferenceConfig model identifier that resolves to nothing in the account/region.
AWS: 400 ValidationException, body message 'The provided model identifier is invalid.' — the whole message is fixed text with no variable part; x-amzn-errortype names ValidationException. Smithy: CreateEvaluationJob (POST /evaluation-jobs, success 202) requires roleArn (pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$) and inferenceConfig (EvaluationInferenceConfig), and models ValidationException httpError 400 alongside AccessDeniedException httpError 403 and ResourceNotFoundException httpError 404; the modelled 403 covers authorization, not an unusable model identifier.
Emulator: 403 AccessDeniedException, x-amzn-errortype 'AccessDeniedException', message 'Cross-account pass role is not allowed.' — the pass-role gate fires first and short-circuits the response, so the model identifier is never examined.
Why: a client is told it has an IAM/trust problem when its request names a model that does not exist. 403 AccessDeniedException is not retryable and routes an operator to IAM policy and role trust editing, while the actual defect — a bad model id in the request body — is never surfaced anywhere in the response. Clients that branch on ValidationException to reject a job submission outright instead treat the call as a permissions failure and either escalate or give up on a request they could have fixed.
Fix: CreateEvaluationJob's validation, in the handler's input-validation phase — resolve the inference-config model identifier and raise ValidationException with AWS's fixed text on a miss, before the pass-role/authorization gate runs; and do not refuse a role AWS accepts.
Traces: bedrock-evaluation-job-lifecycle#task#2.

Cause 2 of 2: CreateModelInvocationJob called with a modelId that resolves to nothing in the account/region.
AWS: 400 ValidationException, body message 'The provided model identifier is invalid.' — fixed text, no variable part; x-amzn-errortype names ValidationException. Smithy: CreateModelInvocationJob (POST /model-invocation-job) requires modelId (ModelId, length 1..2048, pattern admitting a bare model id or a custom-model/foundation-model/inference-profile ARN) and roleArn, and models ValidationException httpError 400 as well as AccessDeniedException httpError 403 and ResourceNotFoundException httpError 404.
Emulator: 403 AccessDeniedException, x-amzn-errortype 'AccessDeniedException', message 'Cross-account pass role is not allowed.' — identical short-circuit to cause 1, so the modelId is never resolved.
Why: batch-inference submission tooling cannot tell 'your modelId is wrong, fix the request' from 'your role cannot be passed, fix IAM'. The wrong error class also changes retry behaviour: AccessDeniedException is terminal and sends the failure to a permissions owner rather than back to the caller that supplied the bad identifier.
Fix: CreateModelInvocationJob's validation, same ordering defect — resolve modelId against the model catalogue and raise ValidationException before the pass-role/authorization gate; the gate itself must not reject a role AWS admits.
Traces: bedrock-model-invocation-jobs#task#4.

Related open issue that does NOT cover these two steps (it carries no replay key for either): 20261029T211357Z-bedrock-answers-resourcenotfoundexception-or-accessdeniedexception-where-aws-answers.md describes the same inverted ordering for CreateModelCustomizationJob, GetImportedModel and CreateProvisionedModelThroughput, but neither CreateEvaluationJob nor CreateModelInvocationJob appears among its Issue-Keys, so these two paths stay broken when it is closed. 20261030T033702Z-bedrock-putmodelinvocationloggingconfiguration-refuses-an-unverifiable-cloudwatchconfig-with-403.md is the same 403-instead-of-400 family on an unrelated operation.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, score 53.3%).

Cause 1 — bedrock-evaluation-job-lifecycle#task#2 CreateEvaluationJob: status expected 400, got 403. $.message value_mismatch: expected 'The provided model identifier is invalid.', got 'Cross-account pass role is not allowed.'. $headers.x-amzn-errortype shape_mismatch: expected a ValidationException-shaped error type, got 'AccessDeniedException'. Divergence key service=bedrock|CreateEvaluationJob|status=400->403|keys=$.message,$headers.x-amzn-errortype,status. Not owned by any open issue and not stub-blocked.

Cause 2 — bedrock-model-invocation-jobs#task#4 CreateModelInvocationJob: status expected 400, got 403. $.message value_mismatch: expected 'The provided model identifier is invalid.', got 'Cross-account pass role is not allowed.'. $headers.x-amzn-errortype shape_mismatch: expected a ValidationException-shaped error type, got 'AccessDeniedException'. Divergence key service=bedrock|CreateModelInvocationJob|status=400->403|keys=$.message,$headers.x-amzn-errortype,status. Not owned by any open issue and not stub-blocked.

In both causes the expected message is entirely fixed text — there is no request- or state-derived substitution in it — so the comparison is not sensitive to generated names, ids, timestamps or request ids.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay fixture bedrock-evaluation-job-lifecycle step task#2 and fixture bedrock-model-invocation-jobs step task#4.
3. Both answer HTTP 403 with x-amzn-errortype 'AccessDeniedException' and body message 'Cross-account pass role is not allowed.', where the recording has HTTP 400 ValidationException with message 'The provided model identifier is invalid.'.
4. Equivalently, by hand: POST /evaluation-jobs (CreateEvaluationJob) with a valid roleArn and an inferenceConfig naming a model identifier that exists in no account/region, and POST /model-invocation-job (CreateModelInvocationJob) with a valid roleArn and a modelId that matches the modelled pattern but resolves to no model. Each should be refused 400 ValidationException naming the invalid model identifier; the emulator instead refuses 403 AccessDeniedException about the pass role.
5. Confirm ordering is the fault: supplying a same-account role does not change the outcome, because the identifier is still never resolved.

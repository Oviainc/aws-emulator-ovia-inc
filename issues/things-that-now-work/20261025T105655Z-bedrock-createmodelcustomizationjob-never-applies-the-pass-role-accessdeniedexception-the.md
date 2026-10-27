Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.model-customization-job-lifecycle bedrock.e2e-fine-tune-deploy-invoke-teardown
Issue-Key: bedrock.model-customization-job-lifecycle|run#12|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code} ({message!r})"
Issue-Key: bedrock.e2e-fine-tune-deploy-invoke-teardown|run#16|AssertionError|assert code == "AccessDeniedException", (code, message)
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# CreateModelCustomizationJob never applies the pass-role AccessDeniedException: the request is accepted, or refused with ResourceNotFoundException from base-model resolution

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModelCustomizationJob

## Description
REOPENS 20261024T175106Z-bedrock-createmodelcustomizationjob-stub.md: its fix left the call refused.

The scaffold stub for CreateModelCustomizationJob was closed as implemented, but the implemented operation still does not answer what the recordings show: one behavioral check now gets a refusal with the wrong error code from it, and the other gets an unconditional success. Both are the one root cause below; a fixer must close it for BOTH check ids in trace_ids.

=== Cause 1 of 1: CreateModelCustomizationJob performs no roleArn PassRole authorization decision, so the deterministic AccessDeniedException is never returned and base-model resolution answers in its place ===

AWS: Smithy (com.amazonaws.bedrock) models CreateModelCustomizationJob as POST /model-customization-jobs with success code 201 and errors AccessDeniedException (httpError 403), ConflictException (400), InternalServerException (500), ResourceNotFoundException (404), ServiceQuotaExceededException (400), ThrottlingException (429), TooManyTagsException (400), ValidationException (400). Required input members: jobName, customModelName, roleArn (RoleArn, @length max 2048, @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$), baseModelIdentifier (BaseModelIdentifier, @length 1..2048, pattern admitting a bare model id, a :foundation-model/ ARN or a :custom-model/ ARN), trainingDataConfig, outputDataConfig. AccessDeniedException carries a single `message` member and is documented "The request is denied because of missing access permissions."

The model does not state the ordering or the denial text, so for those: recorded behavior only. Two independent recordings against real AWS agree, and both were taken against a role created in the SAME account as the caller, carrying the AWS-documented bedrock.amazonaws.com trust statement including the aws:SourceAccount / aws:SourceArn confused-deputy conditions plus an S3 read/write policy for the training bucket. In both recordings the call is refused with HTTP 403 / error code AccessDeniedException, and the message is the specific cross-account-pass-role denial (recorded example, an example only: "Cross-account pass role is not allowed"). The refusal is deterministic — one recording retried the create for up to 300s and the other for 90s and saw the identical denial on every attempt, ruling out IAM propagation lag — and it is reached BEFORE baseModelIdentifier is resolved: one recording sends a bare catalog-reported FINE_TUNING model id and the other sends arn:aws:bedrock:<region>::foundation-model/amazon.titan-text-express-v1, and both get the same 403/AccessDeniedException rather than any model-resolution error. The refusal also leaves no effect: ListModelCustomizationJobs(nameContains=<jobName>) returns an empty modelCustomizationJobSummaries and ListCustomModels(nameContains=<customModelName>) returns an empty modelSummaries. Both recordings establish a positive control first (ListFoundationModels / Converse on an ACTIVE on-demand text model succeed), so the denial is specific to the model-customization PassRole step and is not the caller being denied Bedrock as a whole.

Emulator: no AccessDeniedException is ever produced for this operation, in either of two shapes.
  (a) With a baseModelIdentifier taken from the emulator's own ListFoundationModels catalog (an ACTIVE TEXT model advertising FINE_TUNING in customizationsSupported), the call SUCCEEDS — the check captured error code None and message None, i.e. a 2xx with a jobArn — so the request AWS refuses is accepted and a job is created.
  (b) With baseModelIdentifier = arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-text-express-v1, the call is refused with the WRONG error: code ResourceNotFoundException (modelled httpError 404) instead of AccessDeniedException (403). Recorded emulator message, an example: 'baseModelIdentifier arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-text-express-v1 does not name a base model in this account in us-west-2'. Base-model lookup is therefore running at the point where AWS's authorization decision has already answered.

Why: a client relying on AWS's behavior gets two different wrong answers from the same operation. In shape (a) a request that real AWS refuses with 403 is accepted, so a caller (or a least-privilege/negative-authorization test, or a CI gate asserting that un-passable roles are rejected) sees a model-customization job accepted that production would never start, and subsequently polls for a job and a custom model that AWS would never have created. In shape (b) the caller sees a 404 ResourceNotFoundException and will "fix" it by hunting for a different base model id, when the real contract is a 403 authorization refusal that no change of baseModelIdentifier can clear — the status class (404 vs 403), the error code, and the retryability all differ, so error-handling branches written against AWS take the wrong path.

Fix: the authorization/pass-role gate in front of CreateModelCustomizationJob's handler (the roleArn PassRole decision for the bedrock.amazonaws.com service principal), evaluated BEFORE the handler's baseModelIdentifier resolution, returning the modelled AccessDeniedException with HTTP 403 and leaving no job or custom-model state behind. The ordering fix belongs in the handler's request pipeline, not in the foundation-model catalog.

Related open issue, which does NOT cover this: 20261024T025841Z-bedrock-control-plane-no-iam-authorization-gate-guardrail.md names the missing identity authorization gate for DeleteModelInvocationLoggingConfiguration and CreateCustomModel; it does not name CreateModelCustomizationJob, its roleArn PassRole step, or the ordering against base-model resolution.

Traces: bedrock.model-customization-job-lifecycle (shape b), bedrock.e2e-fine-tune-deploy-invoke-teardown (shape a). Handler: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_customization_job.rs.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1), service bedrock 29/76 passed.

Cause 1, check bedrock.e2e-fine-tune-deploy-invoke-teardown [mismatch]:
  expected: HTTP 403, error code AccessDeniedException, message containing the cross-account-pass-role denial; then ListModelCustomizationJobs(nameContains=<jobName>) == [] and ListCustomModels(nameContains=<customModelName>) == []
  actual:   AssertionError: (None, None) at `assert code == "AccessDeniedException", (code, message)` — the create call raised no ClientError at all, so no error code and no message existed to assert on (the job was accepted)
  (positive control in the same check passed first: ListFoundationModels resolved an ACTIVE FINE_TUNING base model and Converse on an ACTIVE on-demand text model returned text, so Bedrock was reachable for this caller)

Cause 1, check bedrock.model-customization-job-lifecycle [mismatch]:
  expected: HTTP 403, error code AccessDeniedException, message containing the cross-account-pass-role denial
  actual:   AssertionError: expected AccessDeniedException, got ResourceNotFoundException ('baseModelIdentifier arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-text-express-v1 does not name a base model in this account in us-west-2')
  (positive control in the same check passed first: ListFoundationModels returned modelSummaries)

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): CreateModelCustomizationJob http POST /model-customization-jobs code 201; errors include AccessDeniedException httpError 403 and ResourceNotFoundException httpError 404; roleArn required with pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$.

## Steps to Reproduce
Boot the emulator from this checkout and replay the two named checks (suite simparity-behavioral-ga-prod-v1).

1. Create an S3 bucket and upload a JSONL training file (e.g. train.jsonl with 64 {"prompt","completion"} records).
2. IAM CreateRole with a trust policy allowing sts:AssumeRole for Principal Service bedrock.amazonaws.com, with Condition StringEquals aws:SourceAccount = <this account> and ArnEquals aws:SourceArn = arn:aws:bedrock:<region>:<account>:model-customization-job/*; PutRolePolicy granting s3:GetObject/s3:PutObject/s3:ListBucket on that bucket. Take the role ARN from GetRole.
3. Call CreateModelCustomizationJob with jobName, customModelName, roleArn = that role, trainingDataConfig.s3Uri = s3://<bucket>/train.jsonl, outputDataConfig.s3Uri = s3://<bucket>/output/, and baseModelIdentifier set two ways:
     (a) a bare model id taken from ListFoundationModels(byOutputModality=TEXT) whose modelLifecycle.status is ACTIVE and whose customizationsSupported contains FINE_TUNING — expect 403 AccessDeniedException; observe a 2xx success with a jobArn (replay bedrock.e2e-fine-tune-deploy-invoke-teardown);
     (b) arn:aws:bedrock:<region>::foundation-model/amazon.titan-text-express-v1 — expect 403 AccessDeniedException; observe 404 ResourceNotFoundException naming baseModelIdentifier (replay bedrock.model-customization-job-lifecycle).
4. In both cases confirm the effect-absent leg: ListModelCustomizationJobs(nameContains=<jobName>).modelCustomizationJobSummaries and ListCustomModels(nameContains=<customModelName>).modelSummaries must both be empty.

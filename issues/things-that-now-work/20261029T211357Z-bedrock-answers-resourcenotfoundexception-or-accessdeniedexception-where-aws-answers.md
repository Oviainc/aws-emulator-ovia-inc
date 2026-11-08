Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-model-customization-jobs
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-model-customization-jobs bedrock-model-import-jobs bedrock-provisioned-model-throughput-lifecycle
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetModelCustomizationJob|4xx->4xx|$.message
Issue-Key: bedrock|GetModelCustomizationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|GetModelCustomizationJob|4xx->4xx|status
Issue-Key: bedrock|StopModelCustomizationJob|4xx->4xx|$.message
Issue-Key: bedrock|StopModelCustomizationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|StopModelCustomizationJob|4xx->4xx|status
Issue-Key: bedrock|CreateModelCustomizationJob|4xx->4xx|$.message
Issue-Key: bedrock|CreateModelCustomizationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|CreateModelCustomizationJob|4xx->4xx|status
Issue-Key: bedrock|GetModelImportJob|4xx->4xx|$.message
Issue-Key: bedrock|GetModelImportJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|GetModelImportJob|4xx->4xx|status
Issue-Key: bedrock|GetImportedModel|4xx->4xx|$.message
Issue-Key: bedrock|GetImportedModel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|GetImportedModel|4xx->4xx|status
Issue-Key: bedrock|CreateProvisionedModelThroughput|4xx->4xx|$.message
Issue-Key: bedrock|CreateProvisionedModelThroughput|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|CreateProvisionedModelThroughput|4xx->4xx|status
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock answers ResourceNotFoundException or AccessDeniedException where AWS answers 400 ValidationException

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetModelCustomizationJob

## Description
SIX causes, one shared fault: the emulator resolves an identifier against its own state first and reports the lookup miss, where AWS classifies the same input as un-usable BEFORE lookup and answers 400 ValidationException. Two distinct AWS rules are being skipped: (a) an ARN whose region does not match the endpoint's region is a ValidationException, not a not-found; (b) an unresolvable model identifier is a ValidationException, not a not-found. The emulator additionally short-circuits one case to 403 AccessDeniedException. Status, error code AND message all diverge in every cause.

Cause 1 of 6: GetModelCustomizationJob with a job ARN naming a different region than the endpoint.
AWS: 400 ValidationException, message 'The provided ARN is invalid for the service region' (fixed text, no variable part). Smithy: GetModelCustomizationJob jobIdentifier is a pattern-constrained @httpLabel; errors are ValidationException (400) and ResourceNotFoundException (404).
Emulator: 404 ResourceNotFoundException, x-amzn-errortype 'ResourceNotFoundException', message 'no model customization job answers to <arn> in this account in <region>'.
Why: the region-mismatch class is indistinguishable from a genuine not-found, so clients that retry/relocate on a region error (or treat 404 as 'safe to create') take the wrong branch; the message is also emulator-invented prose.
Fix: GetModelCustomizationJob input validation — compare the ARN's region against the endpoint region and raise ValidationException before the state lookup.
Traces: bedrock-model-customization-jobs#task#2.

Cause 2 of 6: StopModelCustomizationJob with the same cross-region job ARN.
AWS: 400 ValidationException, message 'The provided ARN is invalid for the service region'.
Emulator: 404 ResourceNotFoundException with 'no model customization job answers to <arn> in this account in <region>'.
Why: as cause 1; a stop against a mis-regioned ARN looks like an already-gone job.
Fix: same pre-lookup region check, StopModelCustomizationJob handler.
Traces: bedrock-model-customization-jobs#task#3.

Cause 3 of 6: CreateModelCustomizationJob whose baseModelIdentifier does not resolve.
AWS: 400 ValidationException, message 'The provided model identifier is invalid.' (fixed text). Smithy: CreateModelCustomizationJob models ValidationException (400), AccessDeniedException (403) and ResourceNotFoundException (404) — the modelled 403 is for authorization, not for an unresolvable model.
Emulator: 403 AccessDeniedException, x-amzn-errortype 'AccessDeniedException', message 'Cross-account pass role is not allowed.' — a pass-role check fires before the model identifier is validated, so the caller is told about a role problem instead of the model.
Why: 403 is non-retryable and sends operators to IAM; the real defect (an unknown model id) is never surfaced. Validation ordering is also wrong: AWS validates the model identifier before the pass-role gate.
Fix: CreateModelCustomizationJob validation ordering — resolve baseModelIdentifier (ValidationException on miss) before the pass-role/authorization gate.
Traces: bedrock-model-customization-jobs#task#4.

Cause 4 of 6: GetModelImportJob with a job ARN naming a different region than the endpoint.
AWS: 400 ValidationException, message 'The provided ARN is invalid for the service region'.
Emulator: 404 ResourceNotFoundException with 'no model import job answers to <arn> in this account in <region>'.
Why: as cause 1.
Fix: GetModelImportJob input validation — pre-lookup region check.
Traces: bedrock-model-import-jobs#task#3.

Cause 5 of 6: GetImportedModel with a modelIdentifier that does not resolve.
AWS: 400 ValidationException, message 'The provided model identifier is invalid.' (fixed text).
Emulator: 404 ResourceNotFoundException with 'no imported model answers to <identifier> in this account in <region>'.
Why: callers distinguishing 'bad identifier' (fix the request) from 'not there yet' (poll) cannot; the message is emulator prose.
Fix: GetImportedModel input validation — classify an unresolvable identifier as ValidationException.
Traces: bedrock-model-import-jobs#task#5.

Cause 6 of 6: CreateProvisionedModelThroughput whose modelId does not resolve.
AWS: 400 ValidationException, message 'The provided model identifier is invalid.' (fixed text). Smithy: CreateProvisionedModelThroughput modelId is required, pattern-constrained (bare id or custom-model/foundation-model ARN); ValidationException (400) and ResourceNotFoundException (404) are both modelled; success code is 201.
Emulator: 404 ResourceNotFoundException, x-amzn-errortype 'ResourceNotFoundException', message 'modelId <value> does not name a model in this account'.
Why: provisioning tools branch on ValidationException to reject the request outright; a 404 reads as a transient/propagation issue and gets retried forever.
Fix: CreateProvisionedModelThroughput validation — raise ValidationException with AWS's fixed text for an unresolvable modelId.
Traces: bedrock-provisioned-model-throughput-lifecycle#task#2.

Related open issues that do NOT cover these steps (no replay key for them): 20261027T212745Z-bedrock-createmodelcustomizationjob-the-iam-passrole-gate-admits-a.md and 20261029T172312Z-bedrock-reopens-bedrock-getfoundationmodel-the-catalogue-does-not-resolve.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 — bedrock-model-customization-jobs#task#2 GetModelCustomizationJob: expected 400, got 404; $.message expected 'The provided ARN is invalid for the service region', got 'no model customization job answers to arn:aws:bedrock:<other-region>:<acct>:model-customization-job/<name> in this account in <endpoint-region>'; $headers.x-amzn-errortype shape_mismatch, got 'ResourceNotFoundException'.

Cause 2 — bedrock-model-customization-jobs#task#3 StopModelCustomizationJob: identical divergence (expected 400 / 'The provided ARN is invalid for the service region', got 404 / ResourceNotFoundException with the same emulator prose).

Cause 3 — bedrock-model-customization-jobs#task#4 CreateModelCustomizationJob: expected 400, got 403; $.message expected 'The provided model identifier is invalid.', got 'Cross-account pass role is not allowed.'; x-amzn-errortype got 'AccessDeniedException'.

Cause 4 — bedrock-model-import-jobs#task#3 GetModelImportJob: expected 400, got 404; $.message expected 'The provided ARN is invalid for the service region', got 'no model import job answers to arn:aws:bedrock:<other-region>:<acct>:model-import-job/<name> in this account in <endpoint-region>'; x-amzn-errortype got 'ResourceNotFoundException'.

Cause 5 — bedrock-model-import-jobs#task#5 GetImportedModel: expected 400, got 404; $.message expected 'The provided model identifier is invalid.', got 'no imported model answers to <identifier> in this account in <endpoint-region>'; x-amzn-errortype got 'ResourceNotFoundException'.

Cause 6 — bedrock-provisioned-model-throughput-lifecycle#task#2 CreateProvisionedModelThroughput: expected 400, got 404; $.message expected 'The provided model identifier is invalid.', got 'modelId <value> does not name a model in this account'; x-amzn-errortype got 'ResourceNotFoundException'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay bedrock-model-customization-jobs#task#2, #task#3 and #task#4; bedrock-model-import-jobs#task#3 and #task#5; bedrock-provisioned-model-throughput-lifecycle#task#2.
3. Each returns 404 (or 403 for CreateModelCustomizationJob) where the recording has 400 ValidationException, with an emulator-worded message.
4. Equivalently: call GetModelCustomizationJob / StopModelCustomizationJob / GetModelImportJob against a well-formed job ARN whose region differs from the endpoint region; call GetImportedModel and CreateProvisionedModelThroughput with a model identifier that resolves to nothing; call CreateModelCustomizationJob with an unresolvable baseModelIdentifier and a cross-account role.

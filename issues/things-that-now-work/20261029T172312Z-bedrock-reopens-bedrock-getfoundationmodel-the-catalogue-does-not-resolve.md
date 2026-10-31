Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.get-foundation-model-details bedrock.get-foundation-model-has-lifecycle bedrock.error-get-foundation-model-not-found bedrock.update-provisioned-throughput-concurrent-race-one-wins bedrock.model-invocation-job-stop-transitions bedrock.create-foundation-model-agreement-conflict
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetFoundationModel|ResourceNotFoundException
Issue-Key: bedrock.update-provisioned-throughput-concurrent-race-one-wins|run#3|AssertionError|assert code == "ValidationException", (
Issue-Key: bedrock.model-invocation-job-stop-transitions|run#10|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code} ({message!r})"
Issue-Key: bedrock.create-foundation-model-agreement-conflict|run#3|AssertionError|raise AssertionError(
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# REOPENS bedrock GetFoundationModel: the catalogue does not resolve modelled model ids — GetFoundationModel 404s a catalogued model, provisioned-throughput and batch paths 404 a modelled foundation-model ARN, and no marketplace model ever has an agreement offer (3 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFoundationModel

## Description
REOPENS 20261023T232022Z-bedrock-getfoundationmodel-stub.md: its fix left the call refused.

The emulator's foundation-model catalogue does not answer for model identifiers the service catalogues, so three different surfaces refuse valid requests. The open issue 20261023T232022Z-bedrock-catalogued-foundation-model-refused-as-invalid-responses.md owns the inference-side keys for the same model id and 20261024T175106Z-bedrock-the-account-enforced-guardrail-configuration-is-stored.md mentions ListFoundationModelAgreementOffers; neither carries the six replay keys below.

=== Cause 1 of 3: GetFoundationModel refuses a catalogued model id with 404 ResourceNotFoundException ===
AWS: Smithy (com.amazonaws.bedrock) GetFoundationModel = GET /foundation-models/{modelIdentifier}, success 200, readonly; modelIdentifier is required with @length 1..2048 and a pattern that admits both a bare id (<provider>.<model-name>[:<suffix>]) and a foundation-model ARN; output member modelDetails -> FoundationModelDetails (carrying modelId/modelArn/modelLifecycle.status and the input/output modality and inference-type collections); errors ResourceNotFoundException (httpError 404), ValidationException (400), AccessDeniedException (403). For a model the account's catalogue lists, the answer is 200 with modelDetails and a modelLifecycle.status.
Emulator: 404 ResourceNotFoundException "Could not resolve the model from the provided model identifier: <modelIdentifier>" for a model id that this environment's catalogue is expected to carry (the request is well-formed and matches the modelled pattern). Three checks — the details read, the lifecycle read, and the one that uses GetFoundationModel purely as a POSITIVE CONTROL before asserting the not-found path for a bogus id — all die on this refusal.
Why: GetFoundationModel is the discovery call every client makes before inference (to check modality support, inference types and lifecycle). With a catalogued id refused, callers cannot enumerate-then-describe, cannot detect a deprecated model from modelLifecycle, and — worse — the genuine ResourceNotFoundException for a model that really does not exist becomes indistinguishable from this false 404, so the service's own not-found semantics are unusable.
Fix: the catalogue/state backing GetFoundationModel's handler — the model set ListFoundationModels reports and the set GetFoundationModel can resolve must be the same, for both the bare-id and the ARN form of modelIdentifier.
Traces: bedrock.get-foundation-model-details, bedrock.get-foundation-model-has-lifecycle, bedrock.error-get-foundation-model-not-found.

=== Cause 2 of 3: the provisioned-throughput and batch-job paths refuse a modelled foundation-model ARN as not naming a model ===
AWS: Smithy CreateProvisionedModelThroughput and CreateModelInvocationJob both accept a modelId whose pattern admits arn:aws:bedrock:<region>::foundation-model/<provider>.<model>[:<suffix>]; both model ValidationException (400), ResourceNotFoundException (404) and AccessDeniedException (403). Recorded behavior: for a model that exists but is not eligible for the request, the refusal is NOT "this ARN names no model" — CreateProvisionedModelThroughput answers ValidationException (recorded identically for three unrelated model families: amazon.titan-text-express-v1, its Titan successor, and a Llama 3 model), and CreateModelInvocationJob answers 403 AccessDeniedException with a message containing "cross-account pass role" for the passed role.
Emulator: both paths answer 404 ResourceNotFoundException "modelId arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-text-express-v1 does not name a model in this account in us-west-2" — a well-formed, modelled foundation-model ARN is reported as naming no model, so the eligibility and authorization decisions behind it are never reached.
Why: a caller cannot tell "this model is not eligible for provisioned throughput / batch inference" (a ValidationException it must surface to the user) from "this model does not exist" (a typo), and a 404 on a correct ARN sends them to debug the ARN. The same catalogue gap as cause 1, surfacing in two write paths.
Fix: resolve modelId against the same foundation-model catalogue in CreateProvisionedModelThroughput's and CreateModelInvocationJob's handlers, and let the eligibility/authorization refusals AWS actually returns be reached.
Traces: bedrock.update-provisioned-throughput-concurrent-race-one-wins, bedrock.model-invocation-job-stop-transitions.

=== Cause 3 of 3: ListFoundationModelAgreementOffers reports no outstanding offer for any marketplace model ===
AWS: Smithy ListFoundationModelAgreementOffers = GET /list-foundation-model-agreement-offers/{modelId}, success 200, readonly, required httpLabel modelId, output members modelId (required) and offers (required list); errors ResourceNotFoundException (404), AccessDeniedException (403), ValidationException (400). Recorded behavior only: for a marketplace model that requires an explicit EULA acceptance and has not been agreed to, the read returns an outstanding offer carrying a freshly minted offerToken on every call (the invariant is the PRESENCE of an offer, never token equality), and that offer stays listed after a refused CreateFoundationModelAgreement.
Emulator: none of the six candidate marketplace models (anthropic.claude-v2, anthropic.claude-3-sonnet-20240229-v1:0, anthropic.claude-3-haiku-20240307-v1:0, meta.llama3-70b-instruct-v1:0, cohere.command-text-v14, ai21.j2-ultra-v1) reports an outstanding agreement offer, so the Agreement surface has no exercisable state at all: the check cannot obtain an offerToken and fails before CreateFoundationModelAgreement.
Why: the whole model-access/EULA workflow is unreachable — a client cannot discover that a model needs an agreement, cannot obtain an offerToken, and cannot exercise the AccessDeniedException CreateFoundationModelAgreement answers (its message names the Agreement API surface). Anything that gates inference on "has this account agreed to the model" diverges from production.
Fix: the marketplace/agreement state behind ListFoundationModelAgreementOffers' handler must carry an outstanding offer for the catalogue's agreement-requiring models and mint a fresh offerToken per read.
Traces: bedrock.create-foundation-model-agreement-conflict.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service bedrock 91/199 passed.

[error] bedrock.get-foundation-model-details / [error] bedrock.get-foundation-model-has-lifecycle / [error] bedrock.error-get-foundation-model-not-found
  request: GetFoundationModel(modelIdentifier='moonshotai.kimi-k2.5')  (well-formed, matches the modelled pattern)
  expected: 200 with modelDetails (including modelLifecycle.status) — in error-get-foundation-model-not-found this call is only the POSITIVE CONTROL before the bogus-id assertion
  actual:   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetFoundationModel operation: Could not resolve the model from the provided model identifier: moonshotai.kimi-k2.5

[mismatch] bedrock.update-provisioned-throughput-concurrent-race-one-wins
  request: CreateProvisionedModelThroughput on 'amazon.titan-text-express-v1' (and two other unrelated families)
  expected: ValidationException
  actual:   AssertionError: expected ValidationException creating a PT on 'amazon.titan-text-express-v1', got 'ResourceNotFoundException': modelId arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-text-express-v1 does not name a model in this account

[mismatch] bedrock.model-invocation-job-stop-transitions
  request: CreateModelInvocationJob(roleArn=<same-account bedrock-trusted role>, modelId='arn:aws:bedrock:<region>::foundation-model/amazon.titan-text-express-v1', input/output in s3://<bucket>/...)
  expected: AccessDeniedException whose message contains 'cross-account pass role'
  actual:   AssertionError: expected AccessDeniedException, got ResourceNotFoundException ('modelId arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-text-express-v1 does not name a model in this account in us-west-2')

[mismatch] bedrock.create-foundation-model-agreement-conflict
  AssertionError: no candidate marketplace model in this account/region currently has an outstanding agreement offer; the positive control (the read side of the Agreement surface) has nothing to exercise without one
  unreached: CreateFoundationModelAgreement(offerToken=..., modelId=...) expecting AccessDeniedException whose message names the Agreement API, and the two effect-absent reads (offer still listed, availability unchanged).

Regression linkage: 20261023T232022Z-bedrock-getfoundationmodel-stub.md is recorded as fixed (GetFoundationModel no longer answers NotImplemented) but the call is still refused — now 404 ResourceNotFoundException for a model the catalogue is expected to carry.

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): GetFoundationModel GET /foundation-models/{modelIdentifier} code 200 readonly, modelIdentifier required @length 1..2048 with a pattern admitting both the bare id and the foundation-model ARN, output modelDetails -> FoundationModelDetails, errors ResourceNotFoundException 404 / ValidationException 400 / AccessDeniedException 403; CreateModelInvocationJob modelId pattern admits arn:...::foundation-model/..., errors AccessDeniedException 403 / ValidationException 400 / ResourceNotFoundException 404; ListFoundationModelAgreementOffers GET /list-foundation-model-agreement-offers/{modelId} code 200 readonly, output offers required.

## Steps to Reproduce
Boot the emulator from this checkout and replay the six checks in trace_ids (suite simparity-behavioral-aws-v1).
1. ListFoundationModels, take any id it reports, then GetFoundationModel(modelIdentifier=<that id>) and GetFoundationModel(modelIdentifier='moonshotai.kimi-k2.5'). Expect 200 with modelDetails and modelLifecycle.status; observe 404 ResourceNotFoundException 'Could not resolve the model from the provided model identifier: <id>'. Only a genuinely absent id may 404.
2. CreateProvisionedModelThroughput(modelId='amazon.titan-text-express-v1', no commitment). Expect ValidationException; observe ResourceNotFoundException 'does not name a model in this account'.
3. CreateModelInvocationJob with modelId='arn:aws:bedrock:<region>::foundation-model/amazon.titan-text-express-v1' and a same-account bedrock-trusted roleArn. Expect 403 AccessDeniedException containing 'cross-account pass role'; observe the same 404.
4. ListFoundationModelAgreementOffers(modelId='anthropic.claude-v2') (and the other five candidates). Expect at least one to report an outstanding offer with a fresh offerToken; observe none. Then CreateFoundationModelAgreement(offerToken=<that token>, modelId=<that model>) expecting AccessDeniedException naming the Agreement API.

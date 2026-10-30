Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-custom-model-read-and-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-custom-model-read-and-errors
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|DeleteCustomModel|4xx->4xx|$.message
Issue-Key: bedrock|DeleteCustomModel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|DeleteCustomModel|4xx->4xx|status
Issue-Commit: 480adcc070dacf78f0a3f7c0ce91b93d0e563936

# bedrock DeleteCustomModel answers 404 ResourceNotFoundException for an unresolvable modelIdentifier where AWS answers 400 ValidationException

> _Note: this issue was identified against the code base at commit `480adcc070dacf78f0a3f7c0ce91b93d0e563936`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomModel

## Description
ONE cause.

Cause 1 of 1: DeleteCustomModel called with a modelIdentifier that does not resolve to a custom model.

AWS: 400 ValidationException with the fixed message text 'The provided model identifier is invalid.' (no variable part — the rejected identifier is NOT echoed). AWS classifies a model identifier that names no usable custom model as un-usable input BEFORE it performs the state lookup, so the request is refused as a validation error rather than reported as a lookup miss. Smithy (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice): DeleteCustomModel DELETE /custom-models/{modelIdentifier}, modelIdentifier is a required pattern-constrained @httpLabel (@length 1..2048, pattern permitting a bare id or a custom-model/foundation-model ARN); modelled errors are ValidationException (400), ResourceNotFoundException (404), ConflictException (400), AccessDeniedException (403), ThrottlingException (429), InternalServerException (500). The 400/ValidationException classification for this input is taken from the recorded response.

Emulator: 404 with x-amzn-errortype 'ResourceNotFoundException' and the emulator-invented message 'Custom model not found.' — the handler goes straight to the state lookup and reports the miss, so status (404 vs 400), error code (ResourceNotFoundException vs ValidationException) and message text all diverge.

Why: a client that branches on error class gets the wrong branch. ValidationException is a terminal 'fix the request' signal, while a 404 on a DELETE reads as 'already gone / eventually consistent' — so deletion tooling treats a malformed-or-unknown identifier as a successful tear-down (idempotent delete) instead of surfacing a bad request, and retry layers keep polling a 404 that will never change. The invented message text also breaks clients and tests that match AWS's fixed wording.

Fix: DeleteCustomModel's input validation in the bedrock service crate's custom-model handler — classify a modelIdentifier that resolves to no custom model as ValidationException (HTTP 400, x-amzn-errortype ValidationException) with AWS's fixed text 'The provided model identifier is invalid.', raised before the state lookup that currently produces the ResourceNotFoundException.

Traces: bedrock-custom-model-read-and-errors#task#4.

Related open issue that does NOT cover this step (it carries no replay key for it): 20261029T211357Z-bedrock-answers-resourcenotfoundexception-or-accessdeniedexception-where-aws-answers.md describes the same misclassification family for GetModelCustomizationJob / StopModelCustomizationJob / CreateModelCustomizationJob / GetModelImportJob / GetImportedModel / CreateProvisionedModelThroughput, but not DeleteCustomModel.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false).

Cause 1 — bedrock-custom-model-read-and-errors#task#4 DeleteCustomModel, verdict mismatch:
  status: expected 400, got 404
  $.message: expected 'The provided model identifier is invalid.' (entirely fixed text), got 'Custom model not found.'
  $headers.x-amzn-errortype: shape_mismatch — recording asserts a shaped error type, emulator returned 'ResourceNotFoundException' (AWS's class here is ValidationException).

No part of the expected message is request- or state-derived: AWS's wording for this error class is the constant sentence 'The provided model identifier is invalid.'. The emulator's 'Custom model not found.' is not AWS wording for any class.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture bedrock-custom-model-read-and-errors, step task#4 (DeleteCustomModel against a custom-model identifier that resolves to nothing in the account/region).
3. Observe 404 / x-amzn-errortype ResourceNotFoundException / message 'Custom model not found.' where the recording has 400 ValidationException with message 'The provided model identifier is invalid.'.
4. Equivalently: DELETE /custom-models/<identifier-that-names-no-custom-model> against the emulator's bedrock endpoint.

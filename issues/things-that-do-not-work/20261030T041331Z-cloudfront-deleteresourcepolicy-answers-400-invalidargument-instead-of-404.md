Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-resource-policy-missing-resource-error
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-resource-policy-missing-resource-error
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|DeleteResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DeleteResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteResourcePolicy|4xx->4xx|status
Issue-Commit: e7818f8c3a6adfb142c95bd82ae3863b54254e49

# CloudFront DeleteResourcePolicy answers 400 InvalidArgument instead of 404 EntityNotFound for an absent resource

> _Note: this issue was identified against the code base at commit `e7818f8c3a6adfb142c95bd82ae3863b54254e49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResourcePolicy

## Description
One cause.

Cause 1 of 1: DeleteResourcePolicy for a resource ARN that does not exist is refused with the wrong status, error code and message.

AWS: Smithy: DeleteResourcePolicy, input {ResourceArn: string (required)}, http POST /2020-05-31/delete-resource-policy, modeled errors AccessDenied(403), EntityNotFound(404), IllegalDelete(400), InvalidArgument(400), PreconditionFailed(412), UnsupportedOperation(400). EntityNotFound is the client error with httpError 404 and doc "The entity was not found." — it is the modeled outcome when the ResourceArn names no existing resource. The recorded AWS response is HTTP 404 with Error.Code 'EntityNotFound' and Error.Message "The specified resource doesn't exist. Update your request and try again." That message is entirely fixed text for this error class — it interpolates nothing from the request — so it must be reproduced verbatim.
Emulator: HTTP 400 with Error.Code 'InvalidArgument' and Error.Message "The specified resource type isn't valid. Specify a VPC origin and try again." The emulator rejects the ARN on a resource-type check before it ever looks the resource up, so a well-formed ARN for a supported resource type that simply does not exist is reported as a malformed argument.
Why: the status class is wrong (400 vs 404), so a client cannot distinguish "nothing to delete" from "your request is invalid". Delete-if-exists and teardown/idempotency logic keyed on 404/EntityNotFound sees a hard 400 client error instead and aborts the teardown, and retry policies that treat InvalidArgument as non-retryable-and-fatal surface a spurious failure for what AWS reports as a benign absence. The message also misdirects the caller toward a VPC-origin ARN that was never the problem.
Fix: the DeleteResourcePolicy handler's refusal ordering and validation — resolve the ResourceArn against state and raise the modeled EntityNotFound (404, Code 'EntityNotFound', fixed message "The specified resource doesn't exist. Update your request and try again.") when no such resource exists; reserve InvalidArgument for an ARN that is actually malformed or of an unsupported resource type. Related open issues that do NOT cover this step: 20261029T212223Z-cloudfront-refusal-codes-and-messages-diverge-from-aws.md and 20261029T212223Z-cloudfront-resolves-the-entity-before-validating-id-format.md.
Traces: cloudfront-resource-policy-missing-resource-error#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false). Net-new divergence cluster (no owned_by, no stub_path), service cloudfront, operation DeleteResourcePolicy, fixture cloudfront-resource-policy-missing-resource-error step task#3:

  status: expected 404, got 400
  $.ErrorResponse.Error.Code: expected 'EntityNotFound', got 'InvalidArgument'
  $.ErrorResponse.Error.Message: expected "The specified resource doesn't exist. Update your request and try again.", got "The specified resource type isn't valid. Specify a VPC origin and try again."

The expected message is fixed text with no variable parts for this error class. Model reference: Smithy DeleteResourcePolicy errors include EntityNotFound (errorKind client, httpError 404, doc "The entity was not found.") and InvalidArgument (client, httpError 400); input is ResourceArn (string, required); http POST /2020-05-31/delete-resource-policy.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay trace set simparity-wire-aws-v1, fixture cloudfront-resource-policy-missing-resource-error, step task#3.
3. That step calls POST /2020-05-31/delete-resource-policy with a ResourceArn for a resource that was never created.
4. Observe the emulator answers 400 / InvalidArgument / "The specified resource type isn't valid. Specify a VPC origin and try again." where AWS answers 404 / EntityNotFound / "The specified resource doesn't exist. Update your request and try again.".

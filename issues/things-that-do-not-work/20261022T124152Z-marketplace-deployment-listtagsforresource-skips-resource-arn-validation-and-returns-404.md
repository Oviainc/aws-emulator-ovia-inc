Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found
Issue-Shape-Id: com.amazonaws.marketplacedeployment#AWSMPDeploymentParametersService
Issue-Key: marketplace-deployment|ListTagsForResource|4xx->4xx|$.message
Issue-Key: marketplace-deployment|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: marketplace-deployment|ListTagsForResource|4xx->4xx|status
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# ListTagsForResource skips resource-ARN validation and returns 404 NotFoundException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Cause 1 of 1: AWS Marketplace Deployment Service rejects a syntactically unacceptable `resourceArn` on ListTagsForResource with a 400 ValidationException before any lookup of the resource; the emulator skips that input validation entirely, goes straight to a state lookup, misses, and answers 404.

AWS: Smithy (com.amazonaws.marketplacedeployment, AWSMPDeploymentParametersService) models ListTagsForResource as GET /tags/{resourceArn} with required @httpLabel member `resourceArn` and errors ValidationException (httpError 400, 'An error occurred during validation.', required members message + fieldName) and ResourceNotFoundException (httpError 404, 'The specified resource wasn't found.'). The recording shows the service validating the ARN's vendor/service namespace first: for an ARN that is not an aws-marketplace resource ARN it answers HTTP 400 with a ValidationException whose message names the offending input and constraint (recorded example message: 'Invalid input resource arn: expected vendor aws-marketplace'). ResourceNotFoundException/404 is reserved for an ARN that is well formed for this service but names no existing resource. The requirement here is the status (400) and the ValidationException error type/shape, plus that the message identifies the failing member and the expected vendor; the recorded message text itself is only an example.

Emulator: replay of the step returns HTTP 404 with x-amzn-errortype 'NotFoundException' and message 'Resource was not found' instead of HTTP 400 / ValidationException. Two defects are visible: (a) no ARN syntax/vendor validation is performed for the `resourceArn` path label, so every unacceptable ARN degrades into a miss in the tag store; (b) the error type emitted, 'NotFoundException', is not a shape bound to this operation at all — the modeled 404 shape is `ResourceNotFoundException` — so even the not-found path serialises an error name no SDK for this service models.

Why: an SDK client relying on AWS behavior branches on the error type. A caller that passes a malformed or wrong-vendor ARN gets ResourceNotFound-style semantics and will treat a permanent, caller-side input bug as 'resource does not exist yet' — retrying, creating, or silently continuing instead of surfacing a validation failure; the required ValidationException `fieldName` member that tells the caller which input was wrong is never produced. Independently, modelled error deserialisation keys off x-amzn-errortype, so 'NotFoundException' does not map to the generated ResourceNotFoundException and clients fall through to a generic/unmodeled service error even on the legitimate not-found path.

Fix: in the marketplace-deployment ListTagsForResource handler's input validation, validate the `resourceArn` path label before the tag-store lookup and raise ValidationException (400) with message naming the member and the expected aws-marketplace vendor and with `fieldName` set to 'resourceArn'; and correct the service's error serializer so the 404 path emits the modeled shape name `ResourceNotFoundException` in x-amzn-errortype.

Traces: marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found#task#1 (expected 400, got 404; $.message and $headers.x-amzn-errortype diverge).

## Evidence
Fixture marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found, step task#1, operation ListTagsForResource (verdict: mismatch).

Expected (recorded AWS response): HTTP 400, ValidationException, body message 'Invalid input resource arn: expected vendor aws-marketplace' (example text; the requirement is a 400 ValidationException naming the member and expected vendor).

Actual (emulator): HTTP 404, header x-amzn-errortype: 'NotFoundException', body message 'Resource was not found'.

Replay detail verbatim: "status: expected 400, got 404 | $.message: value_mismatch (expected 'Invalid input resource arn: expected vendor aws-marketplace', got 'Resource was not found') | $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'NotFoundException')".

Model cross-check: ListTagsForResource errors = AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). 'NotFoundException' is not among them.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found, step task#1: GET /tags/{resourceArn} against the marketplace-deployment endpoint with a resourceArn whose vendor/service namespace is not aws-marketplace.
3. Observe the emulator answers HTTP 404 with x-amzn-errortype 'NotFoundException' and message 'Resource was not found'.
4. Expected: HTTP 400 with a ValidationException (x-amzn-errortype ValidationException) whose message names resourceArn and the expected aws-marketplace vendor, and whose fieldName is 'resourceArn'.
5. Also verify that a well-formed aws-marketplace ARN naming no existing resource returns 404 with x-amzn-errortype 'ResourceNotFoundException', not 'NotFoundException'.

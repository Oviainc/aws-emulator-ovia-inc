Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-vpc-link-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-vpc-link-error-paths apigateway-vpc-link-negative-paths apigateway-tag-boundary-length-and-character-validation apigateway-tag-isolation-across-resource-types apigateway-tagging-error-paths apigateway-tag-operations-error-paths apigateway-tag-stage-inheritance-vs-direct-tag-precedence apigateway-stage-method-settings-duplicate-spelling apigateway-api-key-import-update-lifecycle apigateway-api-key-lifecycle-and-bulk-import apigateway-import-api-keys-csv apigateway-import-api-keys-csv-bulk-and-failonwarnings-validation apigateway-import-api-keys-csv-round-trip apigateway-import-api-keys-malformed-csv apigateway-error-paths apigateway-import-export-sdk-error-paths apigateway-import-put-export-sdk-error-paths apigateway-import-rest-api-and-update-via-put apigateway-import-rest-api-warnings-and-deploy apigateway-invalid-input-error-paths apigateway-rest-api-import-export-and-put apigateway-rest-api-import-export-put
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateVpcLink|4xx->4xx|$.message
Issue-Key: apigateway|DeleteVpcLink|4xx->4xx|$.message
Issue-Key: apigateway|UntagResource|2xx->4xx|status
Issue-Key: apigateway|CreateStage|4xx->4xx|$.message
Issue-Key: apigateway|CreateStage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateStage|4xx->4xx|status
Issue-Key: apigateway|DeleteRestApi|4xx->2xx|status
Issue-Key: apigateway|ImportApiKeys|4xx->4xx|$.message
Issue-Key: apigateway|ImportApiKeys|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|ImportRestApi|4xx->4xx|$.message
Issue-Key: apigateway|ImportRestApi|2xx->2xx|$.warnings[*]
Issue-Related: 20261029T200808Z-apigateway-api-gateway-usage-plans-and-api-keys-deletes.md
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway tag, VPC link, stage and import refusal contracts (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcLink

## Description
Validation and absent-resource contracts across the VPC-link, tagging, stage, delete and import operations answer the wrong status, the wrong error code or emulator-authored text.

Cause 1 of 10 - CreateVpcLink with a malformed target ARN. AWS: 400 BadRequestException `Invalid ARN. ARN is not well formed <arn>` (fixed prefix, the offending ARN echoed from the request). Emulator: `Invalid ARN specified in the request <arn>`. Fix: CreateVpcLink's ARN validation message. Trace: apigateway-vpc-link-error-paths#task#1 (2 steps).

Cause 2 of 10 - DeleteVpcLink with an unknown id. AWS: 404 NotFoundException `Invalid Vpc Link identifier specified` (that exact casing). Emulator: `Invalid VPC link identifier specified`. Fix: the vpc-link not-found message. Trace: apigateway-vpc-link-error-paths#task#4 (2 steps).

Cause 3 of 10 - UntagResource naming a tag key that is not present on the resource. AWS: 204 No Content - removing an absent key is a no-op success. Emulator: 404. Why: idempotent cleanup code (and CloudFormation-style teardown) fails against the emulator. Fix: UntagResource's key lookup. Trace: apigateway-tag-boundary-length-and-character-validation#task#6 (3 steps).

Cause 4 of 10 - TagResource with a tag key or value outside the allowed character set / length. AWS: 400 BadRequestException with the fixed text `Invalid tag key/value. Valid characters are letters, numbers, spaces, and the following: _ . : / = + - @. Tag key can be up to 128 characters. Tag value can be up to 256 characters.` (one message for every violation - no per-case wording, no echoed length). Emulator: `a tag key may be at most 128 characters, got 129`. Fix: the tagging validator's message. Trace: apigateway-tag-boundary-length-and-character-validation#task#8 (5 steps).

Cause 5 of 10 - CreateStage naming a deploymentId that does not exist. AWS: 400 BadRequestException `Deployment id does not exist` (fixed text). Emulator: 404 NotFoundException `Invalid Deployment identifier specified` - wrong status AND wrong error code. Why: a client retrying on 404-but-not-400 behaves differently. Fix: CreateStage's deployment validation. Trace: apigateway-tag-stage-inheritance-vs-direct-tag-precedence#task#9.

Cause 6 of 10 - DeleteRestApi for a REST API that has already been deleted. AWS: 404 (the API is gone). Emulator: 202. Why: delete is reported as having removed something that does not exist, so teardown cannot detect a missing API. Fix: DeleteRestApi's existence check. Trace: apigateway-stage-method-settings-duplicate-spelling#teardown#12.

Cause 7 of 10 - ImportApiKeys with a CSV header that declares no key column. AWS: 400 BadRequestException `Missing required column 'Key' ` - fixed text, including the trailing space. Emulator: `the CSV header line must declare the required key column`. Fix: ImportApiKeys' CSV header validation message. Trace: apigateway-api-key-import-update-lifecycle#task#7 (6 steps).

Cause 8 of 10 - ImportApiKeys with no body. AWS: 400 with x-amzn-errortype `ValidationException` and message `1 validation error detected: Value null at 'body' failed to satisfy constraint: Member must not be null` (Smithy: ImportApiKeys.body is @required @httpPayload, so this is the framework's required-member refusal, not a service error). Emulator: BadRequestException with its own CSV-header sentence. Why: a client matching on ValidationException never sees it. Fix: required-member validation must run before the handler's CSV parsing. Trace: apigateway-import-api-keys-csv-bulk-and-failonwarnings-validation#task#8.

Cause 9 of 10 - ImportRestApi with a body that declares neither `swagger` nor `openapi`. AWS: 400 BadRequestException `Unable to determine OAS/Swagger version` (fixed text). Emulator: `Invalid OAS input`. Fix: ImportRestApi's version detection message. Trace: apigateway-error-paths#task#7 (9 steps).

Cause 10 of 10 - ImportRestApi warnings. AWS returns, for a definition referencing a missing model, the warning `Reference to model '<modelName>' not found. Ignoring.` AND a second warning `Invalid format for model <contentType> for method <HTTPMETHOD>`. The emulator returns one warning worded `Unable to resolve reference '#/definitions/<modelName>'. Ignoring.` and omits the second. Why: a caller using failOnWarnings or surfacing warnings to a user sees different (and fewer) diagnostics. Fix: the import warning generator. Trace: apigateway-import-rest-api-warnings-and-deploy#task#1.

No open issue owns these clusters.

## Evidence
Cause 1: $.message expected 'Invalid ARN. ARN is not well formed <arn>', got 'Invalid ARN specified in the request <arn>'.
Cause 2: expected 'Invalid Vpc Link identifier specified', got 'Invalid VPC link identifier specified'.
Cause 3: status expected 204, got 404.
Cause 4: expected the fixed 'Invalid tag key/value. Valid characters are ...' message, got 'a tag key may be at most 128 characters, got 129'.
Cause 5: status expected 400 / BadRequestException 'Deployment id does not exist', got 404 / NotFoundException 'Invalid Deployment identifier specified'.
Cause 6: status expected 404, got 202.
Cause 7: expected "Missing required column 'Key' ", got 'the CSV header line must declare the required key column'.
Cause 8: x-amzn-errortype expected 'ValidationException' got 'BadRequestException'; message expected "1 validation error detected: Value null at 'body' failed to satisfy constraint: Member must not be null".
Cause 9: expected 'Unable to determine OAS/Swagger version', got 'Invalid OAS input'.
Cause 10: $.warnings[0] expected "Reference to model 'NonExistentModel' not found. Ignoring." got "Unable to resolve reference '#/definitions/NonExistentModel'. Ignoring."; $.warnings[1] 'Invalid format for model application/json for method GET' missing.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Cause 3/4: DELETE /tags/{arn}?tagKeys=absent-key (expect 204) and POST /tags/{arn} with a 129-character key (compare the message).
3. Cause 5: POST /restapis/{id}/stages with an unknown deploymentId (expect 400 BadRequestException).
4. Cause 6: DELETE the same REST API twice (expect 404 the second time).
5. Causes 7-10: POST /apikeys?mode=import with and without a body/key column, and POST /restapis?mode=import with a version-less and a dangling-$ref definition.

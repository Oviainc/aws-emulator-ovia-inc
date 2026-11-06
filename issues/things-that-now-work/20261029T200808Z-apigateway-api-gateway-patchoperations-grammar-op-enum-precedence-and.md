Issue-Source: trace-sweep-divergence
Issue-Trace: apigw-patch-op-enum-validation-vs-service
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigw-patch-op-enum-validation-vs-service apigw-patch-gateway-response-statuscode-op-support apigw-patch-integration-type-and-method-family apigateway-import-rest-api-and-update-via-put apigateway-stage-cache-cluster-flags-and-patch-op-grammar apigateway-stage-method-settings-path-grammar apigateway-stage-access-log-settings-asymmetric-patch apigateway-stage-method-settings-duplicate-spelling apigw-patch-deployment-stage-invalid-path
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateRestApi|4xx->4xx|$.message
Issue-Key: apigateway|UpdateGatewayResponse|4xx->4xx|$.message
Issue-Key: apigateway|UpdateIntegration|4xx->4xx|$.message
Issue-Key: apigateway|UpdateGatewayResponse|4xx->2xx|$.message
Issue-Key: apigateway|UpdateGatewayResponse|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|UpdateGatewayResponse|4xx->2xx|status
Issue-Key: apigateway|PutRestApi|4xx->4xx|$.message
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.__type
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.cacheClusterEnabled
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.cacheClusterStatus
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.createdDate
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.deploymentId
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.lastUpdatedDate
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.message
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.methodSettings
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.stageName
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.tracingEnabled
Issue-Key: apigateway|UpdateStage|2xx->4xx|status
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.cacheClusterSize
Issue-Key: apigateway|GetStage|2xx->2xx|$.methodSettings.*/*
Issue-Key: apigateway|GetStage|2xx->2xx|$.methodSettings.~1pets/GET
Issue-Key: apigateway|GetStage|2xx->2xx|$.methodSettings.~1pets~1GET
Issue-Key: apigateway|UpdateStage|4xx->2xx|$.message
Issue-Key: apigateway|UpdateStage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|UpdateStage|4xx->2xx|status
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway patchOperations grammar: op-enum, precedence and stage patch values (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRestApi

## Description
The emulator's JSON-Patch front end validates in the wrong order, words the op-enum refusal itself, and refuses patch values that AWS accepts (and accepts one it refuses). Smithy: the affected ops take ListOfPatchOperation and model BadRequestException @httpError 400; Stage.cacheClusterSize targets the CacheClusterSize enum [0.5, 1.6, 6.1, 13.5, 28.4, 58.2, 118, 237]; Stage.methodSettings is MapOfMethodSettings.

Cause 1 of 10 - UpdateRestApi, unsupported patch op. AWS: 400 BadRequestException `Invalid patch operation specified. Must be one of: [add, replace, remove]` (fixed text; the list is fixed). Emulator: "Invalid patch operation specified. Must be 'add'|'remove'|'replace'". Why: a client matching on AWS's wording never recognises the refusal. Fix: the patchOperations op validator's message. Trace: apigw-patch-op-enum-validation-vs-service#task#4.

Cause 2 of 10 - UpdateGatewayResponse, validation precedence. AWS checks the patch PATH before the op, so an unsupported op on an unsupported path is refused `Invalid patch path /statusCode`. Emulator answers the op-enum message instead. Fix: order the two checks path-first in the shared validator. Trace: apigw-patch-gateway-response-statuscode-op-support#task#5.

Cause 3 of 10 - UpdateIntegration, same precedence defect. AWS: `Invalid patch path /timeoutInMillis`; emulator: the op-enum message. Trace: apigw-patch-integration-type-and-method-family#task#9.

Cause 4 of 10 - UpdateGatewayResponse, /statusCode is not patchable. AWS: 400 BadRequestException `Invalid patch path /statusCode`. Emulator: 200, and it applies the patch. Why: a client can mutate a gateway response's status code through a path real AWS rejects, so emulator state has no AWS counterpart. Fix: UpdateGatewayResponse's allowed-path table. Trace: apigw-patch-gateway-response-statuscode-op-support#task#3.

Cause 5 of 10 - PutRestApi, `mode` enum validation message. AWS: 400 `1 validation error detected: Value 'append' at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [overwrite, merge]` - the member path is the wire member `mode` and the value set is listed in the order [overwrite, merge]. Emulator names `putRestApiInput.mode` and lists [merge, overwrite]. Fix: the request-validation layer's member-path and enum rendering for PutRestApi. Trace: apigateway-import-rest-api-and-update-via-put#task#6.

Cause 6 of 10 - UpdateStage, non-boolean /cacheClusterEnabled value. Recorded behavior only: AWS accepts the patch (200) and the resulting Stage reports cacheClusterEnabled=false, cacheClusterStatus='NOT_AVAILABLE'. Emulator refuses 400 BadRequestException with its own text about a boolean value. Why: a valid UpdateStage call fails, so the whole stage update is lost. Fix: UpdateStage's patch value coercion. Trace: apigateway-stage-cache-cluster-flags-and-patch-op-grammar#task#8.

Cause 7 of 10 - UpdateStage, /cacheClusterSize value outside the enum. Recorded behavior only: AWS accepts the patch (200) and stores the value verbatim (it is only validated when a cache cluster is actually provisioned), and a later GetStage echoes it. Emulator refuses 400 listing the enum. Fix: same patch value path; do not enum-check cacheClusterSize at patch time. Trace: apigateway-stage-cache-cluster-flags-and-patch-op-grammar#task#9.

Cause 8 of 10 - UpdateStage, method-settings path grammar. AWS accepts `/~1<resourcePath>/<httpMethod>/...` forms (200) and returns the updated Stage with its methodSettings map. Emulator refuses 400. Why: per-method throttling/caching cannot be configured at all. Fix: UpdateStage's method-settings path parser. Trace: apigateway-stage-method-settings-path-grammar#task#12.

Cause 9 of 10 - Stage.methodSettings key normalisation. AWS keys the map by `<resourcePath>/<httpMethod>` with `/` escaped as `~1` only in the patch path, and collapses the wildcard form to `*/*`; the recorded stage carries keys `*/*` and `~1pets/GET`. The emulator keeps two distinct keys `~1pets/GET` and `~1pets~1GET` and never produces `*/*`. Why: a client reading methodSettings cannot find the setting it just wrote. Fix: normalise the method-settings key when storing and when serialising Stage. Trace: apigateway-stage-method-settings-path-grammar#task#17.

Cause 10 of 10 - UpdateStage, remove of a method setting that does not exist. AWS: 400 BadRequestException `Cannot remove method setting accessLogSettings/format because there is no method setting for this method ` (fixed text with a trailing space; the variable part is the setting path from the request). Emulator: 200, silently a no-op. Fix: UpdateStage's remove-op handling. Trace: apigateway-stage-access-log-settings-asymmetric-patch#task#10.

No open issue owns these clusters (owned_by was empty for all ten); 'API Gateway control plane is over-permissive' covers different operations.

## Evidence
Cause 1: $.message expected 'Invalid patch operation specified. Must be one of: [add, replace, remove]', got "Invalid patch operation specified. Must be 'add'|'remove'|'replace'".
Cause 2/3: expected 'Invalid patch path /statusCode' resp. 'Invalid patch path /timeoutInMillis', got the op-enum message.
Cause 4: status expected 400, got 200; $.message expected 'Invalid patch path /statusCode', got none.
Cause 5: expected "...Value 'append' at 'mode' ... enum value set: [overwrite, merge]", got "...at 'putRestApiInput.mode' ... [merge, overwrite]".
Cause 6: status expected 200 (cacheClusterEnabled false, cacheClusterStatus NOT_AVAILABLE, methodSettings {}), got 400 with __type BadRequestException.
Cause 7: status expected 200 with cacheClusterSize echoed, got 400 listing the CacheClusterSize enum.
Cause 8: status expected 200 with methodSettings populated, got 400.
Cause 9: $.methodSettings expected keys '*/*' and '~1pets/GET', got '~1pets/GET' and '~1pets~1GET'.
Cause 10: status expected 400 with the quoted message, got 200.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Cause 1: PATCH /restapis/{id} with an op outside [add, replace, remove] (apigw-patch-op-enum-validation-vs-service#task#4).
3. Causes 6-8: PATCH /restapis/{id}/stages/{stage} with /cacheClusterEnabled="maybe", /cacheClusterSize="3" and a ~1pets/GET method-setting path (apigateway-stage-cache-cluster-flags-and-patch-op-grammar, apigateway-stage-method-settings-path-grammar).
4. Compare status, x-amzn-errortype and $.message with the expectations above.

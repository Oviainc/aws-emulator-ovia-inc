Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|UpdateStage|BadRequestException
Issue-Key: apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration|run#49|Timeout|time.sleep(5.0)
Issue-Key: apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it|_poll_http#30|Timeout|time.sleep(interval_s)
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# apigateway: 3 net-new causes — UpdateStage refuses a valid unescaped method-setting patch path, and two execute-api stage checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Three net-new root causes in the apigateway emulator, grounded in three failing behavioral checks. A fixer must close ALL THREE before submitting.

=== Cause 1 of 3: UpdateStage refuses a method-setting patch path whose resource half is spelled without the `~1` escape ===

AWS: Smithy `UpdateStage` = PATCH /restapis/{restApiId}/stages/{stageName}, input member `patchOperations` (ListOfPatchOperation), output shape `Stage`, success 200; its modeled client errors are BadRequestException (400), ConflictException (409), LimitExceededException (429), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401). `Stage$methodSettings` documents the key grammar: "Keys (designated as /{method_setting_key} below) are method paths defined as {resource_path}/{http_method} for an individual method override, or /*/* for overriding all methods in the stage." Per-method cache settings are written through the `/{resourcePath}/{httpMethod}/caching/enabled` and `/{resourcePath}/{httpMethod}/caching/ttlInSeconds` patch paths. Real API Gateway accepts the method path spelled with the resource path as a plain reference token (the resource path's leading slash supplied by the pointer separator) as well as the `~1`-escaped spelling; the recording this check is derived from gets a 200 Stage back for a patch list whose paths are `/{resource}/{VERB}/caching/enabled` and `/{resource}/{VERB}/caching/ttlInSeconds`, and the stage then serves a cached body until FlushStageCache (DELETE /restapis/{restApiId}/stages/{stageName}/cache/data, 202) invalidates it.

Emulator: the same UpdateStage call is refused 400 BadRequestException with the message `Invalid method setting path: <path>. Must be one of: [/deploymentId, /description, /cacheClusterEnabled, /cacheClusterSize, /clientCertificateId, /accessLogSettings, /{resourcePath}/{httpMethod}/metrics/enabled, /{resourcePath}/{httpMethod}/logging/dataTrace, /{resourcePath}/{httpMethod}/logging/loglevel, /{resourcePath}/{httpMethod}/throttling/burstLimit, /{resourcePath}/{httpMethod}/throttling/rateLimit, /{resourcePath}/{httpMethod}/caching/ttlInSeconds, /{resourcePath}/{httpMethod}/caching/enabled, /{resourcePath}/{httpMethod}/caching/dataEncrypted, /{resourcePath}/{httpMethod}/caching/requireAuthorizationForCacheControl, /{resourcePath}/{httpMethod}/caching/unauthorizedCacheControlHeaderStrategy, /variables/{variable_name}, /documentationVersion, /canarySettings/percentTraffic, /canarySettings/deploymentId, /canarySettings/stageVariableOverrides/{variable_name}, /canarySettings/useStageCache, /tracingEnabled]`. In that message the fixed text is `Invalid method setting path: ` … `. Must be one of: [` + the enumerated path grammar + `]`; the only variable part is the rejected path copied from the request. Note the enumeration itself advertises `/{resourcePath}/{httpMethod}/caching/enabled`, which is exactly the shape of the path being refused — the refusal contradicts its own message. Root cause: the method-setting path parser only treats the first reference token as a resource path when, after `~1` decoding, it begins with `/` (`names_a_resource` in the UpdateStage handler), so a token that names the resource without the escaped leading slash resolves to no method and falls through to the generic invalid-path refusal.

Why: a client enabling per-method stage caching (the only way to turn on the stage cache for a method) gets a 400 for a patch real AWS applies, so stage caching can never be configured, and every downstream behavior that depends on it (a cached body served until FlushStageCache, cacheTtlInSeconds, cache key parameters) is unreachable. The contradiction between the refusal and its own enumerated grammar also makes the error useless for diagnosis.

Fix: the method-setting patch-path validation in the UpdateStage handler — accept both spellings of the method path's resource half (escaped and unescaped leading slash) before concluding a pointer names no method, and keep the stored/served `methodSettings` key in the documented `{resource_path}/{http_method}` form for either spelling.

Traces: apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it (verdict error; botocore BadRequestException on UpdateStage). File/line citation: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_stage.rs — `names_a_resource` (~line 972), `method_setting` (~line 935), `STAGE_PATCH_PATHS`/`STAGE_INVALID_PATCH_PATH` (~line 407-422). Related but NOT owning this cause: open issue 20261104T130732Z-apigateway-updatestage-returns-an-extra-methodsettings-entry-keyed-by.md, which reports an EXTRA methodSettings entry on an accepted patch; it carries no key for a patch that is refused outright.

=== Cause 2 of 3: an apiKeyRequired method on a deployed stage never answers — the mismatched-key refusal cannot be observed ===

AWS: a method with `apiKeyRequired=true` on a stage attached to a usage plan refuses a request whose `x-api-key` header does not match a key in that plan with HTTP 403 and error type ForbiddenException, body `{"message":"Forbidden"}`, and the refusal is taken BEFORE the integration runs, so the backing proxy integration is never invoked (no backend side effect, no backend log line). The matching key is served normally. Recorded behavior only for the ordering guarantee (it is not expressible in the Smithy model); the status/code/body above are the recorded refusal.

Emulator: the check never reaches a verdict — the deployed execute-api stage produces no response at all and the check is killed at the harness's 60 s cap (`Check timed out after 60.0s`). Nothing is served, so neither the 403 refusal nor the before-integration ordering can be observed.

Why: a client cannot use API-key gating at all: requests hang instead of being answered, so both the allow path and the refusal path are unusable, and a caller relying on the refusal happening before the integration cannot tell whether its backend ran.

Fix: the execute-api data-plane front door / stage invoke path for a method carrying apiKeyRequired — it must produce a response (the 403 refusal before integration dispatch, or the integration's response) rather than never completing.

Traces: apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration (mismatch; Check timed out after 60.0s). Related but NOT owning this cause: open issue 20261029T193654Z-apigateway-api-gateway-execute-api-data-plane-10-root.md (the replay reported this check as unowned by it).

=== Cause 3 of 3: an HTTP_PROXY vs plain-HTTP CORS-header comparison never answers at the stage ===

AWS: for a resource backed by an HTTP_PROXY integration, API Gateway forwards the backend's response headers (including `Access-Control-Allow-Origin`) to the caller unchanged; for the same backend behind a plain `HTTP` (non-proxy) integration, only headers declared in the method response / integration response `responseParameters` mapping survive, so an undeclared `Access-Control-Allow-Origin` is dropped. Smithy: `Integration$type` (IntegrationType, documented as HTTP/HTTP_PROXY/AWS/AWS_PROXY/MOCK) and `Integration$requestParameters`/`IntegrationResponse$responseParameters` carry the mapping contract; the pass-through-vs-mapped distinction is recorded behavior.

Emulator: the check never reaches a verdict — neither the HTTP_PROXY resource nor the plain-HTTP resource on the deployed stage ever answers, and the check is killed at the harness's 60 s cap (`Check timed out after 60.0s`).

Why: a browser client behind an HTTP_PROXY integration gets no response at all, so CORS cannot work; and a client that relies on the plain-HTTP integration dropping undeclared headers cannot distinguish the two integration types.

Fix: the execute-api data-plane integration dispatcher for HTTP and HTTP_PROXY integration types — it must answer, and it must apply the proxy pass-through / non-proxy response-header mapping rule.

Traces: apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it (mismatch; Check timed out after 60.0s).

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), service heading `### apigateway`, 3 failing checks not owned by any open issue:

1) [error] apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it
   botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the UpdateStage operation: Invalid method setting path: /c/GET/caching/enabled. Must be one of: [/deploymentId, /description, /cacheClusterEnabled, /cacheClusterSize, /clientCertificateId, /accessLogSettings, /{resourcePath}/{httpMethod}/metrics/enabled, /{resourcePath}/{httpMethod}/logging/dataTrace, /{resourcePath}/{httpMethod}/logging/loglevel, /{resourcePath}/{httpMethod}/throttling/burstLimit, /{resourcePath}/{httpMethod}/throttling/rateLimit, /{resourcePath}/{httpMethod}/caching/ttlInSeconds, /{resourcePath}/{httpMethod}/caching/enabled, /{resourcePath}/{httpMethod}/caching/dataEncrypted, /{resourcePath}/{httpMethod}/caching/requireAuthorizationForCacheControl, /{resourcePath}/{httpMethod}/caching/unauthorizedCacheControlHeaderStrategy, /variables/{variable_name}, /documentationVersion, /canarySettings/percentTraffic, /canarySettings/deploymentId, /canarySettings/stageVariableOverrides/{variable_name}, /canarySettings/useStageCache, /tracingEnabled]
   The check's patch list was [{op: replace, path: /<resource>/<VERB>/caching/enabled, value: "true"}, {op: replace, path: /<resource>/<VERB>/caching/ttlInSeconds, value: "<ttl>"}] against a stage created with cacheClusterEnabled=true.
   Expected: HTTP 200 with the updated Stage (methodSettings carrying cachingEnabled=true and cacheTtlInSeconds for that method). Actual: HTTP 400 BadRequestException. `/c` and `GET` are this run's resource path and verb; the requirement is the path GRAMMAR, not those values.

2) [mismatch] apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration
   Error: Check timed out after 60.0s
   Expected: the stage answers 403 ForbiddenException {"message":"Forbidden"} for a non-matching x-api-key without invoking the integration, and 200 for the matching key. Actual: no response at all within the harness cap.

3) [mismatch] apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it
   Error: Check timed out after 60.0s
   Expected: HTTP_PROXY resource returns the backend's Access-Control-Allow-Origin; the plain HTTP resource returns the same body without it. Actual: no response at all within the harness cap.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: in the report find `### apigateway` → `[error] apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it`. By hand: CreateRestApi; CreateResource with pathPart `c`; PutMethod GET; PutIntegration; CreateDeployment; CreateStage with cacheClusterEnabled=true and a cacheClusterSize; then UpdateStage with patchOperations [{"op":"replace","path":"/c/GET/caching/enabled","value":"true"},{"op":"replace","path":"/c/GET/caching/ttlInSeconds","value":"300"}]. AWS answers 200 with the patched Stage; the emulator answers 400 BadRequestException "Invalid method setting path: /c/GET/caching/enabled. Must be one of: [...]".
3. Cause 2: replay apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration. By hand: deploy a stage whose method has apiKeyRequired=true, attach the stage to a usage plan with one key, then GET the invoke URL with a non-matching x-api-key. AWS answers 403 {"message":"Forbidden"} without running the integration; the emulator never answers.
4. Cause 3: replay apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it. By hand: on one API create two resources against the same backend, one with type=HTTP_PROXY and one with type=HTTP plus a 200 method/integration response that declares no Access-Control-Allow-Origin; deploy and GET both. AWS returns the header only through the proxy resource; the emulator returns nothing for either.

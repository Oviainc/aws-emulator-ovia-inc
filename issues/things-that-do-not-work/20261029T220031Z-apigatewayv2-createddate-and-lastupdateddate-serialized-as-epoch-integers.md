Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-stage-bogus-child-not-found
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-stage-bogus-child-not-found apigatewayv2-authorizer-deployment-cache-status-codes apigatewayv2-stage-route-settings-bogus-child-not-found apigatewayv2-websocket-deployment-requires-route-targets apigatewayv2-http-route-key-grammar apigatewayv2-websocket-route-key-grammar apigatewayv2-websocket-route-selection-expression-required apigatewayv2-import-reimport-status-codes
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreateStage|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|CreateStage|2xx->2xx|$.lastUpdatedDate
Issue-Key: apigatewayv2|GetStage|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|GetStage|2xx->2xx|$.lastUpdatedDate
Issue-Key: apigatewayv2|UpdateStage|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|UpdateStage|2xx->2xx|$.lastUpdatedDate
Issue-Key: apigatewayv2|CreateDeployment|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|CreateDeployment|2xx->2xx|$.deploymentId
Issue-Key: apigatewayv2|CreateApi|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|GetApi|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|GetStage|2xx->2xx|$.routeSettings.GET /widgets
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigatewayv2 createdDate and lastUpdatedDate serialized as epoch integers

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
The same REST-JSON timestamp-serializer fault reported for the `apigateway` token also reaches steps attributed to the `apigatewayv2` token: `createdDate` / `lastUpdatedDate` are emitted as bare Unix epoch integers where AWS emits RFC3339 date-time strings. The Smithy model targets `__timestampIso8601` with `timestampFormat: date-time` on every one of these members, so a quoted string such as "2026-10-01T20:17:46Z" is the required wire form. Eight causes, one per affected operation.

Cause 1 of 8: CreateStage.createdDate and lastUpdatedDate.
AWS: ISO-8601 date-time strings. Smithy: CreateStageResponse.CreatedDate / LastUpdatedDate -> __timestampIso8601, jsonName `createdDate`/`lastUpdatedDate`, timestampFormat date-time.
Emulator: epoch-seconds integers.
Why: SDK date-time deserialization of a JSON number fails or yields a wrong instant, and any client formatting or ordering on these members breaks.
Fix: the REST-JSON timestamp writer used by the apigatewayv2 response encoders (honour timestampFormat date-time), not the individual handlers.
Traces: apigatewayv2-api-stage-bogus-child-not-found#task#3.

Cause 2 of 8: GetStage.createdDate and lastUpdatedDate.
AWS: ISO-8601 date-time strings.
Emulator: epoch integers.
Why/Fix: as cause 1.
Traces: apigatewayv2-api-stage-bogus-child-not-found#task#4.

Cause 3 of 8: UpdateStage.createdDate and lastUpdatedDate.
AWS: ISO-8601 date-time strings.
Emulator: epoch integers.
Why/Fix: as cause 1.
Traces: apigatewayv2-api-stage-bogus-child-not-found#task#6.

Cause 4 of 8: CreateDeployment.createdDate.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-authorizer-deployment-cache-status-codes#task#6.

Cause 5 of 8: CreateApi.createdDate.
AWS: ISO-8601 date-time string. Smithy: CreateApiResponse.CreatedDate -> __timestampIso8601, date-time.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-http-route-key-grammar#setup#1.

Cause 6 of 8: GetApi.createdDate.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-import-reimport-status-codes#task#3.

Cause 7 of 8: GetStage.createdDate / lastUpdatedDate on a stage whose route settings were deleted (the same cluster also reports the stale routeSettings entry, which belongs to the route-settings deletion fault).
AWS: ISO-8601 date-time strings.
Emulator: epoch integers.
Why/Fix: as cause 1.
Traces: apigatewayv2-stage-route-settings-bogus-child-not-found#task#9.

Cause 8 of 8: CreateDeployment.createdDate on a WebSocket api.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-websocket-deployment-requires-route-targets#task#9.

## Evidence
Replay verdict `mismatch` with matching statuses and details `$.createdDate: value_mismatch (expected '2026-10-01T20:17:46Z', got 1793310309)` and `$.lastUpdatedDate: value_mismatch (expected '2026-10-01T20:17:46Z', got 1793310309)` (representative values; every cluster shows a quoted RFC3339 string expected against a JSON number). Smithy confirms the member targets are __timestampIso8601 with timestampFormat date-time.

## Steps to Reproduce
Replay the corpus and POST /v2/apis/{apiId}/stages (fixture apigatewayv2-api-stage-bogus-child-not-found step task#3): `createdDate` and `lastUpdatedDate` in the 201 body are JSON numbers, not quoted RFC3339 strings. Same for the other seven fixture#step pairs.

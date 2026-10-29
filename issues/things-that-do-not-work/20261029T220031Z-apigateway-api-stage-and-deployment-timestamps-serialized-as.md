Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-mapping-negative-paths
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-mapping-negative-paths apigatewayv2-api-tagging-lifecycle apigatewayv2-export-import-reimport-roundtrip apigatewayv2-http-api-route-config-cleanup apigatewayv2-integration-response-errors apigatewayv2-integration-response-lifecycle apigatewayv2-model-lifecycle apigatewayv2-quick-create-managed-resources apigatewayv2-reset-authorizers-cache apigatewayv2-route-request-parameter-lifecycle apigatewayv2-route-response-lifecycle apigatewayv2-route-settings-lifecycle apigatewayv2-routing-rule-error-paths apigatewayv2-stage-access-log-settings-lifecycle apigatewayv2-stage-deployment-workflow apigatewayv2-tag-resource-workflow apigatewayv2-websocket-api-lifecycle apigatewayv2-cors-configuration-lifecycle apigatewayv2-http-api-cors-accesslog-cache-cleanup apigatewayv2-http-api-lifecycle apigatewayv2-models-and-cors apigatewayv2-deployment-error-paths apigatewayv2-deployment-lifecycle apigatewayv2-stage-create-with-tags
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateApi|2xx->2xx|$.createdDate
Issue-Key: apigateway|GetApi|2xx->2xx|$.createdDate
Issue-Key: apigateway|CreateDeployment|2xx->2xx|$.createdDate
Issue-Key: apigateway|GetDeployments|2xx->2xx|$.items[*]
Issue-Key: apigateway|GetStage|2xx->2xx|$.createdDate
Issue-Key: apigateway|GetStage|2xx->2xx|$.lastUpdatedDate
Issue-Key: apigateway|GetApis|2xx->2xx|$.items[name="sim2real-probe-apigatewayv2-httpapi-3304e4fa"].createdDate
Issue-Key: apigateway|GetStages|2xx->2xx|$.items[*].createdDate
Issue-Key: apigateway|GetStages|2xx->2xx|$.items[*].lastUpdatedDate
Issue-Key: apigateway|CreateStage|2xx->2xx|$.createdDate
Issue-Key: apigateway|CreateStage|2xx->2xx|$.lastUpdatedDate
Issue-Key: apigateway|UpdateStage|2xx->2xx|$.createdDate
Issue-Key: apigateway|UpdateStage|2xx->2xx|$.lastUpdatedDate
Issue-Key: apigateway|GetDeployment|2xx->2xx|$.createdDate
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway API, stage and deployment timestamps serialized as epoch integers

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Every `createdDate` / `lastUpdatedDate` member on the ApiGatewayV2 api, stage and deployment resources is serialized as a bare Unix epoch integer instead of an RFC3339 date-time string. The Smithy model targets `__timestampIso8601` with `timestampFormat: date-time` for all of these members, so the required wire form is a quoted string such as "2026-09-06T13:32:15Z"; the emulator emits an unquoted number of seconds. One serializer fault, listed per affected operation so every cluster can be re-replayed.

Cause 1 of 10: CreateApi.createdDate.
AWS: an ISO-8601 date-time string. Smithy: CreateApiResponse.CreatedDate -> __timestampIso8601, jsonName `createdDate`, timestampFormat date-time.
Emulator: an integer (epoch seconds), e.g. the number where a date-time string is required.
Why: every AWS SDK deserializes this member with the date-time reader; a JSON number makes the field unparseable or silently wrong, and any client formatting/sorting createdDate breaks.
Fix: the REST-JSON timestamp serializer used by the apigatewayv2 response encoders (it must honour timestampFormat date-time), not the individual handlers.
Traces: apigatewayv2-api-mapping-negative-paths#setup#1.

Cause 2 of 10: GetApi.createdDate.
AWS: ISO-8601 date-time string (Smithy GetApiResponse.CreatedDate, date-time).
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-cors-configuration-lifecycle#task#2.

Cause 3 of 10: CreateDeployment.createdDate.
AWS: ISO-8601 date-time string (Smithy CreateDeploymentResponse.CreatedDate, date-time).
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-deployment-error-paths#task#4.

Cause 4 of 10: GetDeployments items[].createdDate.
AWS: ISO-8601 date-time string inside each `items` entry.
Emulator: epoch integer, so the whole list element compares unequal.
Why/Fix: as cause 1 - the list element encoder uses the same timestamp writer.
Traces: apigatewayv2-deployment-lifecycle#task#5.

Cause 5 of 10: GetStage.createdDate and lastUpdatedDate.
AWS: both ISO-8601 date-time strings (Smithy GetStageResponse.CreatedDate / LastUpdatedDate, date-time).
Emulator: both epoch integers.
Why/Fix: as cause 1.
Traces: apigatewayv2-http-api-cors-accesslog-cache-cleanup#task#7.

Cause 6 of 10: GetApis items[].createdDate.
AWS: ISO-8601 date-time string per listed api.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-http-api-lifecycle#task#5.

Cause 7 of 10: GetStages items[].createdDate and lastUpdatedDate.
AWS: ISO-8601 date-time strings per listed stage.
Emulator: epoch integers.
Why/Fix: as cause 1.
Traces: apigatewayv2-quick-create-managed-resources#task#4.

Cause 8 of 10: CreateStage.createdDate and lastUpdatedDate.
AWS: ISO-8601 date-time strings. Smithy: CreateStageResponse.CreatedDate / LastUpdatedDate -> __timestampIso8601, date-time.
Emulator: epoch integers.
Why/Fix: as cause 1.
Traces: apigatewayv2-reset-authorizers-cache#setup#4.

Cause 9 of 10: UpdateStage.createdDate and lastUpdatedDate.
AWS: ISO-8601 date-time strings.
Emulator: epoch integers.
Why/Fix: as cause 1.
Traces: apigatewayv2-stage-access-log-settings-lifecycle#task#5.

Cause 10 of 10: GetDeployment.createdDate.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-stage-deployment-workflow#task#7.

## Evidence
Replay verdict `mismatch` with detail `$.createdDate: value_mismatch (expected '<ISO-8601 date-time>', got <epoch integer>)` (and the same for `$.lastUpdatedDate`) on all ten clusters; status matched in every case. The recorded values are strings like '2026-09-06T13:32:15Z' while the emulator returns a JSON number. Smithy confirms the member target is __timestampIso8601 with timestampFormat date-time.

## Steps to Reproduce
Replay the corpus, then POST /v2/apis (fixture apigatewayv2-api-mapping-negative-paths step setup#1) and look at `createdDate` in the 201 body: it is a JSON number, not a quoted RFC3339 string. Same for the other nine fixture#step pairs.

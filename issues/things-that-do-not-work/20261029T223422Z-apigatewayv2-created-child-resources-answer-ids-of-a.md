Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-authorizer-deployment-cache-status-codes
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-authorizer-deployment-cache-status-codes apigatewayv2-websocket-model-route-response-status-codes
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreateAuthorizer|2xx->2xx|$.authorizerId
Issue-Key: apigatewayv2|CreateModel|2xx->2xx|$.modelId
Issue-Key: apigatewayv2|CreateRouteResponse|2xx->2xx|$.routeResponseId
Issue-Commit: 900a7beaab356ed52889492ca7befa08b17bdc0c

# apigatewayv2 created child resources answer ids of a non-AWS shape

> _Note: this issue was identified against the code base at commit `900a7beaab356ed52889492ca7befa08b17bdc0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Three net-new wire divergences on the `apigatewayv2` service token share one root cause: the server-minted identifier returned by the Create* handlers for authorizers, models and route responses is out of family with the identifier shape AWS returns.

Shared shape of the defect — AWS: Smithy `com.amazonaws.apigatewayv2#Id` — the target of `CreateAuthorizerResponse.authorizerId`, `CreateModelResponse.modelId` and `CreateRouteResponseResponse.routeResponseId` — is an unconstrained string (no @pattern, no @length, doc "The identifier."), so the concrete format is recorded behavior only. In every recording the id is a short, 6-7 character, lowercase-alphanumeric opaque token. Emulator: it answers a 10-character token whose final three characters are always the constant literal `aaa`, so the value fails the recorded id shape for that member whatever concrete id a given run mints. The fixed `aaa` tail also shows the generator is a per-process counter rather than an independent per-resource token, so the same value recurs for unrelated resources within a run. Why: a client that validates, indexes or round-trips API Gateway v2 ids against the documented short-opaque-id family accepts a value real AWS never emits, and an id that repeats across resources collides in any client-side cache keyed by id. Fix: the shared identifier-minting helper in the apigatewayv2 service crate's state module, used by the CreateAuthorizer / CreateModel / CreateRouteResponse handlers (and every other handler returning an `Id`-typed member), so the ids it serializes match the recorded shape.

Cause 1 of 3 — CreateAuthorizer. AWS: 201 with `$.authorizerId` a short opaque id (recorded value in this fixture `d6tyvv`, 6 chars [a-z0-9]). Emulator: 201 with `$.authorizerId` = `kgs2qrlaaa` (10 chars, constant `aaa` tail). Trace: apigatewayv2-authorizer-deployment-cache-status-codes#task#2.

Cause 2 of 3 — CreateModel on a WebSocket API. AWS: 201 with `$.modelId` a short opaque id (recorded `e94sid`). Emulator: 201 with `$.modelId` = `mkuboukaaa` — the identical value the emulator also returns for unrelated models elsewhere in the corpus. Trace: apigatewayv2-websocket-model-route-response-status-codes#task#2.

Cause 3 of 3 — CreateRouteResponse. AWS: 201 with `$.routeResponseId` a short opaque id (recorded `47kks8`). Emulator: 201 with `$.routeResponseId` = `je9fa8laaa`. Trace: apigatewayv2-websocket-model-route-response-status-codes#task#5.

All three steps return the modeled 201; only the identifier member diverges. No open issue carries these replay keys. `20261029T220031Z-apigatewayv2-responses-omit-defaulted-and-derived-members.md` is adjacent in subject matter (derived members) but owns none of these three steps, so it does not cover them.

## Evidence
Replay of trace set simparity-wire-aws-v1. Three clusters with no owning issue and no blocking stub, all service=apigatewayv2:

1. CreateAuthorizer, apigatewayv2-authorizer-deployment-cache-status-codes#task#2, expected 201 / got 201: `$.authorizerId: value_mismatch (expected 'd6tyvv', got 'kgs2qrlaaa')`.
2. CreateModel, apigatewayv2-websocket-model-route-response-status-codes#task#2, 201/201: `$.modelId: value_mismatch (expected 'e94sid', got 'mkuboukaaa')`.
3. CreateRouteResponse, apigatewayv2-websocket-model-route-response-status-codes#task#5, 201/201: `$.routeResponseId: value_mismatch (expected '47kks8', got 'je9fa8laaa')`.

The divergence is the id FAMILY, not the literal value: every recorded id in this corpus is 6-7 chars of [a-z0-9] (`d6tyvv`, `e94sid`, `47kks8`, `5jiki1c`, `kw8ui5s`, `re2gib`), while every emulator id is 10 chars ending in the constant `aaa` (`kgs2qrlaaa`, `mkuboukaaa`, `je9fa8laaa`, `oowklxjaaa`, `nmdy4dkaaa`, `libp7alaaa`, `f64xf2naaa`).

Smithy consulted: `Id` (unconstrained string); `CreateAuthorizer` output `authorizerId` -> `Id`, POST /v2/apis/{ApiId}/authorizers, code 201; `CreateModel` output `modelId` -> `Id`, POST /v2/apis/{ApiId}/models, code 201; `CreateRouteResponse` output `routeResponseId` -> `Id`, POST /v2/apis/{ApiId}/routes/{RouteId}/routeresponses, code 201.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay apigatewayv2-authorizer-deployment-cache-status-codes and read `$.authorizerId` from step task#2 (CreateAuthorizer) — a 10-character token ending in `aaa` instead of a short opaque id.
3. Cause 2: replay apigatewayv2-websocket-model-route-response-status-codes and read `$.modelId` from step task#2 (CreateModel).
4. Cause 3: in the same fixture, read `$.routeResponseId` from step task#5 (CreateRouteResponse).
5. Each response carries the correct 201 status; only the id-shaped member diverges.

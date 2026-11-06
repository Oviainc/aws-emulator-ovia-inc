Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-authorizer-deployment-cache-status-codes
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-authorizer-deployment-cache-status-codes apigatewayv2-domain-api-mapping-bogus-child-not-found
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|ResetAuthorizersCache|2xx->2xx|$headers.content-length
Issue-Key: apigatewayv2|ResetAuthorizersCache|2xx->2xx|status
Issue-Key: apigatewayv2|CreateApiMapping|2xx->2xx|$.apiMappingId
Issue-Commit: 73f8eaecfe0448dfa40697a75965cd99973ceee3

# apigatewayv2 ResetAuthorizersCache answers 204 and CreateApiMapping mints a mis-shaped ApiMappingId (2 causes)

> _Note: this issue was identified against the code base at commit `73f8eaecfe0448dfa40697a75965cd99973ceee3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResetAuthorizersCache

## Description
Two net-new wire divergences on the API Gateway v2 surface served under the `apigatewayv2` service token. They are reported together because they share one service and one verdict from the bootstrap gate; each is an independent cause and BOTH must be closed.

Cause 1 of 2 — ResetAuthorizersCache terminates the authorizer cache with the wrong response framing.
AWS: Smithy `com.amazonaws.apigatewayv2#ResetAuthorizersCache` is `DELETE /v2/apis/{ApiId}/stages/{StageName}/cache/authorizers`, output `Unit`, modelled errors NotFoundException (404) and TooManyRequestsException (429); the model's `@http` code is 204, but the recording of the real service shows the request answered with HTTP **200** and a `Content-Length` response header present (the body carries no modelled members, so the header's value is the byte length of the body the service actually writes). The recorded status, not the modelled code, is the requirement here. Emulator: answers **204** and emits no `Content-Length` header at all. Why: the operation has no output members, so status is the only thing a client can key on — a caller that treats 200 as "cache flushed" and anything else as an anomaly (or a proxy/HTTP client that distinguishes a 204 "no content, no entity headers" response from a 200 with an entity body of length n) sees a different outcome from AWS for a successful flush, and a strict HTTP intermediary may drop the body framing entirely. Fix: the ResetAuthorizersCache handler/serializer in the apigatewayv2 service crate — the success status for this operation must be 200 with the body framed by a Content-Length header, not a bare 204. Traces: apigatewayv2-authorizer-deployment-cache-status-codes#task#9.

Cause 2 of 2 — CreateApiMapping mints an `apiMappingId` outside the identifier family AWS returns.
AWS: `CreateApiMapping` output member `ApiMappingId` targets `com.amazonaws.apigatewayv2#Id`, an unconstrained string with no @pattern and no @length, so the concrete format is recorded behavior only: in every recording in this corpus a server-minted API Gateway v2 child-resource id is a short, 6-7 character, lowercase-alphanumeric opaque token ([a-z0-9], e.g. the recorded `d6tyvv`, `5jiki1c`, `re2gib`, `47kks8` for sibling resources). The step's status (200) matches; only the identifier-shaped member diverges. Emulator: returns a 10-character, purely alphabetic token (observed `funswxvaeh`), which fails the recorded shape for that member in every run regardless of which concrete value a given run produces. Why: a client that validates, indexes or round-trips API Gateway v2 ids against that short-opaque-id family — including anything that stores an apiMappingId and re-addresses `/v2/domainnames/{domainName}/apimappings/{apiMappingId}` from a persisted value, or that length-checks ids — accepts a value real AWS never emits. Fix: the shared identifier-minting helper in the apigatewayv2 service crate's state module as used by the CreateApiMapping handler, so the minted id is of the recorded shape. Related but NOT owning: the open issue `20261029T223422Z-apigateway-generated-resource-identifiers-diverge-from-the-recorded.md` describes the same identifier-family defect, but it is scoped to the `apigateway` service token and its replay keys cover routeId/integrationId/modelId/Route.target only — it does not carry this fixture step and will not exercise apiMappingId. Traces: apigatewayv2-domain-api-mapping-bogus-child-not-found#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (106 fixtures, 899 steps, score 75.5%). Two clusters with service=apigatewayv2, no owning issue and no blocking stub:

1. ResetAuthorizersCache — fixture apigatewayv2-authorizer-deployment-cache-status-codes, step task#9, expected_status 200 / actual_status 204. detail: `status: expected 200, got 204` and `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`. affected_count 1.
2. CreateApiMapping — fixture apigatewayv2-domain-api-mapping-bogus-child-not-found, step task#5, expected_status 200 / actual_status 200. detail: `$.apiMappingId: value_mismatch (expected '<shaped-not-asserted>', got 'funswxvaeh')` — the recorded value is asserted by shape, and the emulator's 10-character all-alphabetic token fails it. affected_count 1.

Smithy consulted (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json): ResetAuthorizersCache (input ApiId/StageName as @httpLabel, output Unit, errors NotFoundException 404 / TooManyRequestsException 429, http DELETE /v2/apis/{ApiId}/stages/{StageName}/cache/authorizers code 204); CreateApiMapping (http POST /v2/domainnames/{DomainName}/apimappings code 201, output apiId/apiMappingId/apiMappingKey/stage, apiMappingId -> Id); shape Id = unconstrained string, doc "The identifier.". Note the recording answers CreateApiMapping with 200, which the emulator matches — that part is not a finding.

The open issue `20261106T033429Z-apigatewayv2-204-deletes-omit-the-content-length-0.md` owns the missing `Content-Length: 0` on genuine 204 deletes; it does not own this step, where the status itself is wrong (200 expected).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture apigatewayv2-authorizer-deployment-cache-status-codes and inspect step task#9 — `DELETE /v2/apis/{apiId}/stages/{stageName}/cache/authorizers` returns 204 with no Content-Length header, where the recording returns 200 with a Content-Length header.
3. Cause 2: replay fixture apigatewayv2-domain-api-mapping-bogus-child-not-found and inspect step task#5 — `POST /v2/domainnames/{domainName}/apimappings` returns 200 (correct) but `$.apiMappingId` is a 10-character all-alphabetic token (e.g. `funswxvaeh`) instead of a short 6-7 character [a-z0-9] opaque id.
4. Both steps must pass before the issue is closed.

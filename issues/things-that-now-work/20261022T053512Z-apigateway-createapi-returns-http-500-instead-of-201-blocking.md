Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewaymanagementapi-connection-envelope-errors
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewaymanagementapi-connection-envelope-errors
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigateway|CreateApi|2xx->5xx|status
Issue-Commit: 4889d6fc2c3f21197ae15ebc7db5fc1046bb7fbe

# CreateApi returns HTTP 500 instead of 201, blocking all dependent steps

> _Note: this issue was identified against the code base at commit `4889d6fc2c3f21197ae15ebc7db5fc1046bb7fbe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: CreateApi fails with an unhandled server error on the happy path, so no API can be created and every step that depends on an existing API id is blocked.

AWS: CreateApi is modeled on the ApiGatewayV2 surface (endpoint prefix `apigateway`, which is the service token the replay reports) as `POST /v2/apis` with success status 201 and output shape CreateApiResponse (apiId, apiEndpoint, name, protocolType, routeSelectionExpression, apiKeySelectionExpression, createdDate, ...). Smithy: CreateApi http {method: POST, uri: /v2/apis, code: 201}; required input members Name (StringWithLengthBetween1And128) and ProtocolType (ProtocolType enum); modeled errors are BadRequestException (400), ConflictException (409), NotFoundException (404) and TooManyRequestsException (429). There is no modeled 500 for this operation, so a well-formed create request must either succeed with 201 or fail with one of those four client errors.

Emulator: the replayed request returns HTTP 500. The replay reports `status: expected 201, got 500` for CreateApi with `not_implemented: 0` and no stub path, so this is not an unimplemented operation — the operation is routed and reached, and the handler faults while servicing a request the recording shows AWS accepting with 201. The run scored 0% on this fixture: the single mismatched step is the fixture's `setup#1`, and the 10 remaining steps of the fixture were reported blocked as a consequence (steps_total 11, steps_passed 0, steps_mismatched 1, steps_blocked 10).

Why: a client cannot create an API at all. Because CreateApi is the bootstrap call for the whole WebSocket/HTTP API object graph, the 500 cascades: callers never obtain an apiId, so stages, routes, integrations and the ApiGatewayManagementApi connection operations that key off that id are all unreachable. A 500 is also retried by the AWS SDKs' default retry policy (5xx is retryable, unlike the modeled 4xx client errors), so clients burn their full retry budget and then surface an opaque internal-failure exception instead of an actionable error — and any client that would legitimately be told BadRequestException/ConflictException gets the wrong class of error and the wrong control flow.

Fix: belongs in the ApiGatewayV2 CreateApi handler and its response path — the handler must construct and persist the Api resource for a valid Name + ProtocolType request, then serialize CreateApiResponse with HTTP 201, and must reject invalid input with the modeled BadRequestException (400) rather than faulting. If the fault is in output serialization (e.g. an unset required-on-the-wire field such as apiId/apiEndpoint/createdDate, or an unformatted `date-time` timestamp for createdDate) the fix is in the serializer for CreateApiResponse; if it is in the create path's state insertion, it belongs in that service's state module. Whatever the internal cause, no code path for this operation may surface a 500.

Traces: fixture `apigatewaymanagementapi-connection-envelope-errors`, step `setup#1` (divergence key `service=apigateway|CreateApi|status=201->500|keys=status`; affected: apigatewaymanagementapi-connection-envelope-errors#setup#1). The same fixture's remaining 10 steps are blocked behind this one and should re-replay clean once CreateApi returns 201.

## Evidence
Expected (recorded AWS response): HTTP 201 with a CreateApiResponse body.
Actual (emulator): HTTP 500.
Replay detail: "status: expected 201, got 500"; verdict=mismatch; divergence_key="service=apigateway|CreateApi|status=201->500|keys=status"; service=apigateway; operation=CreateApi; fixture_id=apigatewaymanagementapi-connection-envelope-errors; step_key=setup#1.
Run summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, state_reset_failures=0, not_implemented=0, stub_path empty (operation is implemented/routed, not stubbed), steps_total=11, steps_passed=0, steps_mismatched=1, steps_blocked=10, score_pct=0.0.
Smithy evidence: CreateApi success code is 201; modeled errors are BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429) — 500 is not a modeled outcome.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `apigatewaymanagementapi-connection-envelope-errors` from trace set simparity-wire-aws-v1.
3. Observe step `setup#1` (CreateApi, POST /v2/apis with a Name and ProtocolType): the emulator answers HTTP 500 where the recording has HTTP 201 with a CreateApiResponse body.
4. Observe that the fixture's remaining 10 steps are reported blocked because no apiId was ever returned.

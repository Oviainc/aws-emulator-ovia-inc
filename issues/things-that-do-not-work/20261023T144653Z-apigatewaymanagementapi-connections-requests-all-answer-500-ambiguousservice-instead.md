Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewaymanagementapi-connection-envelope-errors
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewaymanagementapi-connection-envelope-errors
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->5xx|$
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->5xx|$headers.content-type
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->5xx|status
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->5xx|$
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->5xx|$headers.content-type
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->5xx|status
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->5xx|$
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->5xx|$headers.content-type
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->5xx|status
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->5xx|$.message
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# ApiGatewayManagementApi @connections requests all answer 500 AmbiguousService instead of the modeled error envelope

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
Shared fault, 4 causes. The emulator's front door cannot resolve a target service for the `execute-api` endpoint prefix: two registered emulated services both claim it (the replay body reports "2 emulated services all claim requests for 'execute-api': execute-api, execute-api"), so instead of dispatching to ApiGatewayManagementApi the router refuses the request with HTTP 500 `AmbiguousService`. Every @connections operation is therefore unreachable and none of the service's modeled errors can ever be produced. Smithy (ApiGatewayManagementApi 2018-11-29): awsService.endpointPrefix = "execute-api", sigv4Name = "execute-api", protocol restJson1 — sharing that prefix with the API Gateway execute-api data plane is normal AWS and must be disambiguated (by URI shape: @connections/{ConnectionId}), not rejected. Note the separately-open issue "API Gateway execute-api stage endpoints are not routed ... 404 UnknownService" (20261023T114649Z-apigateway-...) describes the OTHER side of the same router: it does not cover these steps, which fail with 500 AmbiguousService on apigatewaymanagementapi and are not owned by it.

Cause 1 of 4 — GetConnection on a deleted connection (fixture apigatewaymanagementapi-connection-envelope-errors, step task#6).
AWS: GetConnection GET /@connections/{ConnectionId} returns GoneException when the connection no longer exists. Smithy: ApiGatewayManagementApi common error GoneException, errorKind client, @httpError 410, doc "The connection with the provided id no longer exists."; protocol restJson1, so content-type application/json and header x-amzn-errortype naming the error shape.
Emulator: HTTP 500 with content-type application/x-amz-json-1.1, x-amzn-errortype: AmbiguousService and body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'execute-api' ..."}. The step is a poll that waits for GoneException; it gave up after 1 attempt because 500 AmbiguousService can never satisfy it.
Why: a client cannot detect a stale WebSocket connection. The documented contract is "410 -> stop sending to this connection"; a 500 is a retryable server fault, so SDK retry policies will retry forever and connection-reaping logic never runs. The wrong content-type additionally breaks restJson1 error parsing, so the SDK cannot even model the failure.
Fix: the front-door request router / service resolution for the execute-api endpoint prefix (disambiguate ApiGatewayManagementApi by the /@connections/{ConnectionId} URI before falling back to the apigateway data plane), then GetConnection's handler must raise GoneException with @httpError 410 through the restJson1 error serializer.
Traces: apigatewaymanagementapi-connection-envelope-errors#task#6.

Cause 2 of 4 — PostToConnection on a deleted connection (fixture apigatewaymanagementapi-connection-envelope-errors, step task#7).
AWS: PostToConnection POST /@connections/{ConnectionId} returns GoneException (Smithy: common error GoneException, @httpError 410) when the target connection is gone; restJson1 envelope, content-type application/json.
Emulator: HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype: AmbiguousService, same AmbiguousService body — the request never reaches a PostToConnection handler.
Why: the push path is the primary use of this API. A server pushing to WebSocket clients relies on 410 to prune dead connections; a 500 makes every dead connection look like a transient outage, so the sender retries and leaks connection records indefinitely.
Fix: same router/service-resolution component, then PostToConnection's handler + the restJson1 error serializer.
Traces: apigatewaymanagementapi-connection-envelope-errors#task#7.

Cause 3 of 4 — DeleteConnection on a deleted connection (fixture apigatewaymanagementapi-connection-envelope-errors, step task#8).
AWS: DeleteConnection DELETE /@connections/{ConnectionId} succeeds with @http code 204 and returns GoneException (Smithy: common error GoneException, @httpError 410) when the connection no longer exists; restJson1 envelope, content-type application/json.
Emulator: HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype: AmbiguousService, same AmbiguousService body.
Why: a disconnect is not idempotently observable — a caller cannot distinguish "already gone" (410, safe to ignore) from a server failure (500, must retry), so cleanup paths either loop or abort.
Fix: same router/service-resolution component, then DeleteConnection's handler + the restJson1 error serializer.
Traces: apigatewaymanagementapi-connection-envelope-errors#task#8.

Cause 4 of 4 — GetConnection with a malformed connection id (fixture apigatewaymanagementapi-connection-envelope-errors, step task#9).
AWS: ConnectionId is a required @httpLabel on GetConnectionRequest (Smithy: GetConnectionRequest.ConnectionId -> __string, httpLabel, required). A syntactically invalid id is rejected with HTTP 400 and a validation message built from the request naming the offending value; the recording shows body {"message": "Invalid connectionId: x"} with content-type application/json. Status 400 and the request-derived "Invalid connectionId: <value>" shape are the requirement; the literal value `x` is just this recording's input.
Emulator: HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype: AmbiguousService, message "2 emulated services all claim requests for 'execute-api' ..." — i.e. the request is rejected by the router before any input validation happens, so the client sees a server fault rather than its own bad input.
Why: a caller that passed a bad connection id gets a retryable 5xx and will retry an input that can never succeed, instead of failing fast on a 4xx. Input-validation bugs in client code become indistinguishable from emulator outages.
Fix: same router/service-resolution component, then GetConnection's input validation for the ConnectionId @httpLabel, emitting a 400 through the restJson1 error serializer.
Traces: apigatewaymanagementapi-connection-envelope-errors#task#9.

## Evidence
Replay of fixture apigatewaymanagementapi-connection-envelope-errors (trace set simparity-wire-aws-v1), 4 mismatch clusters, all with the identical actual response.

Cause 1 — step task#6, GetConnection: expected status 410 (GoneException), got 500. $: value_mismatch (expected {}, got {'__type': 'AmbiguousService', 'message': "2 emulated services all claim requests for 'execute-api': execute-api, execute-api; the request is rejected rather than served by an arbitrary one"}); $headers.content-type: expected 'application/json', got 'application/x-amz-json-1.1'; $headers.x-amzn-errortype: expected a shaped error type, got 'AmbiguousService'. Poll detail: "poll ended: 500 AmbiguousService cannot satisfy until_error ['GoneException'] after 1 attempt(s)".

Cause 2 — step task#7, PostToConnection: expected status 410, got 500; same AmbiguousService body, same content-type and x-amzn-errortype mismatches.

Cause 3 — step task#8, DeleteConnection: expected status 410, got 500; same AmbiguousService body, same content-type and x-amzn-errortype mismatches.

Cause 4 — step task#9, GetConnection with a malformed id: expected status 400, got 500; $.message: expected 'Invalid connectionId: x', got "2 emulated services all claim requests for 'execute-api': execute-api, execute-api; the request is rejected rather than served by an arbitrary one"; $headers.content-type: expected 'application/json', got 'application/x-amz-json-1.1'; $headers.x-amzn-errortype: got 'AmbiguousService'.

Model facts used: smithy_query service_info -> endpointPrefix/sigv4Name "execute-api", protocol restJson1, common_errors [ForbiddenException, GoneException, LimitExceededException]; smithy_query shape GoneException -> client error, @httpError 410; shape GetConnectionRequest -> ConnectionId @httpLabel required; digest -> DeleteConnection DELETE /@connections/{ConnectionId} code 204, GetConnection GET, PostToConnection POST.

## Steps to Reproduce
1. Boot the emulator and replay fixture apigatewaymanagementapi-connection-envelope-errors from trace set simparity-wire-aws-v1.
2. Establish a WebSocket-style connection record and delete/expire it, then call GetConnection (step task#6), PostToConnection (task#7) and DeleteConnection (task#8) against GET/POST/DELETE /@connections/{ConnectionId} on the ApiGatewayManagementApi endpoint.
3. Observe HTTP 500 with content-type application/x-amz-json-1.1, x-amzn-errortype: AmbiguousService and body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'execute-api': execute-api, execute-api; the request is rejected rather than served by an arbitrary one"}, instead of HTTP 410 GoneException in a restJson1 (application/json) envelope.
4. Call GetConnection with a malformed ConnectionId (step task#9) and observe the same 500 AmbiguousService instead of HTTP 400 with a request-derived message of the form "Invalid connectionId: <value>".
5. Both show that no request carrying the execute-api endpoint prefix is dispatched to ApiGatewayManagementApi at all.

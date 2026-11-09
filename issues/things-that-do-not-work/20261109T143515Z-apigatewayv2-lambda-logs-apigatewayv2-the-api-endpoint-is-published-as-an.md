Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request
Issue-Key: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id|_v2_invoke_url#22|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request|<lambda>#4|ConnectionClosedError|code, message = _error_detail(lambda: mgmt.post_to_connection(ConnectionId=_MALFORMED_ID, Data=b"x"))
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# apigatewayv2: the API endpoint is published as an execute-api.amazonaws.com host, and PostToConnection with a malformed connection id closes the connection instead of refusing it (2 causes)

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
Cause 1 of 2: the published API endpoint names an AWS production host.
AWS: ApiEndpoint is the address a client connects to and the recorded run reaches the API on it.
Emulator: it announces 'https://<api-id>.execute-api.us-west-2.amazonaws.com', a host the emulator does not serve, so the access-log/gateway-request-id check can never issue a request ('endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com ... its host is not one of the emulator's own hosts').
Why: a client honouring the published endpoint cannot reach the API; the emulator announces an address other than the one it is reached on.
Fix: the ApiEndpoint serializer/state in the apigatewayv2 crate.
Traces: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id.

Cause 2 of 2: ApiGatewayManagementApi PostToConnection to a malformed connection id drops the TCP connection instead of answering an error.
AWS: PostToConnection refuses a connection id that is not a valid id with HTTP 400 BadRequestException (Smithy apigatewaymanagementapi PostToConnection errors: BadRequestException @httpError 400, GoneException 410, LimitExceededException 429, ForbiddenException 403). It always answers an HTTP response.
Emulator: the request to '<stage>/@connections/AAAAAAAAAAAAAAA%3D' returns no response at all — 'botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "https://127.0.0.1:<port>/prod/@connections/AAAAAAAAAAAAAAA%3D"'. The closed connection, not the id or port, is the divergence.
Why: an SDK caller sees a transport error it must treat as retryable instead of the modelled 400 BadRequestException, so error handling around stale/invalid connection ids is impossible to write.
Fix: the PostToConnection handler and its connection-id validation in the apigatewaymanagementapi crate (reject with 400 BadRequestException rather than terminating the connection).
Traces: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request.

## Evidence
Cause 1 — Actual: 'simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com ... its host is not one of the emulator's own hosts'.
Cause 2 — expected: 400 BadRequestException. Actual: 'botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "https://127.0.0.1:37981/prod/@connections/AAAAAAAAAAAAAAA%3D"'.

## Steps to Reproduce
Replay apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id (reads ApiEndpoint and requests it) and .post-to-a-malformed-connection-id-is-rejected-bad-request (calls PostToConnection with the connection id 'AAAAAAAAAAAAAAA=').

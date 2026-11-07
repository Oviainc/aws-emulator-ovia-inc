Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.http-api-named-stage-rawpath-includes-stage-prefix apigatewayv2.http-api-route-key-precedence-and-path-parameters apigatewayv2.import-api-payload-format-version-extension-honored apigatewayv2.invoke-http-api-lambda-proxy apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ apigatewayv2.quick-create-lambda-target-invoked-directly-delivers-v2-event apigatewayv2.http-api-path-without-stage-prefix-not-delivered apigatewayv2.http-api-stage-requires-deployment-before-serving apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served apigatewayv2.http-api-v2-binary-body-base64-encoded-both-ways apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.http-api-named-stage-rawpath-includes-stage-prefix|run#20|AssertionError|assert ev.get("rawPath") == "/items", (
Issue-Key: apigatewayv2.http-api-route-key-precedence-and-path-parameters|run#35|AssertionError|assert ev.get("rawPath") == "/items/list", (
Issue-Key: apigatewayv2.import-api-payload-format-version-extension-honored|run#24|AssertionError|assert event_v2.get("rawPath") == "/echo-v2", (
Issue-Key: apigatewayv2.invoke-http-api-lambda-proxy|run#18|AssertionError|assert data.get("rawPath") == "/hello", (
Issue-Key: apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ|run#18|AssertionError|assert v1.get("path") == "/v1echo", (
Issue-Key: apigatewayv2.quick-create-lambda-target-invoked-directly-delivers-v2-event|run#24|AssertionError|assert event.get("rawPath") == "/some/path", (
Issue-Key: apigatewayv2.http-api-path-without-stage-prefix-not-delivered|run#15|AssertionError|assert status == 404, f"{label}: expected 404, got {status}: {body[:300]!r}"
Issue-Key: apigatewayv2.http-api-stage-requires-deployment-before-serving|run#19|AssertionError|assert status == 404, (
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore|run#17|AssertionError|assert _json_or_none(text) == {"message": "Not Found"}, (
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served|run#33|AssertionError|assert status == 403, (
Issue-Key: apigatewayv2.http-api-v2-binary-body-base64-encoded-both-ways|run#23|AssertionError|assert body == bytes(range(256)), (
Issue-Key: apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response|run#35|AssertionError|assert mapped_status == 201, (
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys|run#37|AssertionError|assert status == 200 and json.loads(text).get("marker") == marker, (
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# apigatewayv2 execute-api data plane: 7 net-new fidelity defects — payload rawPath/path carries the $default stage segment, unrouted/undeployed requests answer 403 instead of 404, DisableExecuteApiEndpoint leaks the UnknownService envelope, AWS_IAM is unenforced, isBase64Encoded is not decoded, overwrite:statuscode is ignored, and a CDK CORS+Lambda default route answers 500

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeApi

## Description
Seven distinct root causes in the apigatewayv2 execute-api data plane, all grounded in failing behavioral checks. Each is independent and must be closed separately.

Cause 1 of 7: the Lambda proxy payload's path carries a literal "/$default" stage segment.
AWS: an HTTP API reached on the default endpoint https://{api-id}.execute-api.{region}.amazonaws.com/{path} is served by the $default stage, and $default contributes NO path segment: payload format 2.0 reports `rawPath` as the path the client sent (e.g. "/items", "/items/list", "/echo-v2", "/hello", "/some/path") and payload format 1.0 reports the same value in the top-level `path`. Only a NAMED stage is reached at /{stageName}/{path}, and only then does the stage name appear in rawPath. Smithy: the data-plane payload is not a modeled shape — recorded behavior only; the model does describe the endpoint shape in Api.ApiEndpoint ("The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI..."), i.e. the stage is part of the URL, never injected into the request path.
Emulator: every proxy event prefixes the path with the serving stage name, e.g. rawPath '/$default/items' for GET /items, '/$default/items/list', '/$default/echo-v2', '/$default/hello', '/$default/some/path', and format 1.0 `path` '/$default/v1echo'.
Why: a handler that routes on rawPath/path (every framework adapter — Express/Flask/Chalice/Powertools — and every hand-rolled `if event["rawPath"] == "/items"`) sees a path that does not exist in its route table, so it 404s or mis-routes; path-parameter extraction and `requestContext.http.path` correlation also shift by one segment.
Fix: the execute-api HTTP data-plane request handler that builds the proxy event (v1.0 and v2.0 envelope construction): resolve the stage from the URL, strip it from the path it propagates, and only retain a stage prefix when the request was made against a named stage.
Traces: apigatewayv2.http-api-named-stage-rawpath-includes-stage-prefix, apigatewayv2.http-api-route-key-precedence-and-path-parameters, apigatewayv2.import-api-payload-format-version-extension-honored, apigatewayv2.invoke-http-api-lambda-proxy, apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ, apigatewayv2.quick-create-lambda-target-invoked-directly-delivers-v2-event.

Cause 2 of 7: an unmatched path and a stage with no deployment are refused 403 Forbidden instead of 404 Not Found.
AWS: execute-api answers HTTP 404 with the gateway envelope {"message":"Not Found"} when the request path matches no route of the serving stage (e.g. a path sent without the required stage prefix on a named-stage API) and when the addressed stage has no deployment to serve. The fixed text is the single word pair "Not Found"; the body has no variable part. 403 with {"message":"Forbidden"} is AWS's answer for a missing/failed authentication or a missing API key, not for an unroutable path — so the emulator's refusal tells a client the wrong failure class.
Emulator: both cases answer 403 with body {"message":"Forbidden"}.
Why: a client or SDK retry/branching logic that distinguishes "this route does not exist / this stage is not deployed" (404) from "you are not authorized" (403) takes the authorization branch — it will go hunt for credentials or a resource policy for a request that was simply unrouted, and deployment-gating tests pass when they should fail.
Fix: the execute-api route-resolution step of the HTTP data plane: when no route key matches, or the resolved stage has no current deployment, emit 404 with the gateway's {"message":"Not Found"} body instead of falling through to the Forbidden refusal.
Traces: apigatewayv2.http-api-path-without-stage-prefix-not-delivered, apigatewayv2.http-api-stage-requires-deployment-before-serving.

Cause 3 of 7: with DisableExecuteApiEndpoint=true the refusal is the front door's generic dispatcher error, not a gateway refusal.
AWS: Smithy Api.DisableExecuteApiEndpoint — "Specifies whether clients can invoke your API by using the default execute-api endpoint". When it is turned on, a request to the default endpoint is refused by API Gateway itself and carries the gateway's JSON envelope ({"message":"Not Found"} per the recording), and turning it back off restores service on the same host.
Emulator: the request is not recognised as execute-api traffic at all; it answers the service-dispatcher envelope {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}. The fixed text here is wholly the emulator's — AWS never emits an "UnknownService" __type on an execute-api host.
Why: a client cannot distinguish a disabled default endpoint from a mis-addressed request; error handling keyed on the gateway's {"message":...} body fails to parse, and tooling that asserts the disable/restore lifecycle sees an unrelated protocol error.
Fix: the front-door host router (execute-api host matching) plus the data-plane handler's DisableExecuteApiEndpoint gate: the execute-api host must always resolve to the apigatewayv2 data plane, which then issues the gateway refusal.
Traces: apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore.

Cause 4 of 7: AuthorizationType=AWS_IAM is not enforced — an unsigned/invalid-signature request is served 200.
AWS: Smithy AuthorizationType enum includes AWS_IAM ("AWS_IAM for using AWS IAM permissions"). A route whose AuthorizationType is AWS_IAM is served only when the request carries a SigV4 signature that verifies and whose principal is permitted; a request whose signature does not verify is refused with 403 (gateway body {"message":"Forbidden"}), while the correctly signed twin request is served.
Emulator: the request whose SigV4 signature does not verify is served 200 with the integration's body (the check's own marker came back), i.e. no signature verification happens on the route at all.
Why: this is an overly-permissive auth bypass — code under test that relies on AWS_IAM to keep unsigned callers out passes against the emulator and is wide open in production terms; negative security tests can never fail.
Fix: the execute-api data-plane authorization step for routes with AuthorizationType=AWS_IAM (SigV4 verification of the invoke request before integration dispatch).
Traces: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served.

Cause 5 of 7: a proxy response with isBase64Encoded=true is neither decoded nor unwrapped — the handler's envelope reaches the client verbatim.
AWS: for an AWS_PROXY integration the gateway consumes the handler's response envelope (statusCode/headers/body/isBase64Encoded) and returns only `body` to the client; when isBase64Encoded is true the body is base64-decoded first, so arbitrary binary payloads (here the 256 raw bytes 0x00..0xFF) arrive byte-identical. Smithy: data-plane payload not modeled — recorded behavior only.
Emulator: the client receives the JSON envelope itself — 40 bytes beginning '{"isBase64Encoded": fals' — instead of the decoded body, so both the unwrapping and the base64 decode are missing on this path.
Why: any binary-returning route (images, protobuf, gzip) returns JSON text; clients get a Content-Type/Content-Length mismatch and corrupted payloads, and round-tripping binary through an HTTP API is impossible.
Fix: the AWS_PROXY response serializer in the HTTP data plane (envelope parsing + isBase64Encoded decoding before writing the client response).
Traces: apigatewayv2.http-api-v2-binary-body-base64-encoded-both-ways.

Cause 6 of 7: response parameter mapping overwrite:statuscode is ignored.
AWS: an integration's ResponseParameters keyed by the integration status code may carry overwrite:statuscode, which rewrites the client-visible status code while the integration itself answered its own status — in the recording ResponseParameters['200']['overwrite:statuscode'] = '201' makes the client see 201 for an integration that answered 200 (the unmapped twin route proves the integration answered 200).
Emulator: the client-visible status stays 200; the mapping is not applied. Request-side mappings in the same check are the same machinery.
Why: a client that keys on the status code (201 Created vs 200 OK, Location handling, cache behavior) sees the wrong code, and the documented parameter-mapping feature silently does nothing.
Fix: the HTTP data-plane integration-response stage: apply the integration's ResponseParameters (overwrite:/append:/remove: on statuscode, header.*, querystring.*) when serializing the client response.
Traces: apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response.

Cause 7 of 7: a CDK-synthesised HttpApi with a CORS configuration and a Lambda $default route answers 500 to a normal request.
AWS: the deployed $default stage serves GET / through the AWS_PROXY default route and returns the handler's 200 proxy response body (here the check's marker) with the CORS headers for an allowed Origin; the API serves until the stack is destroyed.
Emulator: GET / with an allowed Origin answers 500 with the emulator's generic body {"message":"Internal Server Error"} — the integration result never reaches the client. Related open issues 20261029T194243Z-apigatewayv2-2-net-new-root-causes-an-http.md and 20261027T053233Z-apigatewayv2-an-http-api-aws-proxy-route-still.md describe a 500 on hand-built AWS_PROXY routes; this check is not marked owned by either, and reaches the route via a CDK/CloudFormation-provisioned stack, so it is filed here and should be cross-checked against them.
Why: every CDK-provisioned HTTP API is unusable — no request can be served, so neither the CORS behavior nor the destroy leg of the lifecycle can be exercised, and a client sees an opaque 500 with no diagnosis.
Fix: the AWS_PROXY integration dispatch path of the HTTP data plane (and the stage/integration state a CDK-synthesised stack creates), so the integration's response is returned instead of a generic 500.
Traces: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys.

## Evidence
Cause 1 — expected vs actual (AssertionError text from the replay):
  http-api-named-stage-rawpath-includes-stage-prefix: "the $default stage adds no prefix; expected rawPath '/items', got '/$default/items'"
  http-api-route-key-precedence-and-path-parameters: "$default must still report the real rawPath, got /$default/items/list"
  import-api-payload-format-version-extension-honored: "a 2.0-format event must carry 'rawPath', got '/$default/echo-v2'"
  invoke-http-api-lambda-proxy: "expected the proxy event's rawPath to be the requested '/hello', got '/$default/hello'"
  lambda-payload-format-1-0-and-2-0-envelopes-differ: "format 1.0 must carry a top-level path '/v1echo', got '/$default/v1echo'"
  quick-create-lambda-target-invoked-directly-delivers-v2-event: "expected rawPath '/some/path', got '/$default/some/path'"

Cause 2 —
  http-api-path-without-stage-prefix-not-delivered: "bare host (no stage prefix): expected 404, got 403: '{\"message\":\"Forbidden\"}'"
  http-api-stage-requires-deployment-before-serving: "expected 404 from a stage with no deployment, got 403: {\"message\":\"Forbidden\"} (2xx => the emulator serves routes without a deployment)"
Expected: 404 + {"message":"Not Found"}. Actual: 403 + {"message":"Forbidden"}.

Cause 3 —
  deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore: "the refusal must be the gateway's {\"message\":\"Not Found\"} envelope, got '{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}'"

Cause 4 —
  deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served: "a request whose SigV4 signature does not verify must be refused with 403, got 200: 'sim2real-behav-iam-<marker>'" (the marker value is per-run; the requirement is 403 + {"message":"Forbidden"} for the bad-signature request and 200 for the signed twin).

Cause 5 —
  http-api-v2-binary-body-base64-encoded-both-ways: "a base64 response must reach the client decoded: expected the 256 raw bytes 0..255, got 40 bytes starting b'{\"isBase64Encoded\": fals'"

Cause 6 —
  parameter-mapping-rewrites-a-real-request-and-response: "ResponseParameters['200']['overwrite:statuscode'] = '201' must rewrite the client-visible status while the integration itself answered 200 (proved by the twin above), got 200"

Cause 7 —
  deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys: "GET / with an allowed Origin answered 500 '{\"message\":\"Internal Server Error\"}'" (expected 200 with the handler's body).

## Steps to Reproduce
Replay the behavioral suite and re-run the checks listed in trace_ids.

Cause 1: CreateApi(ProtocolType=HTTP) with an AWS_PROXY Lambda integration on route 'GET /items' (or ANY /{proxy+}), create/auto-deploy the $default stage, then GET https://{api-id}.execute-api.{region}.amazonaws.com/items and inspect the event the function received: rawPath (payload 2.0) / path (payload 1.0) must be '/items', not '/$default/items'. Repeat against a NAMED stage reached at /{stage}/items, where the stage prefix IS expected in rawPath.
Cause 2: (a) create an HTTP API served only on a named stage and GET the bare host path without the stage prefix; (b) create a stage with no deployment and GET a route path on it. Both must answer 404 {"message":"Not Found"}.
Cause 3: UpdateApi(DisableExecuteApiEndpoint=true), GET the default execute-api endpoint — refusal must be the gateway envelope, not an UnknownService dispatcher error — then set it back to false and confirm the same host serves again.
Cause 4: CreateRoute with AuthorizationType=AWS_IAM, invoke it once with a deliberately broken SigV4 signature (expect 403 {"message":"Forbidden"}) and once correctly signed with SigV4Auth (expect 200).
Cause 5: have the AWS_PROXY handler return {"statusCode":200,"isBase64Encoded":true,"body":base64(bytes(range(256)))} and assert the client receives exactly those 256 raw bytes.
Cause 6: on an integration, set ResponseParameters {"200": {"overwrite:statuscode": "201"}}, invoke the route, and assert the client sees 201 while an unmapped twin route on the same integration backend sees 200.
Cause 7: deploy the CDK-synthesised HttpApi stack (CORS + Lambda $default route) and GET / with an allowed Origin; expect 200 with the handler body and the CORS response headers, then destroy the stack.

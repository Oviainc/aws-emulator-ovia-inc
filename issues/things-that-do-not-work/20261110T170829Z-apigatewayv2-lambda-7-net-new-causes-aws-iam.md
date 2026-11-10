Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.http-api-cors-signed-iam-get-granted-unsigned-get-refused apigatewayv2__lambda.http-api-lambda-authorizer-raises-unauthorized-message-rejected-401 apigatewayv2__lambda.http-proxy-integration-overwrites-via-and-x-forwarded-for apigatewayv2__lambda.lambda-proxy-status-keyed-response-mapping-rewrites-500-to-403 apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500 apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502 apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected
Issue-Key: apigatewayv2__lambda.http-api-cors-signed-iam-get-granted-unsigned-get-refused|_poll#11|AssertionError|raise AssertionError(f"{description} not answered {want} within {timeout_s}s; last {last!r}")
Issue-Key: apigatewayv2__lambda.http-api-lambda-authorizer-raises-unauthorized-message-rejected-401|_poll_http#19|TimeoutError|raise TimeoutError(f"{description} ({url}) never reached an accepted status within {timeout_s:.0f}s. Last: {last!r}")
Issue-Key: apigatewayv2__lambda.http-proxy-integration-overwrites-via-and-x-forwarded-for|run#25|AssertionError|assert direct_headers.get("x-forwarded-for", "").startswith(_SPOOFED_XFF + ", "), (
Issue-Key: apigatewayv2__lambda.lambda-proxy-status-keyed-response-mapping-rewrites-500-to-403|_poll_http#20|TimeoutError|raise TimeoutError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500|run#17|AssertionError|assert status == 500, (
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502|run#13|AssertionError|assert status == 500, (
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda|_exchange_until#27|Timeout|_ws_send_text(sock, json.dumps(message))
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected|_exchange_until#19|Timeout|_ws_send_text(sock, json.dumps(message))
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# apigatewayv2__lambda: 7 net-new causes — AWS_IAM and Lambda-authorizer routes answer 500, X-Forwarded-For is not appended, status-keyed response mapping is inert, a missing invoke permission and a malformed proxy response are both served 200, and the WebSocket AWS-integration template scenarios reach no verdict

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Seven net-new root causes in the Lambda-backed apigatewayv2 data plane. Each must be closed before submitting.

Cause 1 of 7: a correctly SigV4-signed request to an AWS_IAM-authorized route is answered 500 instead of being served.
AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2) CreateRoute.AuthorizationType -> AuthorizationType enum NONE|AWS_IAM|CUSTOM|JWT ("AWS_IAM for using AWS IAM permissions"). A request whose SigV4 signature verifies and whose caller is granted execute-api:Invoke is dispatched to the integration and answered with the integration's own status; only an unsigned/non-verifying request is refused 403 {"message":"Forbidden"}.
Emulator: the signed, granted GET (with an Origin, so CORS headers were attached) was answered HTTP 500 with the gateway envelope {"message":"Internal Server Error"} for the whole polling budget.
Why: an IAM-authorized route is unusable — the positive path fails closed with a server error, so no SigV4-protected API can be exercised.
Fix: the AWS_IAM authorization branch of the execute-api data-plane handler (signature verification and execute-api:Invoke evaluation) — a verifying request must fall through to integration dispatch.
Traces: apigatewayv2__lambda.http-api-cors-signed-iam-get-granted-unsigned-get-refused.

Cause 2 of 7: a route behind a Lambda REQUEST authorizer that raises 'Unauthorized' is answered 500 instead of 401.
AWS: recorded behavior only (CreateAuthorizer/CreateRoute.AuthorizerId are modeled). A Lambda authorizer that raises the literal string 'Unauthorized' makes API Gateway refuse the request with HTTP 401 and the body {"message":"Unauthorized"} (fixed text); any other authorizer failure is a 500.
Emulator: the request is answered HTTP 500 {"message":"Internal Server Error"} and never reaches 401 within the budget.
Why: the documented way a Lambda authorizer signals an unauthenticated caller is lost — clients see a server error instead of 401 and cannot distinguish "bad credentials" from "broken API".
Fix: the authorizer-invocation branch of the execute-api data-plane handler — map the authorizer's 'Unauthorized' error to the 401 {"message":"Unauthorized"} envelope.
Traces: apigatewayv2__lambda.http-api-lambda-authorizer-raises-unauthorized-message-rejected-401.

Cause 3 of 7: the gateway does not append the real peer to a client-supplied X-Forwarded-For on a single AWS_PROXY hop.
AWS: recorded behavior only. Across one hop to an AWS_PROXY (Lambda) backend, the client's Via is delivered unmodified and the client's X-Forwarded-For chain is PRESERVED with the observed peer address APPENDED, so the backend sees '<client chain>, <peer-ip>'.
Emulator: the backend saw the client-claimed chain unchanged ('203.0.113.9, 198.51.100.7' with nothing appended), so the peer address is never recorded.
Why: a backend that trusts the last X-Forwarded-For element to be the real peer (rate limiting, geo/IP allowlisting, audit logging) reads a client-spoofed address instead.
Fix: the request-header construction of the AWS_PROXY integration request mapper in the execute-api data-plane handler (append the connection's peer address to X-Forwarded-For).
Traces: apigatewayv2__lambda.http-proxy-integration-overwrites-via-and-x-forwarded-for.

Cause 4 of 7: status-keyed integration ResponseParameters never rewrite a Lambda-proxy response.
AWS: Smithy CreateIntegration.ResponseParameters -> ResponseParameters, a map keyed by status code, documented "Supported only for HTTP APIs. You use response parameters to transform the HTTP response from a backend integration before returning the response to clients." With a mapping keyed on the backend's 500 that overwrites the status to 403, the client receives 403 even though the function answered 500.
Emulator: the client keeps receiving the function's 500 and its raw body ('{"origin": "lambda", "note": "deliberate-500"}') for the whole budget — the mapping is stored but never applied.
Why: response parameter mapping is inert, so an API that is supposed to translate backend failures into a client-facing contract leaks the backend's status and body.
Fix: response parameter mapping (status-code keyed selection, then overwrite:statuscode / header mappings) in the execute-api data-plane response path.
Traces: apigatewayv2__lambda.lambda-proxy-status-keyed-response-mapping-rewrites-500-to-403.

Cause 5 of 7: a route whose function grants API Gateway no lambda:InvokeFunction is served 200 instead of refused 500.
AWS: Smithy CreateIntegration.IntegrationType -> AWS_PROXY with IntegrationUri the function ARN. API Gateway invokes the function under its own service principal, so without a resource-policy grant for apigateway.amazonaws.com the call is denied and the client gets HTTP 500 {"message":"Internal Server Error"}.
Emulator: answered 200 with the function's body ('<marker>:/ungranted'), i.e. the target's resource policy is never evaluated.
Why: a Lambda-backed API that cannot work on AWS looks healthy in the emulator, and the data plane performs no authorization against its target.
Fix: resource-policy evaluation in the AWS_PROXY Lambda invoke path of the execute-api data-plane handler.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500.

Cause 6 of 7: a malformed Lambda proxy response is passed through 200 instead of being refused 500.
AWS: recorded behavior only. For an HTTP API (payload format 1.0 or 2.0), a function whose return value is not a valid proxy-response shape makes the gateway refuse the request with HTTP 500 and the envelope {"message":"Internal Server Error"} — an HTTP API answers 500 here, not REST's 502.
Emulator: answered 200 with the raw function return ('{"marker":"sim2real-xsvc-malformed-<id>"}'), so the proxy-response shape is never validated.
Why: a client cannot tell a broken handler from a successful call; malformed handler output silently becomes a 200 contract the API never promised.
Fix: proxy-response validation in the AWS_PROXY response mapper of the execute-api data-plane handler (both payload format versions), mapping a malformed shape to the 500 envelope.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502.

Cause 7 of 7: the WebSocket AWS (non-proxy) integration RequestTemplates scenarios never reach a verdict — both checks hang to the 60 s cap.
AWS: Smithy CreateIntegration.IntegrationType -> AWS, with RequestTemplates (TemplateMap), TemplateSelectionExpression and PassthroughBehavior (PassthroughBehavior enum). On a WEBSOCKET API the selected request template is rendered and the rendered body reaches the Lambda; with NEVER passthrough behavior a frame whose content type matches no template is refused instead of forwarded.
Emulator: both checks end with "Check timed out after 60.0s" — neither the rendered template arriving at the Lambda nor the NEVER-passthrough refusal is ever observed.
Why: WebSocket AWS-integration request templates and passthrough behavior cannot be exercised at all, so request transformation on WebSocket APIs is unverifiable.
Fix: the WEBSOCKET execute-api data plane's AWS (non-proxy) integration path — template selection/rendering and PassthroughBehavior=NEVER refusal — plus whatever makes these sessions hang instead of answering.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda, apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected.

## Evidence
Cause 1: "signed GET https://<host>/_aws/execute-api/<api-id>/items from https://app.example.com not answered 200 within 42.7s; last (500, {... 'access-control-allow-origin': 'https://app.example.com' ...}, '{\"message\":\"Internal Server Error\"}')".
Cause 2: "GET /unauthorized behind the authorizer raising 'Unauthorized' returning 401 ... never reached an accepted status within 39s. Last: (500, ..., '{\"message\":\"Internal Server Error\"}')".
Cause 3: "a single hop must keep the client's X-Forwarded-For chain and only APPEND the real peer; origin saw x-forwarded-for='203.0.113.9, 198.51.100.7'" (the exact chain the client sent, nothing appended).
Cause 4: "GET /mapped (the route carrying the status-keyed response mapping) ... never reached an accepted status within 46.4s. Last: 500 '{\"origin\": \"lambda\", \"note\": \"deliberate-500\"}'".
Cause 5: "a route whose Lambda grants API Gateway no lambda:InvokeFunction must be answered 500, got 200 with body '<marker>:/ungranted'".
Cause 6: "format 1.0 'dictbody': a malformed Lambda proxy response must be answered 500 (HTTP API, not REST's 502), got 200 with body '{\"marker\":\"sim2real-xsvc-malformed-<id>\"}'".
Cause 7: both WebSocket template checks report "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. CreateApi(ProtocolType='HTTP') with Lambda AWS_PROXY integrations, routes and a deployed stage.
2. Cause 1: set a route's AuthorizationType='AWS_IAM', grant execute-api:Invoke, and send a properly SigV4-signed GET with an Origin header — it answers 500.
3. Cause 2: attach a REQUEST Lambda authorizer that raises 'Unauthorized' and call the route — 500 instead of 401 {"message":"Unauthorized"}.
4. Cause 3: send Via and X-Forwarded-For headers to a one-hop AWS_PROXY route and echo event['headers'] — the chain is unchanged, no peer appended.
5. Cause 4: configure ResponseParameters keyed on the backend's 500 overwriting the status to 403 and invoke — the client still sees 500.
6. Cause 5: remove the function's apigateway.amazonaws.com invoke permission and call the route — 200 instead of 500.
7. Cause 6: return a non-proxy-shaped object from the function and call the route — 200 instead of 500.
8. Cause 7: CreateApi(ProtocolType='WEBSOCKET') with an AWS (non-proxy) integration carrying RequestTemplates and PassthroughBehavior='NEVER', then send frames — the checks hang to the 60 s cap.

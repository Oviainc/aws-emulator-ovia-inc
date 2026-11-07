Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401 apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500 apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502
Issue-Key: apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route|_poll_until#11|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas|_poll_until#8|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes|_poll_until#4|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake|_poll_until#2|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401|_poll_until#2|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda|_poll_until#8|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame|_poll_until#9|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda|_poll_until#23|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last: {last!r}")
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected|_poll_until#15|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last: {last!r}")
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500|run#16|AssertionError|assert status == 500, (
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502|run#12|AssertionError|assert status == 500, (
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigatewayv2__lambda: 3 net-new root causes — the WEBSOCKET data plane never upgrades (9 blocked checks), a missing lambda:InvokeFunction grant still serves 200, and a malformed Lambda proxy response is passed through as 200 instead of 500

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
execute-api WebSocket upgrade; execute-api GET through an AWS_PROXY Lambda integration

## Description
Three root causes observed under the apigatewayv2__lambda checks.

Cause 1 of 3: a deployed WEBSOCKET stage never completes the RFC 6455 upgrade, so no per-route Lambda, authorizer or template behaviour is reachable.
AWS: an upgrade to a deployed WEBSOCKET stage answers 101 Switching Protocols; $connect/$disconnect fire their own integrations, a $request REQUEST authorizer allows (passing its context to message routes), denies with a 403 handshake refusal, or answers 401 when the identity source is absent, the RouteSelectionExpression dispatches each action to its route, an unmatched action without $default is refused with a gateway error frame, and an AWS integration's RequestTemplates output is what reaches Lambda (with PassthroughBehavior NEVER refusing an unmatched content type). Recorded behavior only for the handshake itself; Smithy: Integration.RequestTemplates / TemplateSelectionExpression / PassthroughBehavior are the WebSocket members these routes rely on.
Emulator: no handshake ever reaches 101 — seven of these checks exhaust the 180 s harness cap with no verdict, and the two AWS-integration-template checks report 'WebSocket upgrade to stage ... returning 101 not met within 120.0s. Last: None', i.e. not even a response line was observed. Fixed part of the divergence: the absence of a 101 response to a valid upgrade; stage names and api ids are per-run values.
Why: every WebSocket capability of a Lambda-backed API is unusable, and nine distinct behaviours (route dispatch, $connect/$disconnect, authorizer allow/deny/401, unmatched action, request templates) are ungradeable.
Fix: the execute-api front door's WebSocket upgrade seam and the WEBSOCKET data-plane handler that dispatches routes to their Lambda integrations. Related open issues that do not carry these checks' replay keys: 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md, 20261029T194243Z-apigatewayv2-lambda-2-net-new-root-causes-the.md, 20261029T192401Z-apigatewayv2-lambda-three-more-websocket-checks-cannot-run.md.
Traces: apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame, apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda, apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected.

Cause 2 of 3: the integration does not evaluate the target function's resource policy — a route whose Lambda grants API Gateway no lambda:InvokeFunction is still served.
AWS: when the function's resource policy does not allow apigateway.amazonaws.com / the API's source ARN to call lambda:InvokeFunction, the HTTP API route answers 500 with body {"message":"Internal Server Error"} and the function is never invoked.
Emulator: the ungranted route answered 200 with the handler's own body, which proves the function ran and no resource policy was consulted.
Why: a permission error AWS refuses is silently allowed, so least-privilege wiring (AddPermission / AWS::Lambda::Permission) cannot be validated and a 'working' emulator run hides a broken deployment.
Fix: the AWS_PROXY integration invoke path in the execute-api data plane must evaluate the target function's resource policy and answer 500 when the invoke is denied.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500.

Cause 3 of 3: a malformed Lambda proxy response is passed through to the client as 200 instead of being refused with 500.
AWS: an HTTP API AWS_PROXY integration validates the function's return envelope. A response that is not a well-formed proxy envelope for its PayloadFormatVersion — no statusCode, a non-string body, a non-integer statusCode, or a bare string instead of an object — makes the gateway answer 500 with body {"message":"Internal Server Error"} (HTTP APIs answer 500 here, not REST's 502). Smithy: Integration.PayloadFormatVersion ('Supported values for Lambda proxy integrations are 1.0 and 2.0').
Emulator: the malformed envelope is forwarded as-is — the 'dictbody' case (statusCode 200 with a non-string body) answered 200 with the raw JSON object as the body, so no envelope validation happens at all.
Why: a client receives a body the gateway should never have produced and a success status for a failed integration, masking handler bugs that AWS surfaces as 500.
Fix: the AWS_PROXY response deserialiser/validator in the execute-api HTTP data plane — validate the envelope per payload format version and map any violation to 500 {"message":"Internal Server Error"}.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502.

## Evidence
Cause 1: seven checks report 'Check timed out after 180.0s'; the two template checks report "WebSocket upgrade to stage 'live' returning 101 not met within 120.0s. Last: None".
Cause 2: "attempt 1: a route whose Lambda grants API Gateway no lambda:InvokeFunction must be answered 500, got 200 with body 'sim2real-xsvc-perm-<id>:/ungranted' (200 means the target never evaluated the function's resource policy)".
Cause 3: "format 1.0 'dictbody': a malformed Lambda proxy response must be answered 500 (HTTP API, not REST's 502), got 200 with body '{\"marker\":\"sim2real-xsvc-malformed-<id>\"}'".

## Steps to Reproduce
Replay the checks in trace_ids. Cause 1: create a WEBSOCKET API with per-route Lambda integrations (plus a $request REQUEST authorizer and an AWS integration with RequestTemplates/PassthroughBehavior NEVER), deploy a stage, then open an RFC 6455 upgrade against it. Cause 2: create an HTTP API AWS_PROXY route whose function has no apigateway invoke permission and GET it (expect 500). Cause 3: create an AWS_PROXY route (PayloadFormatVersion 1.0 and 2.0) onto a handler that returns 'bare', 'dictbody', 'strstatus' and 'text' envelopes and GET each path (expect 500 for all malformed shapes).

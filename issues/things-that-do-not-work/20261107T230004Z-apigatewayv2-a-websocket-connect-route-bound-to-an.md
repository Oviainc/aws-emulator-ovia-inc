Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-connect-mock-integration-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.websocket-connect-mock-integration-rejected|_poll_until#30|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 7f27151474f220c715589d81820913dcd4b7b62b

# apigatewayv2: a WEBSOCKET $connect route bound to an AWS_PROXY Lambda integration is refused 500 "Internal server error", so the handshake never upgrades (1 cause)

> _Note: this issue was identified against the code base at commit `7f27151474f220c715589d81820913dcd4b7b62b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
One net-new root cause, grounded in one failing behavioral check. The divergence is in the execute-api WEBSOCKET data plane (the RFC 6455 opening handshake to a deployed stage), not in the control-plane call named above; the control-plane steps the check makes (CreateApi/CreateIntegration/CreateIntegrationResponse/CreateRoute/CreateRouteResponse/CreateStage/CreateDeployment/UpdateRoute/UpdateStage) all succeed.

Cause 1 of 1: the $connect connection request is refused with the UNPLACEABLE 500 for EVERY integration type, so a Lambda-backed $connect never upgrades.
Failing check ids: apigatewayv2.websocket-connect-mock-integration-rejected

AWS: recorded behavior only — the Smithy model covers only the apigatewayv2 control plane, so the handshake's status taxonomy is not modelled. What the model does state, and what the two legs of this scenario turn on, is Smithy: Integration$IntegrationType — "MOCK: for integrating the route or method request with API Gateway as a 'loopback' endpoint without invoking any backend. Supported only for WebSocket APIs" and "AWS_PROXY: for integrating the route or method request with a Lambda function or other AWS service action. This integration is also referred to as a Lambda proxy integration." The recording shows the two legs answering DIFFERENTLY: (a) a handshake to a deployed WEBSOCKET stage whose $connect route targets a MOCK integration fails the handshake with HTTP 500 and body {"message":"Internal server error"} — API Gateway cannot serve a connection request from a loopback integration; (b) after the SAME $connect route is re-pointed (UpdateRoute) at an AWS_PROXY integration whose IntegrationUri is a Lambda that returns {"statusCode": 200}, a new deployment is created and the stage is re-pointed at it, the SAME client's handshake is answered HTTP/1.1 101 Switching Protocols with the Sec-WebSocket-Accept derived from the client's Sec-WebSocket-Key, after which the untouched MOCK integration still serves the ordinary `whoami` route with a text frame and a client close frame is answered with a close frame. The fixed text of the refusal is the whole body {"message":"Internal server error"} (it carries no request-derived values); the 500 status is the requirement, and it must be produced ONLY for the configurations AWS cannot serve.

Emulator: both legs are answered identically — HTTP/1.1 500 Internal Server Error, content-type: application/json, a 35-byte body (the `{"message":"Internal server error"}` sentence; only date/host/port differ per run) — and the Lambda leg keeps answering 500 for the full 90 s the check polls, so no 101 is ever produced. In services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/websocket.rs the serve path derives `admits` from `dispatch::connection_request(&deployment.configuration)` and emits the unplaceable 500 whenever that is `Err(_unserved)`; an AWS_PROXY $connect lands in exactly that arm because no deployment package runs in this emulator, so the refusal is indiscriminate: the per-integration-type distinction AWS draws on $connect (MOCK fails, AWS_PROXY admits) does not exist here. The MOCK leg of this check therefore "passes" only by coincidence — the same answer is given for a configuration AWS serves.

Why: a client relying on AWS's behavior cannot distinguish an unsupported $connect integration from a correctly configured Lambda-backed one — both are a permanent 500 — and every WebSocket client whose $connect Lambda authorizes the connection never gets a socket, so no route selection, no @connections push and no $disconnect ever run. A negative test for "MOCK on $connect is rejected" also passes vacuously, hiding the defect.

Fix: the WEBSOCKET execute-api data plane's connection-request dispatch and its status mapping — the `admits` computation and `dispatch::connection_request` behind it, and the transport that must answer 101 once a connection request is admitted. Note the module's own docs state that a handshake which passes every resolution step is refused 501 NO_WEBSOCKET_TRANSPORT because the emulator's buffered front door has no upgrade seam; that capability gap is the subject of the open reports 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md and 20261107T193414Z-apigatewayv2-the-websocket-execute-api-data-plane-answers.md. Neither carries this check's replay key, and neither covers this cause: closing the transport alone would still leave this handshake at 500, because `admits` is already false before any transport is reached.

Traces: apigatewayv2.websocket-connect-mock-integration-rejected (the MOCK leg passes, the Lambda-backed $connect leg is the failure).

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

[mismatch] apigatewayv2.websocket-connect-mock-integration-rejected
  File "check_apigatewayv2_websocket_connect_mock_integration_rejected.py", line 139, in run
    conn = _poll_until(...)
  AssertionError: a 101 upgrade at wss://<host>/_aws/execute-api/<api-id>/<stage> once the Lambda-backed $connect redeploy propagates not met within 90.0s.
  Last result: {'status': 500, 'head': b'HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: 35\r\ndate: <per-run>'}

Expected (AWS): HTTP/1.1 101 Switching Protocols with the derived Sec-WebSocket-Accept, once $connect targets the AWS_PROXY Lambda integration returning {"statusCode": 200} and the stage points at the new deployment.
Actual (emulator): HTTP/1.1 500 Internal Server Error, content-type: application/json, body {"message":"Internal server error"} (35 bytes), repeated for 90 s.
The host, port, api id, stage name, content-length and date in the line above are per-run values and are not requirements; the 500 status, the JSON body sentence, and the fact that it is given for an AWS_PROXY $connect are.
The earlier assertion in the same check — a MOCK $connect must fail the handshake with 500 — holds, so the emulator answers the SAME 500 for both configurations.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi with ProtocolType=WEBSOCKET and RouteSelectionExpression=$request.body.action.
3. CreateIntegration with IntegrationType=MOCK (RequestTemplates {"$default": "{\"statusCode\": 200}"}), CreateIntegrationResponse for $default, then CreateRoute RouteKey=$connect targeting it, plus CreateRoute RouteKey=whoami targeting the same integration with a $default route response.
4. CreateStage + CreateDeployment; wait until the deployment reports DEPLOYED.
5. Open an RFC 6455 handshake at the stage's WebSocket URL: observe HTTP 500 {"message":"Internal server error"} (this leg matches AWS).
6. Create a Lambda returning {"statusCode": 200}, AddPermission for apigateway.amazonaws.com, CreateIntegration with IntegrationType=AWS_PROXY / IntegrationUri=<function arn>, UpdateRoute the $connect route onto it, CreateDeployment again and UpdateStage onto the new DeploymentId.
7. Repeat the handshake: the emulator still answers HTTP 500 {"message":"Internal server error"} where AWS answers 101 Switching Protocols. Re-replay apigatewayv2.websocket-connect-mock-integration-rejected to confirm.

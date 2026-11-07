Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# A CloudFormation-deployed WebSocket API stage is unroutable: the wss upgrade is answered 404 UnknownService instead of a 101 switch

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke ($connect, execute-api WebSocket stage)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: a WebSocket upgrade request to a CloudFormation-deployed AWS::ApiGatewayV2::Api (ProtocolType WEBSOCKET) stage is not attributed to API Gateway at all, so the connection is never established and no RouteSelectionExpression dispatch can happen.
Failing check ids for this cause: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action.

AWS: recorded behavior only — the execute-api WebSocket data plane is not described by the ApiGatewayV2 control-plane Smithy model (the modelled setup is CreateApi with ProtocolType=WEBSOCKET and RouteSelectionExpression, CreateIntegration, CreateRoute for $connect/$default/named actions, CreateDeployment/CreateStage). The recording shows the wss:// handshake on the deployed stage answered with an HTTP/1.1 101 Switching Protocols upgrade, after which each frame is dispatched to the route whose key matches the value the RouteSelectionExpression selects from the frame body, falling back to $default.
Emulator: the handshake is answered HTTP/1.1 404 Not Found with content-type application/x-amz-json-1.1 and x-amzn-errortype: UnknownService — the same generic "no service owns this address" refusal the emulator returns for an unattributable request. Its message text is fixed and names no API, stage or route. The upgrade never happens, so the check exhausts its 120 s budget without ever seeing a 101.
Why: no client can connect to a WebSocket API the emulator provisioned. Route selection, $connect authorization, $default fallback and the whole @connections callback surface are unreachable, and the failure is reported as a nonexistent endpoint rather than as anything API Gateway could have said.
Fix: the request-routing front door must recognise the execute-api WebSocket stage address the emulator publishes as the API's wss endpoint (the /_aws/execute-api/<api-id>/<stage> path form) and hand it to the ApiGatewayV2 execute-api data plane, which performs the 101 upgrade and then dispatches frames per RouteSelectionExpression.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action.

## Evidence
Expected (recorded AWS): a 101 WebSocket upgrade at the deployed stage's wss endpoint, then per-frame dispatch by RouteSelectionExpression.

Actual (emulator): AssertionError: a 101 WebSocket upgrade at wss://<host>/_aws/execute-api/<api-id>/p not met within 120.0s. Last result: {'status': 404, 'head': b'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149\r\n...'}

(The host, port and generated api-id differ per run and are not part of the requirement; the 404 status and the UnknownService error type are.)

## Steps to Reproduce
1. CreateStack with AWS::ApiGatewayV2::Api (ProtocolType WEBSOCKET, RouteSelectionExpression $request.body.action), integrations, $connect/$default/named routes, a Deployment and a Stage.
2. Wait for CREATE_COMPLETE and read the stage's wss endpoint.
3. Open a WebSocket handshake against it.
4. Observe HTTP/1.1 404 Not Found with x-amzn-errortype: UnknownService instead of 101 Switching Protocols.
Or replay the check id in trace_ids.

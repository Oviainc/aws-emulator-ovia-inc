Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route
Issue-Key: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route|_poll#14|Timeout|time.sleep(interval_s)
Issue-Commit: f4ad776043a8e85dca4073be4e52e068eea62419

# apigatewayv2__cloudformation__lambda: a SAM (AWS::Serverless-2016-10-31) HttpApi event never serves its route — the check cannot reach a verdict within the suite cap (1 cause)

> _Note: this issue was identified against the code base at commit `f4ad776043a8e85dca4073be4e52e068eea62419`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane, AWS_PROXY/Lambda integration) — reached through CloudFormation CreateStack of an AWS::Serverless-2016-10-31 template

## Description
Cause 1 of 1: a stack built from a SAM template (Transform: AWS::Serverless-2016-10-31, deployed with CAPABILITY_AUTO_EXPAND) that declares an `AWS::Serverless::HttpApi` plus an `AWS::Serverless::Function` with an `HttpApi` event never produces a servable route, so the check wedges and is killed at the suite's 60 s cap (`Error: Check timed out after 60.0s`). No verdict is reachable for any of the four behaviours the recording exercises.

AWS: recorded behavior only for the data plane (execute-api is not in the Smithy model); the Lambda half is modelled. The recording requires, in order: (1) CreateStack of the SAM template settles at CREATE_COMPLETE; (2) the SAM `Globals.Function` block reaches the generated function — lambda `GetFunctionConfiguration` on the declared FunctionName answers 200 with `MemorySize` equal to the template's `Globals.Function.MemorySize` (Smithy: GetFunctionConfiguration output member `MemorySize`, target MemorySize, @range min 128 max 32768; errors ResourceNotFoundException 404, InvalidParameterValueException 400); (3) `DescribeStacks` returns the `Endpoint` output with an `OutputValue` resolved from `!GetAtt Api.ApiEndpoint`; (4) an unsigned HTTPS `GET <ApiEndpoint>/hello` on the implicit `$default` stage answers `200` with the handler's JSON body, which is the 2.0 proxy envelope's own fields — `{"version": "2.0", "routeKey": "GET /hello", "rawPath": "/hello"}` (the SAM `HttpApi` event's default PayloadFormatVersion is 2.0, and `routeKey`/`rawPath` are taken from the declared Method+Path and the request path, not from any recorded run); (5) the same GET carrying an `Origin` header equal to the value in `Globals.HttpApi.CorsConfiguration.AllowOrigins` answers 200 WITH `access-control-allow-origin` echoing that origin, while a GET carrying any other `Origin` still answers 200 but WITHOUT that header at all; (6) a path the template declares no route for answers HTTP `404` with the body exactly `{"message":"Not Found"}` (fixed text; no variable part).

Emulator: none of that is observable. The check never converges on its first data-plane requirement and is terminated at 60 s, so every later assertion (the 2.0 envelope, the CORS echo, the foreign-origin suppression, the 404 `{"message":"Not Found"}`) is unreached. Hypothesis (neither the model nor the recording states which leg stalls, and the replay reports only the timeout): this is the SAM-transform instance of the same shape already filed for a CDK-synthesised template in `20261027T192238Z-apigatewayv2-lambda-cloudformation-a-cdk-synthesised-httpapi-stack-cannot-serve.md` — that issue documents that CloudFormation's `provisioner_for` (services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs) has no `AWS::Lambda::Function`/`AWS::Lambda::Permission` arm and that lambda `Invoke` cannot run a deployment package, so an AWS_PROXY route is answered `500 {"message":"Internal Server Error"}` and a 200 poll never accepts. That issue is keyed to a CDK/plain-CloudFormation template and does NOT cover this one: the SAM path additionally requires the `AWS::Serverless-2016-10-31` macro expansion itself — `AWS::Serverless::HttpApi` into an `AWS::ApiGatewayV2::Api` with the implicit `$default` stage and the `Globals.HttpApi.CorsConfiguration`, `AWS::Serverless::Function` with `InlineCode` into an `AWS::Lambda::Function` plus the generated `AWS::Lambda::Permission`, the `Globals.Function` defaults (Runtime, Handler, MemorySize) merged onto it, and the function's `HttpApi` event into a `GET /hello` route with an AWS_PROXY integration at PayloadFormatVersion 2.0.

Why: SAM is the ordinary way a Lambda-backed HTTP API is shipped, and `sam deploy` is just CreateStack with CAPABILITY_AUTO_EXPAND. A client that deploys this template against the emulator gets a stack it cannot call: the route does not answer 200, the handler never sees a 2.0 event, the API's CORS configuration is never exercised on a real answer, and an undeclared path does not produce the gateway's `404 {"message":"Not Found"}`. Because the whole flow hangs rather than refusing, a caller also gets no error it could act on.

Fix: three components, in the order the flow needs them — (a) the CloudFormation SAM/`AWS::Serverless-2016-10-31` transform expansion (the macro/transform step that feeds the stack provisioner), so `AWS::Serverless::HttpApi`, `AWS::Serverless::Function` + its `HttpApi` event and the `Globals` merge become the concrete ApiGatewayV2/Lambda resources, with `!GetAtt Api.ApiEndpoint` resolvable as an Output value; (b) the CloudFormation Lambda provisioner and an executable deployment package for `InlineCode` (owned by the open issues named above); (c) the apigatewayv2 execute-api AWS_PROXY dispatch and its CORS decoration on the served answer, plus the unmatched-route 404 body, in the apigatewayv2 data-plane invoke path. File and line citations for the known-missing provisioner arm are in the CDK issue cited above.

Traces: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route.

## Evidence
Failing check `apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route` — verdict [mismatch], detail: `Error: Check timed out after 60.0s`.

Expected (AWS):
- CreateStack(TemplateBody = Transform: AWS::Serverless-2016-10-31 template, Capabilities=[CAPABILITY_IAM, CAPABILITY_AUTO_EXPAND]) settles CREATE_COMPLETE.
- lambda GetFunctionConfiguration(FunctionName=<declared name>) -> 200 with MemorySize == Globals.Function.MemorySize (192 in the recording's template).
- DescribeStacks -> Outputs contains Endpoint with an OutputValue from !GetAtt Api.ApiEndpoint.
- GET <Endpoint>/hello with Origin = the template's AllowOrigins value -> 200, body {"version":"2.0","routeKey":"GET /hello","rawPath":"/hello"}, header access-control-allow-origin == that origin.
- GET <Endpoint>/hello with a foreign Origin -> 200 and NO access-control-allow-origin header.
- GET <Endpoint>/missing -> 404 with body exactly {"message":"Not Found"}.

Actual (emulator): the check never reaches the first 200 and is killed at the suite's 60 s cap; no status, body, CORS header or 404 from this flow is observable. The emulator produced no refusal either — the flow hangs.

## Steps to Reproduce
1. Take a SAM template: `Transform: AWS::Serverless-2016-10-31`; Parameters FunctionName, AllowedOrigin; `Globals.Function` = {Runtime: python3.13, Handler: index.handler, MemorySize: 192}; `Globals.HttpApi.CorsConfiguration` = {AllowOrigins: [!Ref AllowedOrigin], AllowMethods: [GET]}; Resources: `Api` of type AWS::Serverless::HttpApi, and `Fn` of type AWS::Serverless::Function with FunctionName: !Ref FunctionName, InlineCode returning {statusCode: 200, headers: {content-type: application/json}, body: json.dumps({version, routeKey, rawPath} taken from the event)}, and Events.Hello of Type HttpApi with ApiId: !Ref Api, Path: /hello, Method: GET; Outputs: Endpoint = !GetAtt Api.ApiEndpoint.
2. cloudformation CreateStack with that body, Capabilities=[CAPABILITY_IAM, CAPABILITY_AUTO_EXPAND], and poll DescribeStacks for CREATE_COMPLETE.
3. lambda GetFunctionConfiguration on the FunctionName parameter: expect MemorySize == 192.
4. DescribeStacks, read the Endpoint output, and GET <Endpoint>/hello with the allowed Origin: expect 200 with {"version":"2.0","routeKey":"GET /hello","rawPath":"/hello"} and access-control-allow-origin echoing the origin. Observe: no 200 ever arrives; the flow hangs until the harness's 60 s cap.
5. GET <Endpoint>/hello with a non-allowed Origin: expect 200 with no access-control-allow-origin header.
6. GET <Endpoint>/missing: expect 404 with body exactly {"message":"Not Found"}.
7. Replay apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route.

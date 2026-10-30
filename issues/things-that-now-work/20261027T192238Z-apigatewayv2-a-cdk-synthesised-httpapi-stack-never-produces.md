Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys|_http_until#37|Timeout|time.sleep(3.0)
Issue-Commit: bbbd5f0b3425ac8a27c3dccbe4215ebaac702410

# apigatewayv2: a CDK-synthesised HttpApi stack never produces a serving $default stage — the CORS/Lambda default-route deploy check reaches no verdict inside the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `bbbd5f0b3425ac8a27c3dccbe4215ebaac702410`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

The CORS leg of this report's `Fix` is implemented: an HTTP api's stored `CorsConfiguration`
now decorates whatever its deployed stage answers — an `Origin` its `AllowOrigins` admits is
echoed in `access-control-allow-origin` (the wildcard when that is what admitted it), a
preflight asking about an admitted method additionally carries `AllowMethods`, `AllowHeaders`
and `MaxAge`, an actual request additionally carries `ExposeHeaders`, either carries
`access-control-allow-credentials` when `AllowCredentials` is set, and a request whose origin
or asked-about method the configuration does not admit carries no `Access-Control-*` header
at all
(`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/invoke.rs`,
`cors_decoration`; pinned end to end by `tests/all/http_api_cors_data_plane.rs`). Before it,
nothing on the data-plane path read that member at all, so a browser could not tell a
configured api from one that had never been given a configuration.

What this report asked for and did NOT get: the `200` with the integration Lambda's body that
its check polls the stage endpoint for, and the `404 {"message": "Not Found"}` the same URL
must answer after the stack is deleted. Its replay key is therefore CARRIED FORWARD, so it
stays attributed to a cause instead of being lost:
`20261027T192238Z-apigatewayv2-lambda-cloudformation-a-cdk-synthesised-httpapi-stack-cannot-serve.md`.

## Operation
Invoke (execute-api data plane)

## Description
One net-new root cause, grounded in one failing behavioral check.

Cause 1 of 1 — a CDK-synthesised HTTP API (`HttpApi` with `corsPreflight` and a `$default` AWS_PROXY Lambda integration), deployed through plain CloudFormation from an asset-free LegacyStackSynthesizer template, never gets far enough for the check to make any assertion: the check is killed at the harness's 60 s cap with no stack convergence and no data-plane answer. Failing check: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys.

AWS: Smithy (com.amazonaws.apigatewayv2): CreateApi/GetApi carry `CorsConfiguration` of shape `Cors` — members `AllowOrigins` (jsonName allowOrigins), `AllowMethods` (jsonName allowMethods), `AllowHeaders`, `ExposeHeaders`, `AllowCredentials`, `MaxAge` @range min -1 max 86400; `Cors` is documented "Supported only for HTTP APIs". The data-plane leg is recorded behavior only: on AWS the stack reaches CREATE_COMPLETE, GetStages lists exactly `$default` with AutoDeploy true, and an unsigned HTTPS GET on the stage endpoint answers 200 with the integration Lambda's JSON body. A simple GET and a preflight OPTIONS whose Origin is in `AllowOrigins` carry `access-control-allow-origin` equal to that request's Origin and (for the preflight) `access-control-allow-methods` equal to the configured method list — the header values are taken from the request/the API's CorsConfiguration, nothing is fixed text here. A preflight whose Origin is not allow-listed, or whose `Access-Control-Request-Method` is not in `AllowMethods`, still answers 200 but carries neither header. After the stack is deleted the same URL answers HTTP 404 with body exactly `{"message": "Not Found"}` (fixed text, no variable parts), GetApi on the deleted api id raises `NotFoundException`, and lambda GetFunction raises `ResourceNotFoundException`.

Emulator: nothing is answered inside the check's budget. The replay verdict is only `Check timed out after 60.0s`: the check's bounded polls — DescribeStacks until a terminal CREATE state, then GET on the `$default` stage endpoint until 200 — never converge, so not a single assertion in the check is reached. The check calls `require_tools("cdk", "node")` before any AWS call and `cdk synth` is asserted to exit 0, so a missing runner tool would have failed the check with a tool/synth error rather than a timeout; the hang is in a poll against the emulator.

Why: the whole CDK → CloudFormation → serve → destroy shape (the normal way an HTTP API with a Lambda default route is shipped) is unusable: a client gets no 200 from its deployed stage, no CORS decoration or allowlist refusal it can observe, and no observable 404 `{"message": "Not Found"}` proving the API stopped serving after teardown. Because the emulator neither serves nor refuses, callers hang on their own deadlines instead of receiving a status they can branch on.

Fix: the apigatewayv2 execute-api data-plane entry point and its stage/route dispatch, plus the stage's AWS_PROXY integration invocation path, must answer requests for a stage created by a CloudFormation-deployed API; the API's CorsConfiguration must be applied on that same path (decorating allow-listed Origin/method, omitting both headers otherwise), and a stage whose API has been deleted must answer 404 `{"message": "Not Found"}`. Related OPEN issues 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md and 20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md report a non-serving stage from other checks but do not carry this check's replay key, so neither closes it.

Traces: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys.

## Evidence
Failing check apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys — verdict [mismatch], detail: `Error: Check timed out after 60.0s`.

Expected (AWS): stack CREATE_COMPLETE; GetStages == {$default, AutoDeploy=true}; GET <stage-endpoint>/ -> 200 with the Lambda body {"marker": <marker from the request>, "path": "/", "method": "GET"}; GET with Origin=<allow-listed origin> -> 200 + access-control-allow-origin: <that origin>; OPTIONS with Origin=<allow-listed origin> + Access-Control-Request-Method: GET -> 200 (forwarded through the $default catch-all to the Lambda) + access-control-allow-origin: <that origin> + access-control-allow-methods: GET; OPTIONS from a non-allow-listed origin or for a non-allow-listed method -> 200 with NEITHER header; after DeleteStack: GET -> 404 body exactly {"message": "Not Found"}, GetApi -> NotFoundException, lambda GetFunction -> ResourceNotFoundException.

Actual (emulator): no response observed at all — the check made no assertion before the 60 s harness cap; its own deadlines (stack create 300 s, serve poll 90 s) were still running, i.e. neither a terminal stack state nor a 200 from the stage endpoint was produced.

## Steps to Reproduce
1. `cdk synth` (LegacyStackSynthesizer, no bootstrap) an app containing: a python3.12 inline-code Lambda, an `apigatewayv2.HttpApi` with `corsPreflight={allow_origins:["https://example.test"], allow_methods:[GET]}` and `default_integration=HttpLambdaIntegration(fn)`, plus the CDK-generated AWS::Lambda::Permission (Principal apigateway.amazonaws.com, Action lambda:InvokeFunction).
2. CreateStack with that template (Capabilities=[CAPABILITY_IAM]) and poll DescribeStacks: expect CREATE_COMPLETE, and stack Outputs ApiId/ApiEndpoint/FunctionName.
3. GetStages(ApiId): expect exactly `$default` with AutoDeploy true.
4. Unsigned GET on <ApiEndpoint>/: expect 200 with the Lambda's JSON body. The emulator returns nothing within the poll window.
5. Repeat the GET with Origin: https://example.test and an OPTIONS preflight with Access-Control-Request-Method: GET: expect 200 plus access-control-allow-origin (and access-control-allow-methods on the preflight); repeat with Origin: https://evil.example.test and with Access-Control-Request-Method: DELETE: expect 200 with neither header.
6. DeleteStack, wait for DELETE_COMPLETE, then GET the same URL: expect 404 with body exactly {"message": "Not Found"}.

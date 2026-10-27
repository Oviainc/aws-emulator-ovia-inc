Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served|run#20|AssertionError|assert (status, text) == (200, marker), (
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves|run#19|AssertionError|assert twin_status == 200, f"GET /twin should serve 200, got {twin_status}: {twin_text[:200]!r}"
Issue-Commit: 9b14291c7f6c8074eee98500db9898b39cf3f43f

# apigatewayv2: an HTTP API AWS_PROXY route still answers 500 Internal Server Error, so even a route with AuthorizationType=NONE never serves (1 cause)

> _Note: this issue was identified against the code base at commit `9b14291c7f6c8074eee98500db9898b39cf3f43f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api HTTP data plane, AWS_PROXY integration)

## Description
REOPENS 20261025T113951Z-apigatewayv2-http-api-invoke-url-answers-500-for.md: its fix left the call refused. That report is marked fixed, but both of its Cause-1 replay keys fail again on this run with the identical assertion, and its own status note states Cause 1 was not fixed and attributes the keys to open issues it names. This is one cause, re-grounded in the current replay.

=== Cause 1 of 1: an AWS_PROXY (Lambda proxy) route answers 500 {"message":"Internal Server Error"} instead of the integration's response ===
AWS: recorded behavior only for the data plane (execute-api has no Smithy model); the setup is modeled: CreateIntegration input IntegrationType (enum, required), PayloadFormatVersion, TimeoutInMillis @range 50..30000; CreateRoute AuthorizationType; CreateStage AutoDeploy. Recorded behavior: a deployed HTTP API route whose AWS_PROXY integration targets a Lambda function returning {"statusCode": 200, "body": <marker>} serves HTTP 200 with that body, and a route whose AuthorizationType is NONE serves an UNSIGNED request. A route whose integration exceeds TimeoutInMillis answers 503, while its twin route with a 30000 ms timeout keeps serving 200 — so the 200 on the control route is the precondition for the timeout behaviour being observable at all.
Emulator: the invoke URL accepts the request and resolves a route, then answers HTTP 500 with the body {"message":"Internal Server Error"} for every AWS_PROXY route: GET /open (AuthorizationType=NONE, marker backend) returns 500 instead of 200 + the marker, and the control route GET /twin (TimeoutInMillis=30000, same backend) returns 500 instead of 200. Status 500 and the body {"message":"Internal Server Error"} are the divergence; AWS returns 200 with the function's own body. The blocking dependency is named in the reopened report: Lambda Invoke answers 501 for RequestResponse (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs), so no deployment package can run.
Why: a client gets an unconditional 5xx from every HTTP API backed by a Lambda proxy integration — the single most common API Gateway v2 configuration. 500 is indistinguishable from a genuine backend fault, so callers retry requests AWS would have served, and every behaviour layered on a served response (authorization outcomes, integration timeouts, payload-format mapping) is untestable.
Fix: the execute-api HTTP data-plane handler for AWS_PROXY integrations — proxy-event construction, the Lambda invocation itself (blocked on lambda Invoke executing a deployment package for RequestResponse), the proxy-response translation back to an HTTP response, and the error mapping that currently collapses an internal failure into 500 for a well-formed request.
Traces: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served, apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves. Related open issues that carry the same cause for OTHER checks and did not claim these keys on this run: 20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md, 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md.

## Evidence
Expected: GET <invoke-url>/open (AWS_PROXY to a Lambda returning {"statusCode":200,"body":<marker>}, route AuthorizationType=NONE, unsigned request) => 200 with the marker body; GET <invoke-url>/twin (same backend, TimeoutInMillis=30000) => 200.
Actual (this run): both answer 500 with body '{"message":"Internal Server Error"}'.
  - AssertionError: GET /open (AuthorizationType=NONE) must serve an unsigned request with marker 'sim2real-behav-iam-<run id>'; got 500: '{"message":"Internal Server Error"}'
  - AssertionError: GET /twin should serve 200, got 500: '{"message":"Internal Server Error"}'
These are the same two assertions listed in the Issue-Keys of the fixed report 20261025T113951Z-apigatewayv2-http-api-invoke-url-answers-500-for.md (run#20 and run#19).

## Steps to Reproduce
1. Boot the emulator from this checkout and point an SDK/CLI at it.
2. CreateApi(ProtocolType='HTTP'); create a Lambda function (python3.12, inline zip) returning {"statusCode":200,"body":"<marker>"} and grant apigateway.amazonaws.com lambda:InvokeFunction on it.
3. CreateIntegration(ApiId, IntegrationType='AWS_PROXY', IntegrationUri=<function arn>, PayloadFormatVersion='2.0', TimeoutInMillis=30000).
4. CreateRoute(ApiId, RouteKey='GET /open', AuthorizationType='NONE', Target='integrations/<integration id>') and a twin CreateRoute(RouteKey='GET /twin', Target=the same integration).
5. CreateStage(ApiId, StageName='$default', AutoDeploy=true).
6. GET <ApiEndpoint>/open unsigned and GET <ApiEndpoint>/twin: each must answer 200 with the function's marker body, not 500 {"message":"Internal Server Error"}.
7. Re-replay the two check ids in trace_ids to confirm.

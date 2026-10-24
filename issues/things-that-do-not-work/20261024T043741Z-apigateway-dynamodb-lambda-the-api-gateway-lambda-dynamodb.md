Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive|run#12|AssertionError|assert status == 200, f"Expected 200, got {status}: {resp_body}"
Issue-Key: apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__dynamodb__lambda: the API Gateway -> Lambda -> DynamoDB pipeline gets 404 UnknownService at the deployed stage, and the per-path resource-policy check hangs to the 60 s cap (2 causes)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Two root causes, neither carrying a key held by an open issue (20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md and 20261023T170254Z-apigateway-dynamodb-lambda-the-request-validator-negative-check.md describe adjacent causes for other keys).

Cause 1 of 2: the deployed stage's invoke URL is answered by the emulator's generic AWS-API front door.
AWS: a POST at the stage invoke URL of a REST API with an AWS_PROXY integration returns the Lambda's own response — HTTP 200 with the handler's JSON body, after which the item it wrote is readable from DynamoDB.
Emulator: HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}. The execute-api endpoint is not bound to the API Gateway data plane, so the request is classified as an unknown AWS service call.
Why: UnknownService is an error code API Gateway never emits, and 404 is not a status a matched proxy route can answer with; the whole serverless-ingest path (request mapping, Lambda invocation, DynamoDB write) is unobservable.
Fix: the emulator's front-door request routing — recognise the execute-api stage endpoint and dispatch it to the API Gateway data plane instead of the SigV4/X-Amz-Target service resolver.
Traces: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive.

Cause 2 of 2: a request at a path the Lambda resource policy does not cover never produces a response.
AWS: with a lambda:InvokeFunction permission whose SourceArn covers only one method/path, a request to another path is refused by API Gateway with an HTTP response (a 5xx integration failure) while the covered path serves 200; in both directions a status line arrives.
Emulator: the check reports exactly 'Check timed out after 60.0s' — no response at all, so the denial is never observed.
Why: a hang stalls the caller instead of failing it, and the per-path scoping of the Lambda invoke permission cannot be verified.
Fix: the execute-api data-plane request path for a deployed stage (it must always produce a response) and the per-method/path source-ARN check before invoking the integration's function.
Traces: apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path.

## Evidence
Cause 1: AssertionError: Expected 200, got 404: {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}.
Cause 2: Error: Check timed out after 60.0s — no HTTP status line from the stage.

## Steps to Reproduce
1. create_rest_api -> create_resource -> put_method -> put_integration(type='AWS_PROXY', uri=<lambda>) -> create_deployment(stageName=...) and lambda.add_permission for API Gateway.
2. POST JSON at the stage invoke URL -> observe HTTP 404 with the UnknownService body instead of the handler's 200 (cause 1).
3. Scope add_permission's SourceArn to a single method/path and request a different path -> observe no response within 60 s (cause 2).
4. Or replay the two check ids in trace_ids.

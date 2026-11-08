Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow|_assert_missing_write_policy_denies#24|AssertionError|assert status in {500, 502}, (
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__cloudwatch__dynamodb__lambda__logs: the SAM transform workflow's deployed stage answers 404 UnknownService, so the denied-write leg can never see a 5xx (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to the deployed stage's invoke URL is answered by the emulator's generic AWS-API front door instead of the API Gateway data plane.
AWS: a deployed REST API stage serves the integration's response at its invoke URL. When the backing Lambda's PutItem is denied by its execution role, the handler raises and API Gateway surfaces an integration failure as HTTP 500 (or 502 for a malformed integration response) — recorded behavior for the exact status, structural fact that the response comes from the stage and is never a service-resolution error.
Emulator: HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the execute-api stage endpoint is not bound to the API Gateway data plane, so the request is classified as an unknown AWS service call.
Why: UnknownService is an error code API Gateway never emits, and 404 is not in the set of statuses a deployed stage can answer with; a client cannot observe the integration at all, let alone the IAM-denial behaviour the check is about. The SAM-deployed application appears to have no endpoint.
Fix: the emulator's front-door request routing — recognise the execute-api stage endpoint (host/path form) and dispatch it to the API Gateway data plane instead of the SigV4/X-Amz-Target service resolver. Open issue 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md describes this cause for other keys but does not carry this one.
Traces: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow.

## Evidence
AssertionError: a handler whose PutItem is denied cannot answer 404: '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}' (assert status in {500, 502}).

## Steps to Reproduce
1. create_stack from the SAM template (Transform: AWS::Serverless-2016-10-31) that wires an API event to a Lambda writing to DynamoDB, with the write policy omitted.
2. POST to the deployed stage's invoke URL.
3. Observe HTTP 404 with the UnknownService body instead of a 500/502 from the failing integration.
4. Or replay apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow.

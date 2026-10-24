Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape|_get_stage_json#10|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording|_get_stage_json#12|AssertionError|raise AssertionError(
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__dynamodb__lambda__s3__xray: both X-Ray trace-shape checks get 404 UnknownService from the deployed stage, so no trace is ever started (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: the execute-api stage endpoint is answered by the emulator's generic AWS-API front door instead of the API Gateway data plane.
AWS: a GET at a deployed stage's invoke URL returns the integration's response (HTTP 200 with the handler's JSON here), and the request is sampled into X-Ray — an AWS::ApiGateway::Stage segment with the Lambda, DynamoDB and S3 subsegments beneath it, honouring an inbound X-Amzn-Trace-Id Root=...;Sampled=1 continuation and refusing to record Sampled=0.
Emulator: every attempt answers HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the request is handed to the service resolver, which cannot classify the execute-api host/path, so neither check ever gets a traced request.
Why: UnknownService is an error code API Gateway never emits and 404 is not a status a matched route answers with; distributed tracing of an API-fronted application (trace-tree shape and header continuation) is entirely unobservable.
Fix: the emulator's front-door request routing — recognise the execute-api stage endpoint and dispatch it to the API Gateway data plane, which must also propagate/mint the X-Amzn-Trace-Id header. Open issues 20261023T170254Z-apigateway-dynamodb-lambda-s3-xray-both-x-ray.md (UpdateStage 501) and 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md (cause 2) describe adjacent causes but carry neither of these keys.
Traces: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape, apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording.

## Evidence
AssertionError: the API Gateway stage never served '<stage invoke URL>/traced/x?...' with headers None: last status 404 body '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'. Same body for the request carrying {'X-Amzn-Trace-Id': 'Root=1-...;Sampled=1'}.

## Steps to Reproduce
1. create_rest_api with a Lambda AWS_PROXY integration whose handler writes to DynamoDB and S3; create_deployment to a stage with tracingEnabled.
2. GET the stage invoke URL, once without and once with X-Amzn-Trace-Id: Root=<id>;Sampled=1.
3. Observe HTTP 404 with the UnknownService body instead of the handler's JSON, so no X-Ray trace is produced.
4. Or replay the two check ids in trace_ids.

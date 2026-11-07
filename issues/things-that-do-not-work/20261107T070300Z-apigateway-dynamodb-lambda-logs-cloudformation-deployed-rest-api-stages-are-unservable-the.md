Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-dynamodb-direct-integration-positive apigateway__dynamodb__lambda__logs.deploy-cfn-missing-lambda-permission-for-apigateway-negative apigateway__dynamodb__lambda__logs.deploy-cfn-serverless-api-full-stack-positive apigateway__dynamodb__lambda__logs.deploy-missing-lambda-permission-blocks-apigateway-invoke-negative apigateway__dynamodb__lambda__logs.deploy-wrong-integration-uri-negative
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive|_request_until_logged#10|AssertionError|assert status == 200, f"expected 200 from GET {url} on attempt {attempts}, got {status}: {body[:300]!r}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-dynamodb-direct-integration-positive|_poll_until#6|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-missing-lambda-permission-for-apigateway-negative|_poll_until#7|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-serverless-api-full-stack-positive|_poll_until#6|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-missing-lambda-permission-blocks-apigateway-invoke-negative|run#8|AssertionError|assert status == 500, f"expected 500 (Lambda unreachable, no permission), got {status}: {body}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-wrong-integration-uri-negative|run#11|AssertionError|assert status == 500, f"expected 500 (integration target does not exist), got {status}: {body}"
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# CloudFormation-deployed REST API stages are unservable: the execute-api invoke URL answers 404 UnknownService, so six template-deploy checks never reach their data plane

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
REOPENS 20261023T114649Z-apigateway-api-gateway-execute-api-stage-endpoints-are-not.md: its fix left the call refused. That fix routes the execute-api data plane for REST APIs created through `CreateRestApi`, but every REST API this suite creates through a CloudFormation TEMPLATE is still refused at its stage invoke URL.

Cause 1 of 1 — a request to the stage invoke URL of a REST API that a CloudFormation stack created is not resolved as an execute-api data-plane request, because the api the stack reports does not exist in API Gateway.

AWS: an HTTP request to a deployed stage's invoke URL (`https://<api-id>.execute-api.<region>.amazonaws.com/<stage>/<path>`, which this harness addresses as `/_aws/execute-api/<api-id>/<stage>/<path>`) is matched to the resource/method and answered with the integration's response. A template that declares `AWS::ApiGateway::RestApi` + `::Resource` + `::Method` + `::Deployment` + `::Stage` and reaches CREATE_COMPLETE has really created those resources, `Ref RestApi` is the id API Gateway minted, and the stage answers: 200 for a healthy AWS_PROXY or AWS (DynamoDB) integration, and 500 with the gateway body `{"message":"Internal server error"}` for a method whose integration target does not exist or whose Lambda grants API Gateway no `lambda:InvokeFunction` (the two negative checks assert exactly that 500). Smithy: there is no modeled operation for the execute-api data plane, so the 200/500 statuses above are recorded behavior only; `AWS::ApiGateway::Stage`'s `AccessLogSettings`/`MethodSettings` map onto the modeled `AccessLogSettings` (`format` must include `$context.requestId`, `destinationArn` is a log-group ARN) and `MethodSetting` shapes of the apigateway model.

Emulator: every one of those requests is answered HTTP 404 with body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` — the emulator's front-door service router does not recognise the path as execute-api, so no stage, method or integration is consulted. The status (404) and the `UnknownService` code/message are the emulator's own router text, not any AWS error: AWS never answers an execute-api request with `__type`/`UnknownService`. Fixed text here is `could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)`; it carries no value from the request at all, which is the tell — the api id and stage in the URL are never looked at. The six checks observe this as: `got 404` where 200 was expected (access-logging, DynamoDB direct integration, with-permission endpoint), `got 404` where 500 was expected (missing Lambda permission, wrong integration URI), and — where the probe maps any non-200 to None — `API endpoint returning 200 not met within 45.0s. Last result: None` (serverless full stack).

Why: a client that deploys an API with CloudFormation (the normal path, including SAM/CDK output) gets CREATE_COMPLETE and an `ApiId` output, then finds the api absent from the control plane and its stage unservable. Nothing downstream of the request can be exercised — the Lambda integration, the direct DynamoDB integration, the access-log and execution-log delivery, and the two permission-boundary refusals all sit behind this one 404.

Fix: the CloudFormation resource providers for `AWS::ApiGateway::RestApi` and its `::Resource`/`::Method`/`::Deployment`/`::Stage`/`::Account` children must create those resources through the API Gateway service and report the ids that service minted, so the already-working execute-api resolver can serve them; the stack must not report CREATE_COMPLETE for an api API Gateway does not hold. The open issue 20261103T000000Z-apigateway-cloudformation-a-template-deployed-rest-api-stage-url-answers-404-unknownservice.md describes this same root cause for a different check (and names the provisioner/template files and the merge-gate line limits that block them); it does not carry any of the six replay keys below, so this issue is filed to carry them.

Traces: the six failing checks in trace_ids, all in the apigateway__dynamodb__lambda__logs suite.

## Evidence
deploy-cfn-apigateway-access-logging-enabled-positive: AssertionError: expected 200 from GET https://<host>/_aws/execute-api/<api-id>/prod/ping on attempt 1, got 404: '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'

deploy-cfn-apigateway-dynamodb-direct-integration-positive: AssertionError: direct-integration PutItem call succeeds not met within 45.0s. Last result: (404, '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}')

deploy-cfn-missing-lambda-permission-for-apigateway-negative: AssertionError: withperm endpoint reachable not met within 45.0s. Last result: (404, '{"__type":"UnknownService",...}')

deploy-cfn-serverless-api-full-stack-positive: AssertionError: API endpoint returning 200 not met within 45.0s. Last result: None (the probe maps every non-200 to None)

deploy-missing-lambda-permission-blocks-apigateway-invoke-negative: AssertionError: expected 500 (Lambda unreachable, no permission), got 404: {'__type': 'UnknownService', 'message': 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'}

deploy-wrong-integration-uri-negative: AssertionError: expected 500 (integration target does not exist), got 404: {'__type': 'UnknownService', 'message': 'could not resolve a target service from the request ...'}

Expected: 200 from the integration for the three positive checks; 500 `{"message":"Internal server error"}` for the two permission/integration-target negatives. Actual: 404 `UnknownService` from the emulator's service router in all six.

## Steps to Reproduce
1. Boot the emulator and replay the six checks listed in trace_ids.
2. Minimal repro: CreateStack with a template declaring `AWS::ApiGateway::RestApi`, a `::Resource`, a `::Method` (Integration Type AWS_PROXY at a Lambda, or Type AWS at DynamoDB), a `::Deployment` and a `::Stage` named `prod`, plus an Output `ApiId: !Ref RestApi`.
3. Wait for CREATE_COMPLETE and read the `ApiId` output.
4. Call `GetRestApi` with that id — it is not held by API Gateway.
5. GET `/_aws/execute-api/<ApiId>/prod/<path>` — the emulator answers 404 `{"__type":"UnknownService",...}` instead of the integration's 200, or the 500 `{"message":"Internal server error"}` that a missing Lambda permission / non-existent integration target must produce.

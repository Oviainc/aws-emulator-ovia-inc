Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-apigw-lambda-permission-missing-negative apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive apigateway__cloudformation__lambda.cfn-apigw-lambda-request-integration-mapping-positive apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative apigateway__cloudformation__lambda.cfn-apigw-role-trust-policy-positive apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive apigateway__cloudformation__lambda.cfn-apigw-wrong-integration-uri-negative
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-permission-missing-negative|run#9|AssertionError|assert bad_status == 500, f"expected 500 for missing permission, got {bad_status}: {bad_body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive|run#6|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-request-integration-mapping-positive|run#6|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative|run#9|AssertionError|assert bad_status == 500, f"expected 500 for an unassumable role, got {bad_status}: {bad_body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-role-trust-policy-positive|run#7|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive|run#8|AssertionError|assert status == 200, f"expected 200 from initial deployment, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-wrong-integration-uri-negative|run#12|AssertionError|assert bad_status == 500, f"expected 500 for a nonexistent target, got {bad_status}: {bad_body!r}"
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# The execute-api data plane refuses every call to a CloudFormation-deployed REST API stage with 404 UnknownService

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api deployed stage)

## Description
REOPENS 20261107T043755Z-apigateway-dynamodb-lambda-the-deployed-aws-proxy-stage.md: its fix left the call refused. That issue's Cause 1 required that "every request that reaches a deployed stage must terminate in a response"; requests now terminate, but with a refusal that says no service owns the address, so no stage invoke is served at all.

This issue carries 1 net-new root cause.

Cause 1 of 1: an HTTP request to the invoke URL of a CloudFormation-deployed AWS::ApiGateway::Stage/Deployment is not routed to API Gateway at all — the emulator's front door cannot attribute the request to a service and refuses it.
Failing check ids for this cause: apigateway__cloudformation__lambda.cfn-apigw-lambda-permission-missing-negative, apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive, apigateway__cloudformation__lambda.cfn-apigw-lambda-request-integration-mapping-positive, apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative, apigateway__cloudformation__lambda.cfn-apigw-role-trust-policy-positive, apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive, apigateway__cloudformation__lambda.cfn-apigw-wrong-integration-uri-negative.

AWS: recorded behavior only — the execute-api data plane is not described by the API Gateway control-plane Smithy model (the modelled setup is CreateRestApi / CreateResource / PutMethod / PutIntegration / CreateDeployment, plus lambda AddPermission for the apigateway.amazonaws.com principal). The recordings show two response classes at a deployed stage's invoke URL, and nothing else: (a) a correctly wired method answers HTTP 200 carrying the integration's mapped body — for AWS_PROXY the handler's own statusCode/headers/body, for a non-proxy integration the integration response template's rendering of the handler's return value; (b) a method whose integration cannot be invoked answers HTTP 5xx — AWS answers 500 with the fixed body {"message": "Internal server error"} and header x-amzn-ErrorType: InternalServerErrorException — for each of the three distinct causes the checks exercise: the target function has no API Gateway invoke resource policy, the integration's credentials role does not trust apigateway.amazonaws.com, and the integration URI names a function that does not exist. In every case the method is reachable and the response comes from API Gateway.
Emulator: every such request is answered HTTP 404 with content-type application/x-amz-json-1.1 and body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}. The whole sentence is fixed text — nothing in it is taken from the request — and it is the generic "no service owns this address" refusal, not an API Gateway response: the stage, its resources, its methods and the integration target are never consulted, so the 200 path and all three 5xx paths collapse onto one unrelated status and error code.
Why: a client of a CloudFormation-provisioned REST API cannot call it. The happy path returns no payload, and — worse for callers that depend on failure semantics — a missing lambda invoke permission, an untrusted credentials role and a dangling integration URI are all indistinguishable from "this endpoint does not exist", so nothing built on API Gateway's documented 500/InternalServerErrorException contract can be exercised or relied on. Stack updates that swap the integration target are equally unobservable.
Fix: the request-routing front door must attribute an execute-api stage-invoke URL to the API Gateway service (by host and/or the /_aws/execute-api/<rest-api-id>/<stage> path form it publishes as the invoke URL) and hand it to API Gateway's execute-api data-plane path, which then resolves the deployment/stage, the resource and method, authorizes and performs the integration, and maps the result (or the integration failure) back onto the HTTP response.
Traces: the seven check ids listed above; the CFN-provisioned happy path is cfn-apigw-lambda-proxy-integration-deploy-positive and the three refusal classes are cfn-apigw-lambda-permission-missing-negative, cfn-apigw-role-missing-trust-principal-negative and cfn-apigw-wrong-integration-uri-negative.

The open issue 20261023T125454Z-apigateway-cloudformation-provisioned-api-gateway-resources-are-unaddressable-through.md carries the replay key of cfn-apigw-stage-deployment-invocation-positive only; it does not carry any of the seven keys above.

## Evidence
Expected (recorded AWS): GET/POST to the deployed stage invoke URL answers HTTP 200 with the integration's body for a correctly wired method, and HTTP 500 with body {"message": "Internal server error"} / x-amzn-ErrorType: InternalServerErrorException when the integration cannot be invoked (missing lambda resource policy, credentials role that does not trust apigateway.amazonaws.com, integration URI naming a nonexistent function).

Actual (emulator), identical for all seven checks:
  HTTP 404, body b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'

Assertion lines as reported:
- cfn-apigw-lambda-proxy-integration-deploy-positive: AssertionError: expected 200, got 404: b'{"__type":"UnknownService",...}'
- cfn-apigw-lambda-request-integration-mapping-positive: AssertionError: expected 200, got 404
- cfn-apigw-role-trust-policy-positive: AssertionError: expected 200, got 404
- cfn-apigw-stack-update-swap-lambda-target-positive: AssertionError: expected 200 from initial deployment, got 404
- cfn-apigw-lambda-permission-missing-negative: AssertionError: expected 500 for missing permission, got 404
- cfn-apigw-role-missing-trust-principal-negative: AssertionError: expected 500 for an unassumable role, got 404
- cfn-apigw-wrong-integration-uri-negative: AssertionError: expected 500 for a nonexistent target, got 404

## Steps to Reproduce
1. CreateStack with AWS::ApiGateway::RestApi, a Resource, a Method with an AWS_PROXY integration onto an AWS::Lambda::Function, the matching AWS::Lambda::Permission, and an AWS::ApiGateway::Deployment + Stage.
2. Wait for CREATE_COMPLETE and read the stage invoke URL.
3. Issue a plain HTTP request to <invoke-url>/<resource-path>.
4. Observe 404 with {"__type":"UnknownService",...} instead of the integration's 200.
5. Repeat with the lambda permission removed, with a credentials role that trusts ec2.amazonaws.com instead of apigateway.amazonaws.com, and with an integration URI naming a nonexistent function: AWS answers 500/InternalServerErrorException for each, the emulator answers the same 404.
Or replay the seven check ids in trace_ids.

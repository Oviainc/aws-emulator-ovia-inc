Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive apigateway__cognito-idp__lambda.cfn-apigw-lambda-proxy-integration-resource-binding-positive
Issue-Key: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive|_poll_invoke_until#6|AssertionError|raise AssertionError(f"never observed status in {target_statuses}; last={last}")
Issue-Key: apigateway__cognito-idp__lambda.cfn-apigw-lambda-proxy-integration-resource-binding-positive|_poll_invoke_until#6|AssertionError|raise AssertionError(f"never observed status in {target_statuses}; last={last}")
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigateway__cognito-idp__lambda: a CloudFormation-provisioned REST API stage invoke URL answers 404 UnknownService, so the stack's role-trust and Lambda-proxy resource-binding checks never reach a method

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane)

## Description
Cause 1 of 1: a REST API created by a CloudFormation stack is never registered with the emulator's execute-api router, so its deployed stage is unreachable.

AWS: a stack-created AWS::ApiGateway::RestApi with a Resource, an AWS_PROXY (Lambda proxy) Method, a Deployment and a Stage serves requests at the stage invoke URL exactly like an SDK-created API — 200 with the handler's proxy response when the role trusts apigateway.amazonaws.com and the Lambda permission is in place. There is no `UnknownService` error on the execute-api data plane; an unmatched route is 403 `{"message":"Forbidden"}`.

Emulator: the stage invoke URL answers HTTP 404 with the emulator-invented body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`. The entire sentence is fixed emulator text — no part of it is AWS wording — and the only variable parts of the request are host/port, api id, stage and path. APIs created directly through the apigateway API are served at the same URL shape in the same run, so the gap is the stack-provisioning path, not the data plane's URL format.

Why: a client cannot invoke any template-deployed REST API, so neither the IAM role-trust behavior nor the resource→method→integration binding a stack produces can be exercised; both checks fail on the very first request.

Fix: the CloudFormation resource providers for AWS::ApiGateway::RestApi / Resource / Method / Integration / Deployment / Stage must register the provisioned api id and stage with the execute-api route table that the apigateway control-plane handlers populate. Related but NOT covering these checks: open issue 20261103T000000Z-apigateway-cloudformation-a-template-deployed-rest-api-stage-url-answers-404-unknownservice.md.

Traces: the two failing checks in trace_ids.

## Evidence
Expected: HTTP 200 from the stack's deployed stage.
Actual (cfn-apigateway-role-trusts-apigateway-service-positive): `AssertionError: never observed status in {200}; last=(404, '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}')`.
Actual (cfn-apigw-lambda-proxy-integration-resource-binding-positive): `AssertionError: never observed status in {200}; last=(404, '{"__type":"UnknownService",...}')`.
Status diverges (404 vs 200), the error code `UnknownService` is not an AWS error for this plane, and the message is emulator-authored in full.

## Steps to Reproduce
1. CreateStack with AWS::IAM::Role (trusting apigateway.amazonaws.com), AWS::Lambda::Function, AWS::Lambda::Permission, AWS::ApiGateway::RestApi, ::Resource, ::Method (AWS_PROXY to the function), ::Deployment and ::Stage.
2. Wait for CREATE_COMPLETE and resolve the stage invoke URL from the RestApi id and stage name.
3. GET that URL -> 404 `{"__type":"UnknownService",...}` instead of 200 with the Lambda proxy response.

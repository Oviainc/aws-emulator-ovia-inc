Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive apigateway__cloudformation__lambda.cfn-apigw-lambda-request-integration-mapping-positive apigateway__cloudformation__lambda.cfn-apigw-role-trust-policy-positive apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive apigateway__cloudformation__lambda.cfn-apigw-lambda-permission-missing-negative apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative apigateway__cloudformation__lambda.cfn-apigw-wrong-integration-uri-negative apigateway__cloudformation__lambda.apigw-integration-lambda-alias-mismatch-negative apigateway__cloudformation__lambda.cfn-custom-resource-signals-failure-stack-rollback-negative
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive|run#6|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-request-integration-mapping-positive|run#6|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-role-trust-policy-positive|run#7|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive|run#8|AssertionError|assert status == 200, f"expected 200 from initial deployment, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-permission-missing-negative|run#9|AssertionError|assert bad_status == 500, f"expected 500 for missing permission, got {bad_status}: {bad_body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative|run#9|AssertionError|assert bad_status == 500, f"expected 500 for an unassumable role, got {bad_status}: {bad_body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-wrong-integration-uri-negative|run#12|AssertionError|assert bad_status == 500, f"expected 500 for a nonexistent target, got {bad_status}: {bad_body!r}"
Issue-Key: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-mismatch-negative|run#22|AssertionError|assert good_resp["status"] == 200, f"positive control: real alias must invoke, got {good_resp}"
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-signals-failure-stack-rollback-negative|run#16|Timeout|time.sleep(5.0)
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# CloudFormation-deployed REST APIs: invoking the stage URL answers 404 UnknownService instead of routing to the API (3 root causes)

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
This issue bundles 3 NET-NEW root causes affecting REST APIs provisioned by CloudFormation together with Lambda. A fixer must close all 3 before submitting.

Cause 1 of 3: an HTTPS request to the execute-api URL of a CloudFormation-deployed stage is answered by the emulator's service router, not by API Gateway.
AWS: a request to https://<api-id>.execute-api.<region>.amazonaws.com/<stage>/<path> is routed to that REST API's stage and answered by the configured integration — HTTP 200 for a working proxy integration, HTTP 500 for a missing lambda:InvokeFunction permission, an unassumable integration role, or an integration URI naming a nonexistent function.
Emulator: answers HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the request never reaches the API Gateway data plane at all. The fixed text is the __type 'UnknownService' and that message; it is a generic front-door refusal and names no API.
Why: no API deployed through CloudFormation can be called over HTTP, so both positive deployments and the negative permission/trust/URI cases are indistinguishable from each other and from a nonexistent endpoint.
Fix: execute-api host/path routing in the front-door request router, so an execute-api style request resolves the REST API from the host or path instead of requiring a SigV4 credential scope / X-Amz-Target.
Traces: apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive, apigateway__cloudformation__lambda.cfn-apigw-lambda-request-integration-mapping-positive, apigateway__cloudformation__lambda.cfn-apigw-role-trust-policy-positive, apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive, apigateway__cloudformation__lambda.cfn-apigw-lambda-permission-missing-negative, apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative, apigateway__cloudformation__lambda.cfn-apigw-wrong-integration-uri-negative

Cause 2 of 3: an AWS_PROXY integration pointing at a Lambda alias cannot be invoked, so even the positive control fails.
AWS: TestInvokeMethod on an AWS_PROXY method whose uri names a real function alias returns the function's response (status 200 for a working handler).
Emulator: returns status 500 with body {"message":"Internal server error"} and the execution log 'Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call: no deployment package runs in process and no outbound connection is made, so no backend answer exists to map' then 'Answered through this API's API_CONFIGURATION_ERROR gateway response'. The open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describes the Lambda-side refusal but does not carry this check.
Why: an alias-targeted proxy integration can never succeed, so an alias mismatch cannot be distinguished from a correct alias.
Fix: the AWS_PROXY integration backend dispatch in the invoke pipeline (and the Lambda invoke path it calls).
Traces: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-mismatch-negative

Cause 3 of 3: a stack whose custom resource signals FAILED never settles, so the check never completes.
AWS: a custom resource that signals FAILED moves the stack to ROLLBACK_COMPLETE promptly and DescribeStacks reports that terminal status.
Emulator: the check never completes and the harness kills it with 'Check timed out after 60.0s' — no terminal stack status is ever reached.
Why: a client waiting for a terminal stack status after a failed custom resource waits forever.
Fix: custom-resource signal handling and stack state settlement in the CloudFormation stack engine.
Traces: apigateway__cloudformation__lambda.cfn-custom-resource-signals-failure-stack-rollback-negative

## Evidence
Representative replay output (verbatim):
- 'AssertionError: expected 200, got 404: b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'' (positive deployments) and 'expected 500 for missing permission, got 404: b'{"__type":"UnknownService", ...}'' / 'expected 500 for an unassumable role, got 404: ...' / 'expected 500 for a nonexistent target, got 404: ...' (Cause 1)
- 'AssertionError: positive control: real alias must invoke, got {... 'status': 500, 'body': '{"message":"Internal server error"}', 'log': "... Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call: no deployment package runs in process and no outbound connection is made, so no backend answer exists to map ... Answered through this API's API_CONFIGURATION_ERROR gateway response ..."}' (Cause 2)
- 'Error: Check timed out after 60.0s' (Cause 3)

## Steps to Reproduce
1. Deploy a CloudFormation stack containing an AWS::ApiGateway::RestApi with an AWS_PROXY integration to an AWS::Lambda::Function, a Deployment and a Stage.
2. Issue an HTTPS GET to the stage's execute-api URL — the emulator answers 404 {"__type":"UnknownService",...} instead of routing to the API (Cause 1). Repeat with the AWS::Lambda::Permission removed, with a role whose trust policy omits apigateway.amazonaws.com, and with an integration uri naming a nonexistent function: all three also answer 404 instead of 500.
3. Publish a Lambda alias, point an AWS_PROXY integration at the alias ARN and call TestInvokeMethod (Cause 2).
4. Deploy a stack whose custom resource signals FAILED and poll DescribeStacks for a terminal status (Cause 3).

Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-apigw-lambda-permission-missing-negative apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive apigateway__cloudformation__lambda.cfn-apigw-lambda-request-integration-mapping-positive apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative apigateway__cloudformation__lambda.cfn-apigw-role-trust-policy-positive apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive apigateway__cloudformation__lambda.cfn-apigw-wrong-integration-uri-negative apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test apigateway__dynamodb__lambda__logs.cli-apigateway-missing-deployment-stage-negative apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-dynamodb-direct-integration-positive apigateway__dynamodb__lambda__logs.deploy-cfn-missing-lambda-permission-for-apigateway-negative apigateway__dynamodb__lambda__logs.deploy-missing-lambda-permission-blocks-apigateway-invoke-negative apigateway__dynamodb__lambda__logs.deploy-wrong-integration-uri-negative
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-permission-missing-negative|run#9|AssertionError|assert bad_status == 500, f"expected 500 for missing permission, got {bad_status}: {bad_body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive|run#6|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-lambda-request-integration-mapping-positive|run#6|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative|run#9|AssertionError|assert bad_status == 500, f"expected 500 for an unassumable role, got {bad_status}: {bad_body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-role-trust-policy-positive|run#7|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive|run#8|AssertionError|assert status == 200, f"expected 200 from initial deployment, got {status}: {body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-wrong-integration-uri-negative|run#12|AssertionError|assert bad_status == 500, f"expected 500 for a nonexistent target, got {bad_status}: {bad_body!r}"
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow|_assert_missing_write_policy_denies#24|AssertionError|assert status in {500, 502}, (
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive|run#12|AssertionError|assert status == 200, f"Expected 200, got {status}: {resp_body}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip|run#25|AssertionError|assert status == 200, f"unexpected status {status}: {body_text}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test|run#27|AssertionError|assert status == 200, f"unexpected status {status}: {body_text}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-deployment-stage-negative|run#21|AssertionError|assert status == 403, f"expected 403 before any deployment, got {status}: {body_text}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-dynamodb-direct-integration-positive|_poll_until#6|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-missing-lambda-permission-for-apigateway-negative|_poll_until#7|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-missing-lambda-permission-blocks-apigateway-invoke-negative|run#8|AssertionError|assert status == 500, f"expected 500 (Lambda unreachable, no permission), got {status}: {body}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-wrong-integration-uri-negative|run#11|AssertionError|assert status == 500, f"expected 500 (integration target does not exist), got {status}: {body}"
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# API Gateway execute-api stage endpoints are not routed: every request to a deployed REST API answers 404 UnknownService

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane)

## Description
Cause 1 of 1: the emulator's front door cannot route a data-plane request addressed to an API Gateway REST API stage endpoint, so no deployed API is reachable over HTTP.

AWS: a REST API that has a deployment and a stage is served at https://{rest-api-id}.execute-api.{region}.amazonaws.com/{stage}/{resource-path}. A request to that endpoint is answered by the stage: the integration's response on success, and API Gateway's own status on failure (403 for a missing/denied authorizer, 500 with {"message": "Internal server error"} when the integration target does not exist or the integration lacks lambda:InvokeFunction, 403 {"message":"Forbidden"} before any deployment exists). Smithy (apigateway / backplanecontrolservice): CreateDeployment and GetStage model the control-plane objects; the invoke endpoint itself is not a modeled operation, so the expected statuses above are recorded behavior from the checks plus AWS's documented API Gateway responses. The one thing that is never correct is for the host to be unknown to the service.

Emulator: every such request is answered HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}. The front door only resolves a target service from a SigV4 credential scope, an X-Amz-Target header or a recognised Host header, and an execute-api virtual host carries none of those, so the request never reaches the API Gateway emulation at all. This is the emulator's own refusal: it handed the check an invoke URL it then does not serve.

Why: a client cannot exercise any deployed REST API. Positive paths never return the integration's response, and — worse — every negative path is indistinguishable from every other, because a missing Lambda permission, a wrong integration URI, an undeployed stage and a denied authorizer all collapse to the same 404 UnknownService instead of 403/500. Any test that asserts on API Gateway's status taxonomy silently passes or fails for the wrong reason.

Fix: the request router / front door host resolution, so that a Host of the form {rest-api-id}.execute-api.{region}.amazonaws.com (and the equivalent path-style form the emulator hands out in invoke URLs) dispatches to the API Gateway stage handler; then the stage handler must resolve stage -> deployment -> resource -> method -> integration and apply the authorizer and resource-policy checks before invoking the integration.

Traces: see trace_ids; the 404 UnknownService body is identical in all of them, e.g. apigateway__cloudformation__lambda.cfn-apigw-lambda-proxy-integration-deploy-positive ("expected 200, got 404") and apigateway__dynamodb__lambda__logs.deploy-wrong-integration-uri-negative ("expected 500 (integration target does not exist), got 404").

## Evidence
Expected: HTTP 200 with the integration's response for a deployed proxy integration; HTTP 500 {"message":"Internal server error"} when the integration target is missing or un-invokable; HTTP 403 when no deployment/authorization exists.
Actual: HTTP 404, headers x-amzn-errortype: UnknownService, body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} for every request to the invoke URL.
Sample assertions from the replay: "expected 200, got 404: b'{\"__type\":\"UnknownService\"...}'"; "expected 500 for missing permission, got 404"; "expected 403 before any deployment, got 404"; "direct-integration PutItem call succeeds not met within 45.0s. Last result: (404, '{\"__type\":\"UnknownService\"...}')".

## Steps to Reproduce
1. Create a REST API, a resource and a method with an AWS_PROXY integration to a Lambda function, grant apigateway.amazonaws.com lambda:InvokeFunction on it.
2. CreateDeployment with stageName=prod.
3. Issue GET https://{rest-api-id}.execute-api.{region}.amazonaws.com/prod/{path} (or the invoke URL the emulator reports) against the emulator.
4. Observe HTTP 404 with __type UnknownService instead of the integration's response.

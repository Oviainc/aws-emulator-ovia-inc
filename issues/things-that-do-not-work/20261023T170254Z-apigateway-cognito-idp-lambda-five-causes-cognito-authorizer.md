Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-passthrough-to-lambda apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-reach-integration apigateway__cognito-idp__lambda.apigw-cognito-authorizer-group-based-access-positive apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered apigateway__cognito-idp__lambda.apigw-cognito-authorizer-wrong-user-pool-arn-negative apigateway__cognito-idp__lambda.apigw-cognito-user-pools-authorizer-invalid-token-negative apigateway__cognito-idp__lambda.apigw-cognito-user-pools-authorizer-missing-scope-negative apigateway__cognito-idp__lambda.apigw-cognito-user-pools-authorizer-valid-token-positive apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative apigateway__cognito-idp__lambda.cli-apigw-lambda-cognito-full-stack-smoke apigateway__cognito-idp__lambda.cli-cognito-create-userpool-apigw-authorizer-invoke apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive apigateway__cognito-idp__lambda.cli-cognito-admin-create-user-lambda-invoke apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-passthrough-to-lambda|_poll_invoke_until#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-reach-integration|_poll_http#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-group-based-access-positive|_poll_invoke_until#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered|_poll_http#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-wrong-user-pool-arn-negative|_poll_invoke_until#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-user-pools-authorizer-invalid-token-negative|_poll_invoke_until#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-user-pools-authorizer-missing-scope-negative|_poll_invoke_until#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-user-pools-authorizer-valid-token-positive|_poll_invoke_until#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative|_http_get_status#46|AssertionError|raise AssertionError(f"GET {url} never resolved within {timeout_s}s: {last_exc}")
Issue-Key: apigateway__cognito-idp__lambda.cli-apigw-lambda-cognito-full-stack-smoke|_http_get#38|AssertionError|raise AssertionError(f"GET {url} never resolved within {timeout_s}s: {last_exc}")
Issue-Key: apigateway__cognito-idp__lambda.cli-cognito-create-userpool-apigw-authorizer-invoke|_http_get#43|AssertionError|raise AssertionError(f"GET {url} never resolved within {timeout_s}s: {last_exc}")
Issue-Key: apigateway__cognito-idp__lambda|GetAccount|NotImplemented
Issue-Key: apigateway__cognito-idp__lambda.cli-cognito-admin-create-user-lambda-invoke|_run_shell#28|AssertionError|raise AssertionError(
Issue-Key: apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test|_run_shell#26|AssertionError|raise AssertionError(
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__cognito-idp__lambda: five causes — Cognito-authorizer traffic hangs to the 60 s cap, deployed stage URLs answer 404, GetAccount is a 501 stub, Lambda Invoke is a 501 stub, and cognito-idp is unregistered under its SigV4 name

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Five distinct root causes across 14 failing checks in this service.

Cause 1 of 5: traffic driven at a REST API with a Cognito user-pool authorizer never completes — eight checks are killed at the harness's 60 s budget with no response.
AWS: recorded behavior only. Real AWS answers each of these requests promptly: a valid token yields the integration's 200 with the authorizer claims in the event, a missing/invalid token or a wrong user-pool ARN yields 401/403 without invoking the integration, and a missing scope yields 403.
Emulator: no HTTP status and no error code are produced at all; the check is terminated by the harness ('Error: Check timed out after 60.0s'), so the emulator accepted the request and never responded.
Why: client calls block until their own socket timeout instead of failing fast, and the entire authorizer allow/deny contract — claims passthrough, token validation, scope and user-pool-ARN checks — is unobservable.
Fix: the deployed-stage request path and its authorizer evaluation must terminate and respond. The related open issue 20261023T125454Z-apigateway-traffic-driven-at-a-deployed-rest-api-and.md covers other hanging traffic checks but does not carry these replay keys.
Traces: apigw-cognito-authorizer-claims-passthrough-to-lambda, apigw-cognito-authorizer-claims-reach-integration, apigw-cognito-authorizer-group-based-access-positive, apigw-cognito-authorizer-missing-token-not-delivered, apigw-cognito-authorizer-wrong-user-pool-arn-negative, apigw-cognito-user-pools-authorizer-invalid-token-negative, apigw-cognito-user-pools-authorizer-missing-scope-negative, apigw-cognito-user-pools-authorizer-valid-token-positive.

Cause 2 of 5: a deployed stage's invoke URL answers HTTP 404 instead of executing the stage, for three CLI-driven flows.
AWS: recorded behavior only. A GET against a deployed REST API stage path returns the integration's response (200 for an authorized call) or the authorizer's 401/403 — never 404 — once the deployment exists.
Emulator: the request answers HTTP 404 Not Found and never resolves within the check's 45 s poll.
Why: a client that deploys an API and then calls it cannot reach it; a 404 is indistinguishable from 'this route does not exist', so callers conclude their configuration is wrong.
Fix: the execute-api request router for deployed stages. The related open issue 20261023T114649Z-apigateway-api-gateway-execute-api-stage-endpoints-are-not.md describes the same family but does not carry these replay keys.
Traces: cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative, cli-apigw-lambda-cognito-full-stack-smoke, cli-cognito-create-userpool-apigw-authorizer-invoke.

Cause 3 of 5: apigateway GetAccount answers 501 NotImplemented.
AWS: Smithy GetAccount (GET /account, success 200) returns Account (cloudwatchRoleArn, throttleSettings, features, apiKeyVersion); modeled errors are BadRequestException (400), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401).
Emulator: HTTP 501, error code NotImplemented ('operation GetAccount is scaffolded but not implemented in this emulator').
Why: any flow that reads the account-level CloudWatch role or throttle settings before configuring logging gets an unmodeled 501 rather than the Account document or a modeled 4xx. Existing issue 20261023T125454Z-apigateway-getaccount-stub.md names the stub but not this check.
Fix: the GetAccount handler (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_account.rs).
Traces: cfn-apigateway-role-trusts-apigateway-service-positive.

Cause 4 of 5: lambda Invoke answers 501 NotImplemented for a real invocation.
AWS: Invoke returns HTTP 200 with the function's response payload (or 200 with X-Amz-Function-Error for a handler error).
Emulator: HTTP 501, code NotImplemented — 'Invoke resolved <function arn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator'. Via the CLI this surfaces as rc=254.
Why: no function can actually run, so no Cognito-trigger or admin-create-user flow can be verified end to end. Existing issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the family but not this check.
Fix: the Invoke handler's execution path (lambda service src/ops/invoke.rs).
Traces: cli-cognito-admin-create-user-lambda-invoke.

Cause 5 of 5: cognito-idp requests answer UnknownService because no service is registered for the SigV4 signing name 'awscognitoidentityproviderservice'.
AWS: SignUp is a modeled cognito-idp operation returning HTTP 200 with SignUpResponse, and its failures are modeled Cognito errors (e.g. InvalidParameterException, UsernameExistsException, UserLambdaValidationException).
Emulator: the request is not routed to any emulated service — error code UnknownService, message 'no emulated service is registered for \'awscognitoidentityproviderservice\''. Via the CLI this surfaces as rc=254.
Why: a correctly signed cognito-idp call is rejected at the front door with a code no SDK models, so every sign-up/trigger flow fails before reaching any Cognito logic.
Fix: service registration/routing for the cognito-idp SigV4 signing name in the request dispatcher. The related open issue 20261023T125454Z-cognito-idp-cognito-every-unsigned-anonymous-operation-answers-unknownservice-admininitiateauth.md covers the family but not this check.
Traces: cli-lambda-cognito-trigger-setup-and-test.

## Evidence
Cause 1: Error: Check timed out after 60.0s (report lines 51860, 52170, 52763, 53063, 54055, 54364, 54667, 54982) — no status, no error code.
Cause 2: AssertionError: GET https://<host>/_aws/execute-api/<api-id>/prod/secure never resolved within 45.0s: HTTP Error 404: Not Found (lines 57289, 58271); same for .../prod/whoami (line 57675). Expected 200 from the integration or the authorizer's 401/403.
Cause 3: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAccount operation: operation GetAccount is scaffolded but not implemented in this emulator (line 55273). Expected HTTP 200 with an Account document.
Cause 4: aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:...:function:... at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator; CLI rc=254 (lines 58029-58032). Expected HTTP 200 with the function payload.
Cause 5: aws: [ERROR]: An error occurred (UnknownService) when calling the SignUp operation: no emulated service is registered for 'awscognitoidentityproviderservice'; CLI rc=254 (lines 58637-58640). Expected HTTP 200 SignUpResponse or a modeled Cognito error.

## Steps to Reproduce
Cause 1: create a REST API with a COGNITO_USER_POOLS authorizer, deploy it, and send a request with a valid pool token; observe the call never returns within 60 s.
Cause 2: deploy the same API and GET the stage path via the invoke URL; expect 200/401/403, observe HTTP 404 on every poll for 45 s.
Cause 3: aws apigateway get-account; expect 200 with cloudwatchRoleArn/throttleSettings, observe 501 NotImplemented.
Cause 4: aws lambda invoke --function-name <fn> out.json; expect 200 with the payload, observe rc=254 / 501 NotImplemented.
Cause 5: aws cognito-idp sign-up --client-id <id> --username u --password P; expect 200 or a modeled Cognito error, observe UnknownService.
Re-replay the 14 check ids in trace_ids.

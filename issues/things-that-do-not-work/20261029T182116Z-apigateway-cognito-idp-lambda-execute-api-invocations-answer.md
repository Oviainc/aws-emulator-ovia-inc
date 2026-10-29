Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered apigateway__cognito-idp__lambda.apigw-cognito-authorizer-tampered-signature-negative apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive apigateway__cognito-idp__lambda.cfn-apigw-lambda-proxy-integration-resource-binding-positive apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative apigateway__cognito-idp__lambda.cli-apigw-lambda-cognito-full-stack-smoke apigateway__cognito-idp__lambda.cli-cognito-create-userpool-apigw-authorizer-invoke apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test apigateway__cognito-idp__lambda.cognito-custom-message-trigger-modifies-message apigateway__cognito-idp__lambda.cognito-pre-signup-trigger-blocks-signup-on-error apigateway__cognito-idp__lambda.cognito-trigger-missing-invoke-permission-negative apigateway__cognito-idp__lambda.cfn-cognito-userpool-lambda-trigger-config-positive
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered|run#19|AssertionError|assert bad_status == 401, (
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-tampered-signature-negative|_poll_invoke_until#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive|_poll_invoke_until#6|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.cfn-apigw-lambda-proxy-integration-resource-binding-positive|_poll_invoke_until#6|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative|_http_get_status#50|Timeout|time.sleep(2)
Issue-Key: apigateway__cognito-idp__lambda.cli-cognito-create-userpool-apigw-authorizer-invoke|_http_get#46|AssertionError|raise AssertionError(f"GET {url} never resolved within {timeout_s}s: {last_exc}")
Issue-Key: apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test|run#27|AssertionError|assert signup_out["UserConfirmed"] is True, (
Issue-Key: apigateway__cognito-idp__lambda.cognito-custom-message-trigger-modifies-message|_client_error#19|AssertionError|raise AssertionError(
Issue-Key: apigateway__cognito-idp__lambda.cognito-pre-signup-trigger-blocks-signup-on-error|_client_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: apigateway__cognito-idp__lambda.cognito-trigger-missing-invoke-permission-negative|_client_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cognito-identity-provider|SignUp|InvalidParameterException
Issue-Related: 20261023T170254Z-apigateway-cognito-idp-lambda-five-causes-cognito-authorizer.md
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# apigateway__cognito-idp__lambda: execute-api invocations answer 500 (or never answer), Cognito Lambda triggers are not invoked, and a CFN-created app client's Ref is not a ClientId (3 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Three root causes behind the surviving failures of this service combination. The open issues 20261023T170254Z-apigateway-cognito-idp-lambda-five-causes-cognito-authorizer.md and 20261023T114649Z-cognito-idp-cognito-user-pool-lambda-triggers-are-not-invoked.md look similar but carry different replay keys; the checks below are not in them.

Cause 1 of 3 — a request to a deployed REST API's stage returns HTTP 500, or never returns at all.
AWS: a GET to https://<api-id>.execute-api.<region>.amazonaws.com/<stage>/<resource> behind a COGNITO_USER_POOLS authorizer answers 200 with the integration's body for a valid ID token, and 401 with {"message":"Unauthorized"} for a missing, tampered or otherwise unverifiable bearer token. It never answers 500 for either case.
Emulator: every such call answers 500 with the body {"message":"Internal server error"}; the checks that poll for a non-500 answer exhaust their budget ('never resolved within 45.0s: HTTP Error 500' / 'Check timed out after 60.0s').
Why: the whole data plane behind API Gateway is unusable, and a client cannot distinguish an authorization refusal from a server fault — a negative test that should see 401 sees 500.
Fix: the execute-api request path (authorizer evaluation + integration dispatch) in the apigateway data-plane handler.
Traces: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered, apigateway__cognito-idp__lambda.apigw-cognito-authorizer-tampered-signature-negative, apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive, apigateway__cognito-idp__lambda.cfn-apigw-lambda-proxy-integration-resource-binding-positive, apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative, apigateway__cognito-idp__lambda.cli-apigw-lambda-cognito-full-stack-smoke, apigateway__cognito-idp__lambda.cli-cognito-create-userpool-apigw-authorizer-invoke.

Cause 2 of 3 — the user pool's LambdaConfig triggers are never invoked.
AWS: a pool with LambdaConfig.PreSignUp returning autoConfirmUser=true makes SignUp answer UserConfirmed=true; a PreSignUp trigger that raises makes SignUp fail with UserLambdaValidationException; a CustomMessage trigger rewrites the delivered message; and a trigger whose function grants Cognito no lambda:InvokeFunction permission makes the triggering call fail with InvalidLambdaResponseException/UnexpectedLambdaException rather than succeed.
Emulator: SignUp returns UserConfirmed=False, the raising trigger's SignUp SUCCEEDS (200 with UserSub), the custom-message call SUCCEEDS unchanged, and the missing-permission call SUCCEEDS with 200.
Why: every trigger-dependent behavior silently no-ops, and negative permission wiring is not detectable.
Fix: trigger invocation in the cognito-idp user-pool operations (SignUp/ConfirmSignUp/AdminCreateUser message path) plus the invoke-permission check.
Traces: apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test, apigateway__cognito-idp__lambda.cognito-custom-message-trigger-modifies-message, apigateway__cognito-idp__lambda.cognito-pre-signup-trigger-blocks-signup-on-error, apigateway__cognito-idp__lambda.cognito-trigger-missing-invoke-permission-negative.

Cause 3 of 3 — Ref of a CloudFormation-created AWS::Cognito::UserPoolClient is not a ClientId.
AWS: Ref on that resource returns the generated ClientId, which matches ClientIdType pattern [\w+]+ and is accepted by SignUp.
Emulator: the value handed to SignUp is the stack-qualified logical name ('sim2real-xsvc-stk-<id>-UserPoolClient-<suffix>'), which SignUp then refuses with InvalidParameterException '1 validation error detected: Value ... at \'clientId\' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w+]+$'.
Why: no CFN-provisioned app client can be used by the client SDK.
Fix: the AWS::Cognito::UserPoolClient resource's physical-id/Ref resolution in the CloudFormation resource provider.
Traces: apigateway__cognito-idp__lambda.cfn-cognito-userpool-lambda-trigger-config-positive.

## Evidence
Verbatim from the replay:
- 'expected 401 for an unverifiable bearer token, got 500: {"message":"Internal server error"} (2xx => the emulator admitted an arbitrary string as a valid pool token)'
- 'GET https://127.0.0.1:40535/_aws/execute-api/8sh3tqaaaa/prod/whoami never resolved within 45.0s: HTTP Error 500: Internal Server Error'
- 'Error: Check timed out after 60.0s' (apigw-cognito-authorizer-tampered-signature-negative, cfn-apigateway-role-trusts-apigateway-service-positive, cfn-apigw-lambda-proxy-integration-resource-binding-positive, cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative)
- 'PreSignUp trigger should have auto-confirmed the user via SignUp\'s UserConfirmed flag; got {UserConfirmed: False, UserSub: ...}'
- 'expected a ClientError, but the call succeeded: {UserConfirmed: False, UserSub: ...}' (pre-signup trigger raising)
- 'expected the call to be refused, but it succeeded: {HTTPStatusCode: 200}' (trigger with no invoke permission)
- InvalidParameterException on SignUp: "Value 'sim2real-xsvc-stk-f5cec73b11a9-UserPoolClient-49CBDF9BFBC2' at 'clientId' failed to satisfy constraint"

## Steps to Reproduce
1. CreateRestApi + a Lambda proxy integration + a COGNITO_USER_POOLS authorizer, CreateDeployment, then GET the stage URL with a valid ID token (expect 200) and with a tampered token (expect 401). Observe 500 for both.
2. CreateUserPool with LambdaConfig.PreSignUp pointing at a function returning autoConfirmUser=true, then SignUp -> expect UserConfirmed=true.
3. Deploy a CFN stack declaring AWS::Cognito::UserPool + AWS::Cognito::UserPoolClient, read the Ref output and pass it to SignUp as ClientId.

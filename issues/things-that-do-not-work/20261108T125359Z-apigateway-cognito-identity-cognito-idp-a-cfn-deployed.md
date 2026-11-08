Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive
Issue-Key: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive|_poll_until#7|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 5f5c8c74019b1fc5b04177af48d49ea878911ccb

# apigateway__cognito-identity__cognito-idp: a CFN-deployed REST stage behind a TOKEN authorizer whose authorizerCredentials role is correctly trusted answers 500 forever instead of the Allow/Deny decision (1 cause)

> _Note: this issue was identified against the code base at commit `5f5c8c74019b1fc5b04177af48d49ea878911ccb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
One net-new behavioral root cause, grounded in 1 failing check of the simparity-behavioral-aws-v1 replay (apigateway__cognito-identity__cognito-idp 12/21; the other eight failures in this service are marked OWNED by open issues and are NOT part of this issue). Related open issues that do NOT carry this replay key, named for the fixer: 20261107T153156Z-apigateway-lambda-logs-a-rest-method-behind-a.md (a REST method behind a TOKEN Lambda authorizer never answers 200/401/403 — same gate, different service token, and there the stage produces NO answer at all) and 20261107T184737Z-apigateway-cloudformation-lambda-the-execute-api-data-plane-refuses-every-call.md / 20261107T070300Z-apigateway-dynamodb-lambda-logs-cloudformation-deployed-rest-api-stages-are-unservable-the.md (CFN-deployed stage URLs answered 404 UnknownService). This check is a DIFFERENT symptom: the stage DOES answer, and it answers 500 persistently, so a fixer must re-replay this check even after those are closed.

Cause 1 of 1: an API Gateway TOKEN authorizer whose authorizerCredentials role is trusted by apigateway.amazonaws.com and grants lambda:InvokeFunction on the authorizer function is treated as unusable — the deployed stage answers every request 500 instead of running the authorizer and honouring its policy document.
AWS: Smithy com.amazonaws.apigateway#CreateAuthorizer, member authorizerCredentials — "Specifies the required credentials as an IAM role for API Gateway to invoke the authorizer. To specify an IAM role for API Gateway to assume, use the role's Amazon Resource Name (ARN). To use resource-based permissions on the Lambda function, specify null." With type TOKEN, identitySource method.request.header.Authorization, authorizerResultTtlInSeconds 0, a method whose authorizationType is CUSTOM naming that authorizerId, a MOCK integration returning {"message":"ok"}, and a stage deployment — all created by a CloudFormation stack that reaches CREATE_COMPLETE — a GET to the stage invoke URL with header Authorization: allow-me invokes the authorizer under the credentials role (no Lambda resource policy needed, because the role itself carries the lambda:InvokeFunction grant), gets a policyDocument with Effect Allow for the method ARN, and reaches the integration: HTTP 200 with the integration's response body. The same GET with Authorization: deny-me yields HTTP 403 (explicit deny). AWS answers 500 with the generic body {"message":"Internal server error"} ONLY when the authorizer cannot be invoked (an untrusted or unauthorised authorizerCredentials role) — that is exactly the negative case the companion checks in this suite exercise, and it is the case that must NOT fire here. Recorded behavior for the message: AWS's 500 body for an authorizer configuration failure is the fixed text {"message":"Internal server error"} with no variable parts; the 200 body here is the template's own fixed text {"message":"ok"}.
Emulator: every probe of the deployed stage URL is answered HTTP 500 with body {"message":"Internal server error"}, for the whole 60 s poll window (probe every 3 s), so the status never becomes 200 or 403. The positive leg and the deny leg are both unobservable: the check fails at its first poll with 'authorizer-backed request to stop 500-ing (role trust becomes usable) not met within 60.0s. Last result: (500, b'{"message":"Internal server error"}')'. The CFN stack itself reached CREATE_COMPLETE and published its ApiId output, so the divergence is in serving the authorized request, not in provisioning.
Why: a client cannot use a Lambda TOKEN authorizer wired with authorizerCredentials at all — the documented alternative to a Lambda resource policy is dead. Worse, the emulator makes the correctly-permissioned configuration indistinguishable from the broken one: this suite's negative checks assert 500 for a role WITHOUT the invoke grant, so a caller (or a security test) that compares the two configurations sees the same 500 for both and cannot tell a working authorizer from a misconfigured one. No Allow decision ever reaches an integration and no explicit Deny ever produces its 403.
Fix: the execute-api REST stage request path in the apigateway crate — the authorizer invocation step must assume/accept an authorizerCredentials role that trusts apigateway.amazonaws.com and carries lambda:InvokeFunction on the authorizer function, invoke the authorizer, and evaluate the returned policyDocument into either the integration call (200) or a 403, instead of classifying the role as unusable and returning the configuration-error 500.
Traces: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[mismatch] apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive
  File "check_apigateway__cognito_identity__cognito_idp_deploy_apigw_execution_role_trust_policy_template_positive.py", line 152, in run
    status, body = _poll_until(...)
  File "...", line 178, in _poll_until
    raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
  AssertionError: authorizer-backed request to stop 500-ing (role trust becomes usable) not met within 60.0s. Last result: (500, b'{"message":"Internal server error"}')

What the check does before that point (all of it succeeded): create_stack with a template declaring LambdaExecutionRole (trusts lambda.amazonaws.com, AWSLambdaBasicExecutionRole), AuthorizerFunction (python3.12 handler returning Allow only for the token 'allow-me'), ApiGatewayInvokeRole (AssumeRolePolicyDocument trusting apigateway.amazonaws.com, inline policy allowing lambda:InvokeFunction on !GetAtt AuthorizerFunction.Arn), RestApi, a /secure resource, TokenAuthorizer (Type TOKEN, AuthorizerUri the Lambda invocations ARN, AuthorizerCredentials !GetAtt ApiGatewayInvokeRole.Arn, IdentitySource method.request.header.Authorization, AuthorizerResultTtlInSeconds 0), SecureMethod (GET, AuthorizationType CUSTOM, MOCK integration with IntegrationResponses 200 -> {"message":"ok"}), ApiDeployment and ApiStage 'test'; the stack reached CREATE_COMPLETE and the ApiId output resolved.

Expected (AWS):
  GET /test/secure, Authorization: allow-me   -> 200, body {"message":"ok"} (MOCK integration reached)
  GET /test/secure, Authorization: deny-me    -> 403 (authorizer returned Effect Deny)
Actual (emulator):
  GET /test/secure, Authorization: allow-me   -> 500, body {"message":"Internal server error"}, on every probe for 60 s
  (the deny leg and the body assertions were never reached)

The 500 is AWS's authorizer-configuration-error answer, which this wiring must not produce: the credentials role is trusted by apigateway.amazonaws.com and holds lambda:InvokeFunction on the authorizer. Only the status and the fixed body text are at issue; the api id, role names, function name and stage host are per-run values.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. cloudformation create-stack with CAPABILITY_IAM/CAPABILITY_NAMED_IAM and a template containing: an IAM role trusting lambda.amazonaws.com for the authorizer function; a python3.12 Lambda whose handler returns a policyDocument with Effect Allow when event['authorizationToken'] == 'allow-me' and Deny otherwise; an IAM role whose AssumeRolePolicyDocument trusts apigateway.amazonaws.com with an inline policy allowing lambda:InvokeFunction on that function's ARN; an AWS::ApiGateway::RestApi with a /secure resource; an AWS::ApiGateway::Authorizer of Type TOKEN with AuthorizerUri arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations, AuthorizerCredentials set to the apigateway-trusting role ARN, IdentitySource method.request.header.Authorization and AuthorizerResultTtlInSeconds 0; an AWS::ApiGateway::Method GET with AuthorizationType CUSTOM, that AuthorizerId and a MOCK integration returning {"message":"ok"}; a Deployment and a Stage named 'test'. Wait for CREATE_COMPLETE.
3. curl -H 'Authorization: allow-me' https://<restApiId>.execute-api.<region>.amazonaws.com/test/secure
   Observed: 500 {"message":"Internal server error"}, repeatedly for at least 60 s. AWS: 200 {"message":"ok"}.
4. curl -H 'Authorization: deny-me' <same url>
   AWS: 403. (Unreachable today — step 3 never clears.)
5. Re-replay the check id in trace_ids.

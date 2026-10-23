Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cognito-pre-signup-trigger-blocks-signup-on-error apigateway__cognito-idp__lambda.cognito-pre-token-v2-trigger-group-override apigateway__cognito-idp__lambda.cognito-trigger-wrong-lambda-arn-negative
Issue-Key: apigateway__cognito-idp__lambda.cognito-pre-signup-trigger-blocks-signup-on-error|run#11|AssertionError|assert code == "UserLambdaValidationException", (
Issue-Key: apigateway__cognito-idp__lambda.cognito-pre-token-v2-trigger-group-override|run#15|AssertionError|assert claims.get("cognito:groups") == ["sim2real-override-group"], (
Issue-Key: apigateway__cognito-idp__lambda.cognito-trigger-wrong-lambda-arn-negative|_client_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Cognito user pool Lambda triggers are not invoked and trigger misconfiguration is not rejected

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignUp

## Description
Three distinct root causes in the cognito-idp emulation.

Cause 1 of 3 - a PreSignUp trigger that fails does not abort SignUp with UserLambdaValidationException.
AWS: when a user pool has a PreSignUp Lambda trigger and that function raises, SignUp is refused with UserLambdaValidationException (400) and a message naming the trigger. Smithy (cognito-idp): SignUp models UserLambdaValidationException; the trigger contract is recorded behavior.
Emulator: the error the client sees is 'UnknownService' - the trigger invocation path does not reach a Lambda emulation at all, and the resulting failure is surfaced as an unknown-service error rather than the modeled Cognito exception. Replay: "expected SignUp to be aborted with UserLambdaValidationException, got 'UnknownService'".
Why: a client cannot distinguish a trigger-rejected sign-up from a broken endpoint, and UnknownService is not a Cognito error code at all, so error handling keyed on UserLambdaValidationException never fires.
Fix: the user-pool Lambda-trigger invocation path and its error mapping in the SignUp handler.
Checks: apigateway__cognito-idp__lambda.cognito-pre-signup-trigger-blocks-signup-on-error.

Cause 2 of 3 - a PreTokenGeneration V2_0 trigger's groupOverrideDetails is not applied to the issued token.
AWS: when the pool's PreTokenGenerationConfig uses LambdaVersion V2_0 and the trigger returns claimsAndScopeOverrideDetails.groupOverrideDetails.groupsToOverride, the issued ID/access token carries the overridden cognito:groups claim.
Emulator: the cognito:groups claim is absent - expected ['sim2real-override-group'], got None.
Why: group-based authorization built on the trigger silently grants nothing (or everything), and API Gateway/Cognito authorizers that read cognito:groups make the wrong decision.
Fix: apply the V2_0 trigger response's override details when minting tokens in the authentication path.
Checks: apigateway__cognito-idp__lambda.cognito-pre-token-v2-trigger-group-override.

Cause 3 of 3 - a trigger pointing at a Lambda ARN that cannot be invoked is not rejected.
AWS: an authentication flow whose configured trigger ARN does not exist (or cannot be invoked) fails the call rather than completing it; the pool's trigger configuration is validated against an invokable function.
Emulator: the call succeeds and returns a full AuthenticationResult (AccessToken/IdToken/RefreshToken), i.e. the trigger is simply skipped. Replay: "expected the call to be refused, but it succeeded: {'AuthenticationResult': {...}}".
Why: a misconfigured trigger is invisible - every flow that depends on the trigger's effect (validation, claim enrichment, migration) proceeds as if the trigger ran and approved it, which is an authorization bypass in anything built on it. (The token values themselves are environment-specific and are not the requirement here; the requirement is that the call is refused.)
Fix: trigger resolution/validation in the user-pool trigger dispatch path - a configured trigger that cannot be invoked must fail the operation, not be skipped.
Checks: apigateway__cognito-idp__lambda.cognito-trigger-wrong-lambda-arn-negative.

Note: the related failures where a CloudFormation-created AWS::Cognito::UserPool is addressed by a stack-derived physical id and rejected by AdminCreateUser are filed separately against cloudformation (physical resource ids are logical ids).

## Evidence
Cause 1: expected HTTP 400 UserLambdaValidationException from SignUp; actual error code 'UnknownService'.
Cause 2: expected the issued token's cognito:groups claim == ['sim2real-override-group'] from the V2_0 groupOverrideDetails; actual None.
Cause 3: expected a ClientError refusing the call because the configured trigger ARN is not invokable; actual HTTP 200 with a complete AuthenticationResult (AccessToken, IdToken, RefreshToken, ExpiresIn 3600, TokenType Bearer).

## Steps to Reproduce
Cause 1: CreateUserPool with LambdaConfig.PreSignUp set to a function that raises; SignUp a new user -> error code UnknownService instead of UserLambdaValidationException.
Cause 2: CreateUserPool with LambdaConfig.PreTokenGenerationConfig {LambdaVersion: V2_0, LambdaArn: fn} where fn returns claimsAndScopeOverrideDetails.groupOverrideDetails.groupsToOverride=['g']; authenticate and decode the token -> no cognito:groups claim.
Cause 3: CreateUserPool with a LambdaConfig trigger ARN naming a function that does not exist; run the authentication flow -> it succeeds.

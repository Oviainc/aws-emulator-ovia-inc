Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.pre-authentication-trigger-denial-rejected-and-the-allowed-user-still-signs-in cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails
Issue-Key: cognito-identity-provider|InitiateAuth|UnexpectedLambdaException
Issue-Key: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication|_retry_on_lambda_warmup#16|Timeout|time.sleep(3.0)
Issue-Key: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails|run#21|AssertionError|assert error_payload["Error"]["Message"] == (
Issue-Commit: 58db3281dc5b97463dad6d5a03f3fef55333dd92

# REOPENS cognito-idp__lambda PreAuthentication/PreTokenGeneration triggers: every user-pool trigger invocation is now refused, and the trigger-timeout refusal message is emulator-authored

> _Note: this issue was identified against the code base at commit `58db3281dc5b97463dad6d5a03f3fef55333dd92`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
REOPENS 20261029T182116Z-cognito-idp-lambda-preauthentication-denial-is-ignored-pretokengeneration.md: its fix left the call refused. That issue's fix added the PreAuthentication/PreTokenGeneration invocation and the five-second trigger budget, but InitiateAuth now answers `UnexpectedLambdaException` for every pool whose LambdaConfig names a trigger, so the three checks it was grounded in still fail — two of them on a refusal of a call that succeeds on AWS. The still-open 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describes the Lambda-side runtime gap; it does not carry these cognito-idp__lambda replay keys and the suite did not mark these checks as owned, so they are filed here.

Cause 1 of 2 — a sign-in through a pool with a synchronous user-pool trigger is refused instead of running the trigger and honouring its answer.
AWS: when a pool's LambdaConfig names a PreAuthentication function, InitiateAuth invokes it after the password is proved; a function that RAISES makes that sign-in fail with `UserLambdaValidationException` and issue no token, and a function that RETURNS lets the sign-in complete with a full AuthenticationResult (recorded behavior only for the pairing — the model states the error, `UserLambdaValidationException`, but not the hook ordering). Likewise a PreTokenGenerationConfig V2_0 function is invoked on BOTH a USER_PASSWORD_AUTH sign-in and a subsequent REFRESH_TOKEN_AUTH exchange, each with its own `triggerSource` (`TokenGeneration_Authentication` / `TokenGeneration_RefreshTokens`), and the claims it returns in `claimsAndScopeOverrideDetails.idTokenGeneration` appear in the issued ID token.
Emulator: the trigger invocation is refused rather than executed, and the refusal is propagated to the caller as `UnexpectedLambdaException` with an emulator-authored message that names the emulator itself — "PreAuthentication failed with error Invoke resolved <function-arn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function." The user the trigger ALLOWS is refused exactly like the denied one, so the positive control never signs in; the V2_0 triggerSource check never gets a token at all and the harness's bounded retry spends its whole budget on the refusal ("Check timed out after 60.0s").
Why: a client gets a 400-class Cognito failure for a sign-in AWS completes — any pool configured with a Lambda trigger becomes unusable rather than trigger-governed — and the one text it does get back advertises emulator internals ($LATEST/python3.12/DryRun) that no AWS error message contains.
Fix: the cognito-idp trigger-invocation path (`src/state/lambda_triggers.rs` in the cognito-idp service) plus the Lambda Invoke handler it calls: a RequestResponse invocation of a real deployment package must execute and return the function's payload so the trigger's own answer — deny, or claims override — decides the sign-in. Until it does, no trigger-governed sign-in can succeed.
Traces: cognito-idp__lambda.pre-authentication-trigger-denial-rejected-and-the-allowed-user-still-signs-in, cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication.

Cause 2 of 2 — when the trigger budget does elapse, the refusal carries the emulator's own message instead of AWS's socket-timeout wording.
AWS: a synchronous PreTokenGeneration function that does not answer inside Cognito's own trigger budget makes the sign-in fail with HTTP 400 `UnexpectedLambdaException` and the message `PreTokenGeneration invocation failed due to error Socket timeout while invoking Lambda function.` The fixed text is `<TriggerName> invocation failed due to error Socket timeout while invoking Lambda function.`; the only variable part is the trigger name of the stage that timed out (here `PreTokenGeneration`). No request value, function ARN, version or runtime name appears in it. The budget is Cognito's own and independent of the function's configured Timeout (recorded behavior only: measured as a failure well inside a 60s function Timeout).
Emulator: the error code is right (`UnexpectedLambdaException`) but the message is the invocation-not-implemented text above, which embeds the resolved function ARN, `$LATEST`, the runtime identifier and advice about `InvocationType=DryRun`.
Why: a client matching on the documented socket-timeout text cannot tell a slow trigger from any other trigger failure, and a per-run function ARN is leaked into what AWS keeps a fixed sentence.
Fix: the trigger-timeout branch of the cognito-idp trigger-invocation path must format the refusal message as AWS words it for that error class, and must not reuse the Lambda-side not-implemented text as a Cognito error message.
Traces: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails.

## Evidence
Verbatim from the replay.

cognito-idp__lambda.pre-authentication-trigger-denial-rejected-and-the-allowed-user-still-signs-in [error]:
  botocore.errorfactory.UnexpectedLambdaException: An error occurred (UnexpectedLambdaException) when calling the InitiateAuth operation: PreAuthentication failed with error Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-66799f7d7638 at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.
  (expected: UserLambdaValidationException and no tokens for the DENIED user, and a successful AuthenticationResult for the second, ALLOWED user in the same pool/client with the same password.)

cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails [mismatch]:
  AssertionError: the PreTokenGeneration timeout surfaced with the wrong message: {'Message': 'PreTokenGeneration failed with error Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-9fbbe95f5a39 at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: ... InvocationType=DryRun validates an invocation without running the function.', 'Code': 'UnexpectedLambdaException'}
  expected Error.Message == 'PreTokenGeneration invocation failed due to error Socket timeout while invoking Lambda function.' (Error.Code UnexpectedLambdaException is already correct)

cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication [mismatch]:
  Error: Check timed out after 60.0s — the pool retains LambdaConfig.PreTokenGenerationConfig.LambdaVersion=V2_0, but neither the USER_PASSWORD_AUTH sign-in nor the REFRESH_TOKEN_AUTH exchange ever yields an ID token, so the two triggerSource values (TokenGeneration_Authentication vs TokenGeneration_RefreshTokens) can never be compared.

The prior fix's own Resolution section states the remaining blocker: "Lambda answers NotImplemented for every RequestResponse invocation, which these gating triggers report as the trigger failure it is" — i.e. the fix converted the three failures into refusals rather than clearing them.

## Steps to Reproduce
1. CreateFunction with a python3.12 zip whose handler raises for one username and returns for every other one; AddPermission for Principal=cognito-idp.amazonaws.com with SourceArn=<pool arn>.
2. CreateUserPool; UpdateUserPool(LambdaConfig={'PreAuthentication': <fn arn>}); CreateUserPoolClient with ExplicitAuthFlows=[ALLOW_USER_PASSWORD_AUTH, ALLOW_REFRESH_TOKEN_AUTH]; AdminCreateUser + AdminSetUserPassword(Permanent=true) for TWO users, one matching the deny marker.
3. InitiateAuth(USER_PASSWORD_AUTH) as the denied user -> expect UserLambdaValidationException, no AuthenticationResult. InitiateAuth as the allowed user -> expect a full AuthenticationResult whose AccessToken works against GetUser.
   Observe: BOTH are refused with UnexpectedLambdaException and the emulator's "running its python3.12 deployment package is not implemented in this emulator" message.
4. On a second pool set LambdaConfig={'PreTokenGenerationConfig': {'LambdaVersion': 'V2_0', 'LambdaArn': <fn arn>}} with a handler that echoes event['triggerSource'] into claimsAndScopeOverrideDetails.idTokenGeneration.claimsToAddOrOverride; sign in with a password, then exchange the refresh token -> the ID tokens must carry TokenGeneration_Authentication and TokenGeneration_RefreshTokens respectively.
   Observe: no ID token is ever issued; the call is refused.
5. On a third pool point PreTokenGeneration at a handler that sleeps 8s, with the FUNCTION's own Timeout set to 60s, and sign in -> expect HTTP 400 UnexpectedLambdaException with Error.Message exactly 'PreTokenGeneration invocation failed due to error Socket timeout while invoking Lambda function.', well before the function's own 60s Timeout.
   Observe: the right code with the emulator's not-implemented message, which also leaks the resolved function ARN, $LATEST and the runtime name.

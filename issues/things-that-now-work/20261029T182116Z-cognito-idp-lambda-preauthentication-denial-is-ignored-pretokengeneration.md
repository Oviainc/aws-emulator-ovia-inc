Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.pre-authentication-trigger-denial-rejected-and-the-allowed-user-still-signs-in cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails
Issue-Key: cognito-idp__lambda.pre-authentication-trigger-denial-rejected-and-the-allowed-user-still-signs-in|_rejection_response#19|AssertionError|raise AssertionError(
Issue-Key: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication|run#20|AssertionError|assert signin_source == "TokenGeneration_Authentication", (
Issue-Key: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails|_retry_on_cold_start_rejected#17|Timeout|time.sleep(3.0)
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp__lambda: PreAuthentication denial is ignored, PreTokenGeneration carries no triggerSource and its 5-second timeout is not enforced (3 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Three root causes left over after the owned trigger issues. 20261024T114943Z-cognito-idp-lambda-the-pool-s-jwks-document.md and 20261027T020619Z-cognito-idp-lambda-the-usermigration-trigger-is-never.md are open on other replay keys.

Cause 1 of 3 — a PreAuthentication trigger that denies the sign-in is ignored.
AWS: when the PreAuthentication Lambda raises, authentication fails with UserLambdaValidationException and no tokens are issued; a second user the same trigger allows still signs in.
Emulator: the denied sign-in SUCCEEDS and returns a full AuthenticationResult (AccessToken/RefreshToken/IdToken).
Why: a deny hook that is supposed to stop authentication does nothing — the emulator issues credentials AWS would refuse.
Fix: invoke and honor the PreAuthentication trigger in the (Admin)InitiateAuth path.
Traces: cognito-idp__lambda.pre-authentication-trigger-denial-rejected-and-the-allowed-user-still-signs-in.

Cause 2 of 3 — PreTokenGeneration V2 is invoked without (or not at all with) its triggerSource.
AWS: a password sign-in invokes PreTokenGeneration with triggerSource 'TokenGeneration_Authentication', and a refresh invokes it with 'TokenGeneration_RefreshTokens', which is how the function distinguishes the two.
Emulator: the recorded triggerSource is None — the invocation the check reads back never carries it.
Fix: populate triggerSource when invoking the PreTokenGeneration (V1_0/V2_0) trigger.
Traces: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication.

Cause 3 of 3 — the 5-second trigger timeout is not enforced.
AWS: a PreTokenGeneration function that sleeps past Cognito's 5-second trigger budget makes the sign-in FAIL (UnexpectedLambdaException/InvalidLambdaResponseException) promptly.
Emulator: the sign-in neither completes nor fails — the check ends with 'Check timed out after 60.0s'.
Why: a slow trigger hangs the caller instead of producing the modeled error.
Fix: apply the 5-second trigger invocation deadline in the token-generation path.
Traces: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails.

## Evidence
Verbatim from the replay:
- 'the call was expected to be rejected but SUCCEEDED, which is the defect: {AuthenticationResult: {AccessToken: eyJ..., RefreshToken: eyJ...}}' (PreAuthentication deny)
- 'a password sign-in\'s PreTokenGeneration invocation did not carry the documented triggerSource TokenGeneration_Authentication: None'
- 'Error: Check timed out after 60.0s' (PreTokenGeneration sleeping past five seconds)

## Steps to Reproduce
1. CreateUserPool with LambdaConfig.PreAuthentication pointing at a function that raises for one username and returns for another; InitiateAuth as the denied user -> expect UserLambdaValidationException and no tokens.
2. CreateUserPool with LambdaConfig.PreTokenGenerationConfig (V2_0); sign in with a password and then with REFRESH_TOKEN_AUTH; inspect the event the function received -> triggerSource must be TokenGeneration_Authentication / TokenGeneration_RefreshTokens.
3. Point PreTokenGeneration at a function that sleeps 10s and sign in -> the call must fail quickly, not hang.

## Resolution
All three causes are fixed in the cognito-idp sign-in path, in the one place a pool's
`LambdaConfig` member becomes an invocation
(`services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/state/lambda_triggers.rs`):

1. A pool whose `LambdaConfig` names a `PreAuthentication` function now has that function
   invoked by `InitiateAuth` / `AdminInitiateAuth` once the request has proved it holds the
   user's password, and the sign-in continues only on a function that RETURNED: a function
   that reports an error is answered with `UserLambdaValidationException`, and one Lambda
   could not run or that gave no answer with `UnexpectedLambdaException` — no token is
   minted either way.
2. The pre-token-generation trigger is invoked by every operation that mints a sign-in's
   tokens (`InitiateAuth` password and refresh flows, `AdminInitiateAuth`,
   `RespondToAuthChallenge`, `AdminRespondToAuthChallenge`, `GetTokensFromRefreshToken`),
   with the `triggerSource` of the stage that reached it
   (`TokenGeneration_Authentication`, `TokenGeneration_RefreshTokens`,
   `TokenGeneration_NewPasswordChallenge`), the event `version` the configured trigger
   version asks for (the legacy `PreTokenGeneration` member and `V1_0` get `1`, `V2_0` `2`,
   `V3_0` `3`), the user's group configuration, and — from the second trigger version on —
   the scopes the access token carries.
3. Every trigger invocation is bounded by Amazon Cognito's five-second trigger budget: a
   function that has not answered when it elapses is given up on and the operation is
   refused rather than held.

The observables these three checks read are still not reachable, and NOT because Cognito
withholds the invocation: each of them is read from what the trigger function itself did
(an SQS message its python3.12 handler sends, or the deny it raises), so they need Lambda
to EXECUTE a deployment package. That is tracked by
`20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` —
until it is fixed, Lambda answers `NotImplemented` for every `RequestResponse` invocation,
which these gating triggers report as the trigger failure it is rather than inventing the
verdict the function never gave.


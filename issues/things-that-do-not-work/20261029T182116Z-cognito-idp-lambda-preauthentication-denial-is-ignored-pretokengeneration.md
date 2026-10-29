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

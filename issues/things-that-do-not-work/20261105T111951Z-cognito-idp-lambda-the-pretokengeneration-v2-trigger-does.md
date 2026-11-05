Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication
Issue-Key: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication|run#20|AssertionError|assert signin_source == "TokenGeneration_Authentication", (
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cognito-idp__lambda: the PreTokenGeneration V2 trigger does not distinguish a refresh from an authentication triggerSource

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Cause 1 of 1 — the triggerSource handed to a PreTokenGeneration V2 trigger does not differentiate a token refresh from an initial authentication.
AWS: the PreTokenGeneration_V2_0 trigger is invoked with triggerSource 'TokenGeneration_Authentication' (or 'TokenGeneration_HostedAuth'/'TokenGeneration_NewDeviceUsed') on an initial sign-in and with 'TokenGeneration_RefreshTokens' when the tokens are minted from a REFRESH_TOKEN_AUTH flow; a handler keys its claim overrides on that value.
Emulator: recorded behavior only — the check fails in its body where it compares the triggerSource observed for the refresh flow with the one observed for authentication, so the two are not distinguished. The open issue 20261031T170200Z-cognito-idp-lambda-reopens-... covers other PreTokenGeneration checks and does not carry this replay key.
Why: a handler cannot tell a refresh from a sign-in, so claim enrichment that must only happen once (or only on refresh) is applied in the wrong place and the resulting access token carries wrong claims.
Fix: the triggerSource value the cognito-idp user-pool trigger dispatcher passes for the REFRESH_TOKEN_AUTH path.
Traces: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication.

## Evidence
The check fails at its own assertion comparing the triggerSource seen on the refresh flow with the one seen on authentication. Expected: 'TokenGeneration_RefreshTokens' for the REFRESH_TOKEN_AUTH mint and 'TokenGeneration_Authentication' for the initial sign-in.

## Steps to Reproduce
CreateUserPool with LambdaConfig.PreTokenGenerationConfig (LambdaVersion V2_0); sign a user in with USER_PASSWORD_AUTH and record the trigger event's triggerSource; then InitiateAuth with REFRESH_TOKEN_AUTH and record it again.

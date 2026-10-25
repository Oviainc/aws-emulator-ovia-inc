Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cognito-define-auth-challenge-trigger-chain
Issue-Key: cognito-identity-provider|AdminInitiateAuth|NotImplemented
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# apigateway__cognito-idp__lambda: the DefineAuthChallenge trigger chain is unreachable because AdminInitiateAuth with AuthFlow=CUSTOM_AUTH answers NotImplemented

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Cause 1 of 1: AdminInitiateAuth is implemented for other auth flows but refuses AuthFlow=CUSTOM_AUTH with the scaffold error, so the Cognito custom-authentication trigger chain (DefineAuthChallenge -> CreateAuthChallenge -> VerifyAuthChallengeResponse) never runs.

AWS: AdminInitiateAuth with AuthFlow=CUSTOM_AUTH starts a custom authentication flow: the pool's DefineAuthChallenge Lambda trigger is invoked, and the response carries the ChallengeName the trigger selected plus a Session to carry into AdminRespondToAuthChallenge. Recorded behavior: the call succeeds (HTTP 200) and the configured Lambda triggers are invoked in order.

Emulator: the call is refused with HTTP error code NotImplemented and message "operation AdminInitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator". The refusal is per-AuthFlow, so the operation itself is present — only the CUSTOM_AUTH branch is a stub.

Why: no custom-auth-based sign-in can be emulated, and every Lambda trigger that only fires inside the custom-auth flow is dead code from the client's point of view. A client exercising a passwordless/OTP design gets an unmodeled error code where AWS returns a challenge.

Fix: the AdminInitiateAuth handler's CUSTOM_AUTH branch in the cognito-idp service, plus the DefineAuthChallenge/CreateAuthChallenge/VerifyAuthChallengeResponse trigger dispatch it drives. The open issue 20261023T170254Z-apigateway-cognito-idp-lambda-five-causes-cognito-authorizer.md covers Cognito-authorizer traffic and GetAccount for this same service tuple; it does not carry this check id or this AuthFlow branch.

Traces: apigateway__cognito-idp__lambda.cognito-define-auth-challenge-trigger-chain.

## Evidence
apigateway__cognito-idp__lambda.cognito-define-auth-challenge-trigger-chain [error]
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AdminInitiateAuth operation: operation AdminInitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator

expected: HTTP 200 with the ChallengeName chosen by the pool's DefineAuthChallenge trigger and a Session token
actual:   error code NotImplemented; no trigger is invoked

## Steps to Reproduce
1. CreateUserPool with a DefineAuthChallenge (and CreateAuthChallenge / VerifyAuthChallengeResponse) Lambda trigger configured, plus a client and a confirmed user.
2. Call AdminInitiateAuth with AuthFlow=CUSTOM_AUTH for that user.
3. Observe error code NotImplemented instead of a challenge response; the DefineAuthChallenge Lambda is never invoked.

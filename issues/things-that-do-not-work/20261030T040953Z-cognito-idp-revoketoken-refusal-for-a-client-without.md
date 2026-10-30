Issue-Source: trace-sweep-divergence
Issue-Trace: cip-cfg-revoke-token-disabled-on-client
Issue-Step: task#6
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-cfg-revoke-token-disabled-on-client
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|RevokeToken|4xx->4xx|$.message
Issue-Commit: 390a3ffeb67353645c4fabe5e8016102d43849f4

# cognito-idp RevokeToken refusal for a client without token revocation uses emulator wording instead of AWS's message

> _Note: this issue was identified against the code base at commit `390a3ffeb67353645c4fabe5e8016102d43849f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RevokeToken

## Description
One recorded cause: the refusal RevokeToken raises when the app client it is called with does not have token revocation enabled carries the emulator's own sentence rather than AWS's.

Cause 1 of 1: RevokeToken against an app client created with EnableTokenRevocation=false.
AWS: HTTP 400 with the modeled error OperationNotEnabledException (Smithy: RevokeToken errors include OperationNotEnabledException, errorKind client, httpError 400; "thrown when an operation is not available ... for the current user pool configuration"). The recorded response body is {"__type": "OperationNotEnabledException", "message": "This feature is not enabled for this client"} with x-amzn-errortype naming the same error. The message is entirely FIXED text - 'This feature is not enabled for this client' - with no variable part: it names neither the client id, nor the token, nor the pool, so nothing in it is taken from the request or from emulator state.
Emulator: the status (400) and the error code (OperationNotEnabledException) are correct, but the message is rendered as 'Token revocation is not enabled for this app client.' - different wording, a different shape (it names the feature), and a trailing period AWS does not emit.
Why: a client that branches on the refusal text (SDK error-message assertions, log/alert matching, user-facing surfacing of the refusal) sees a sentence that never occurs on real Cognito, so revocation-disabled handling that works against AWS does not match here. Because the error code is already right, this is only reachable through the message.
Fix: the RevokeToken handler's refusal path (the token-revocation-enabled gate) must emit AWS's fixed sentence for OperationNotEnabledException verbatim instead of composing its own; it belongs with that operation's error construction, not its serializer, since the code and status are already correct.
Traces: cip-cfg-revoke-token-disabled-on-client#task#6 (1 recorded step).

This divergence is not carried by any open cognito-idp issue: 20261029T184956Z-cognito-idp-revocation-provider-linking-risk-and-mfa.md covers revocation STATE not being honored (GetUser/GetTokensFromRefreshToken accepting revoked credentials) and carries no RevokeToken replay key, and 20261029T184956Z-cognito-idp-ten-duplicate-resource-and-policy-refusal.md / 20261029T184956Z-cognito-idp-validation-and-auth-refusal-message-rendering.md carry message keys for other operations only.

## Evidence
RevokeToken (cip-cfg-revoke-token-disabled-on-client#task#6), expected_status 400 -> actual_status 400:
  $.message: value_mismatch (expected 'This feature is not enabled for this client', got 'Token revocation is not enabled for this app client.')
No other field diverged in this cluster: $.__type and $headers.x-amzn-errortype already read OperationNotEnabledException, and the status already matches, so the whole divergence is the message text.
Smithy grounding: RevokeToken -> errors [ForbiddenException 403, InternalErrorException 500, InvalidParameterException 400, OperationNotEnabledException 400, TooManyRequestsException 429, UnauthorizedException 401, UnsupportedOperationException 400, UnsupportedTokenTypeException 400].

## Steps to Reproduce
1. Replay fixture cip-cfg-revoke-token-disabled-on-client against the emulator; it creates a user pool and an app client with token revocation disabled, authenticates, and then calls RevokeToken with the issued refresh token.
2. Inspect step task#6 (RevokeToken): the status is 400 and $.__type is OperationNotEnabledException as recorded, but $.message reads 'Token revocation is not enabled for this app client.'
3. Expected: $.message is exactly 'This feature is not enabled for this client' (fixed text, no substituted values).

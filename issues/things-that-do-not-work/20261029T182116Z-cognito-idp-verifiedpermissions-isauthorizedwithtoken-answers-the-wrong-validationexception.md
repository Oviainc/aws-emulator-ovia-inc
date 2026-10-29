Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-unlisted-client-rejected
Issue-Key: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-unlisted-client-rejected|run#22|AssertionError|assert code == ("ValidationException", "Token audience claim contains no valid audience"), (
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp__verifiedpermissions: IsAuthorizedWithToken answers the wrong ValidationException message for an ID token whose aud is not in the identity source's clientIds

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IsAuthorizedWithToken

## Description
Cause 1 of 1 — the audience-rejection message is emulator prose.
AWS: an ID token from a Cognito user pool whose aud claim is not among the identity source's configured clientIds is refused with ValidationException and the fixed text 'Token audience claim contains no valid audience' — no variable parts, in particular no echo of the rejected aud value.
Emulator: ValidationException with 'Failed to verify IdentityToken: its aud claim "<aud>" is not one of the clientIds the identity source accepts' (recorded aud 'aa8ee9a45a4049898860ea51d5'). The error code is right; the message is not, and it echoes the token's audience, which AWS withholds.
Why: a caller matching AWS's documented text to distinguish an audience failure from other validation failures never matches, and the emulator discloses a value from the rejected token.
Fix: the token-validation refusal path of IsAuthorizedWithToken (audience check) in the verifiedpermissions handler.
Traces: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-unlisted-client-rejected.

## Evidence
assert code == ('ValidationException', 'Token audience claim contains no valid audience') -> AssertionError: an ID token whose aud is not in clientIds should be ValidationException, got ('ValidationException', 'Failed to verify IdentityToken: its aud claim "aa8ee9a45a4049898860ea51d5" is not one of the clientIds the identity source accepts'). The quoted aud is a per-run client id filling the emulator's template; AWS's text has no variable part.

## Steps to Reproduce
1. CreateUserPool with two app clients; create a Verified Permissions policy store and a Cognito identity source listing only client A.
2. Sign in through client B to obtain an ID token.
3. IsAuthorizedWithToken with that identity token -> expect ValidationException 'Token audience claim contains no valid audience'.

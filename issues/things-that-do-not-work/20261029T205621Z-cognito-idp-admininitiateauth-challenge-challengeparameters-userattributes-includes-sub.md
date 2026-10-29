Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-auth-challenge-flow
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-auth-challenge-flow
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminInitiateAuth|2xx->2xx|$.Session
Issue-Related: 20261029T184956Z-cognito-idp-authentication-success-responses-omit-challengeparameters-and.md
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# cognito-idp AdminInitiateAuth challenge ChallengeParameters userAttributes includes sub

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
This issue has 1 cause.

AWS: when AdminInitiateAuth returns a challenge, `ChallengeParameters` is a map of string guidance values the caller must echo back in AdminRespondToAuthChallenge. Smithy: AdminInitiateAuthResponse.ChallengeParameters — 'The parameters of an authentication challenge. Amazon Cognito returns challenge parameters as a guide to the responses your user or application must provide for the returned ChallengeName.' The recorded response for this challenge carries `userAttributes` as the JSON-encoded object `{}` — the immutable `sub` identifier is NOT projected into the challenge's userAttributes guidance (recorded behavior for the userAttributes content; the model does not enumerate the keys).

Emulator: the emulator sets `ChallengeParameters.userAttributes` to `{"sub":"<user-sub-uuid>"}`, i.e. it serializes the internal `sub` attribute into the challenge guidance where AWS projects an empty object.

Why: a client (or a DefineAuthChallenge/CreateAuthChallenge Lambda chain) that reads `ChallengeParameters.userAttributes` to decide which attributes it must collect, or that hashes/echoes the guidance verbatim into AdminRespondToAuthChallenge, sees a user attribute that AWS never advertises. Code that treats a non-empty userAttributes as 'the user already has profile attributes' branches differently than it does against AWS, and anything that round-trips the value back into the challenge response sends a parameter AWS would not have asked for.

Fix: the AdminInitiateAuth handler's challenge-response builder in the cognito-idp service crate — the projection that fills ChallengeParameters.userAttributes must exclude the server-generated `sub` (and any other internal identifier) and emit `{}` when the user carries no advertisable attributes.

Note on the step's other assertion: the same step also reports `$.Session` differing from the fixture's `{{challenge_session}}` placeholder. The session token is an opaque per-run value, so that part is not a requirement and is not claimed here; only the userAttributes projection is.

Traces: fixture cognito-idp-admin-auth-challenge-flow, step task#4.

## Evidence
Expected (recorded AWS): `$.ChallengeParameters.userAttributes` = `'{}'`.
Actual (emulator): `$.ChallengeParameters.userAttributes` = `'{"sub":"<uuid>"}'` — reported as `value_mismatch (expected '{}', got '{"sub":"4784bd95-..."}')`. The concrete uuid is a per-run value; the defect is that the `sub` key is present at all.
HTTP status matched (200) on both sides.
Smithy: AdminInitiateAuthResponse.ChallengeParameters is a StringMap of challenge guidance; the docs describe USERNAME, SECRET_HASH and USER_ID_FOR_SRP, and do not include an internal identifier in userAttributes.

## Steps to Reproduce
1. Replay cognito-idp-admin-auth-challenge-flow step task#4 — AdminInitiateAuth on a pool/client configured to return a challenge.
2. Read `ChallengeParameters.userAttributes` in the 200 response: the emulator returns a JSON object containing a `sub` key; AWS returns `{}`.
3. Ignore the `Session` value in the same step: it is an opaque per-run token.

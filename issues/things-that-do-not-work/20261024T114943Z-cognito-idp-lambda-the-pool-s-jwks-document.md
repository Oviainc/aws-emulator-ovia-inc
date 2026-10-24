Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims cognito-idp__lambda.custom-auth-challenge-triad-issues-tokens-only-after-the-secret-answer cognito-idp__lambda.user-migration-trigger-creates-the-user-and-returns-tokens-on-first-sign-in
Issue-Key: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims|_verify_jwt_against_jwks#21|HTTPError|with urllib.request.urlopen(url, timeout=30) as r: # noqa: S310 - fixed scheme from the client
Issue-Key: cognito-identity-provider|InitiateAuth|NotImplemented
Issue-Key: cognito-idp__lambda.user-migration-trigger-creates-the-user-and-returns-tokens-on-first-sign-in|run#14|AssertionError|assert precondition["Error"]["Message"] == "User does not exist.", (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# cognito-idp__lambda: the pool's JWKS document is not served, InitiateAuth CUSTOM_AUTH is unimplemented, and UserNotFoundException names the user (3 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Three root causes blocking the Cognito-plus-Lambda trigger scenarios, all grounded in failing checks the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 3 — the per-pool JWKS document is not served: GET <cognito-idp endpoint>/<poolId>/.well-known/jwks.json answers HTTP 404.
AWS: recorded behavior only — every user pool publishes its signing keys at https://cognito-idp.<region>.amazonaws.com/<userPoolId>/.well-known/jwks.json as a JSON document {'keys': [ {kid, kty: 'RSA', alg: 'RS256', n, e, use: 'sig'} … ]}, and the 'kid' in the header of an issued IdToken/AccessToken is present in it so the token's RS256 signature verifies. This endpoint is how every resource server, API Gateway JWT authorizer and OIDC client validates a Cognito token.
Emulator: urllib.error.HTTPError: HTTP Error 404: Not Found for that URL, so no token the emulator mints can be verified and the check aborts before it can inspect the claims a PreTokenGeneration V2_0 trigger added, suppressed or scoped.
Why: token verification is the foundation every relying party stands on; with no JWKS, no client, authorizer or federated service can accept a Cognito token, and claim-shaping behavior cannot be observed at all.
Fix: serve the pool JWKS route in the cognito-idp front end, backed by the per-pool signing key used to mint tokens. Related open issue 20261023T125454Z-cognito-idp-cognito-every-unsigned-anonymous-operation-answers-unknownservice-admininitiateauth.md records (as its Cause 3) that issued tokens are alg='none' with an empty signature and says the fix must 'publish the matching JWKS'; it carries neither this check's replay key nor this service token, so a fixer could close it with the JWKS route still absent.

Cause 2 of 3 — InitiateAuth with AuthFlow=CUSTOM_AUTH answers NotImplemented.
AWS: Smithy (cognito-idp) InitiateAuth requires AuthFlow of type AuthFlowType, which includes CUSTOM_AUTH, and returns a ChallengeName/Session/ChallengeParameters triple driven by the DefineAuthChallenge and CreateAuthChallenge triggers; the modeled errors are InvalidParameterException, InvalidLambdaResponseException, NotAuthorizedException, UserLambdaValidationException, UnexpectedLambdaException, InvalidUserPoolConfigurationException, UserNotFoundException and friends — 'NotImplemented' is not among them, and the operation itself is implemented here for other flows.
Emulator: (NotImplemented) 'operation InitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator'. The already-filed cause covers AdminInitiateAuth's CUSTOM_AUTH branch only (Issue-Key cognito-identity-provider|AdminInitiateAuth|NotImplemented in 20261023T125454Z-…); the self-service InitiateAuth branch is a different handler branch and carries no filed replay key.
Why: the self-service custom-authentication challenge chain (DefineAuthChallenge / CreateAuthChallenge / VerifyAuthChallengeResponse) cannot be exercised at all, so no passwordless or secret-answer sign-in flow can be emulated and no client of it can be tested.
Fix: the CUSTOM_AUTH branch of the InitiateAuth handler and the challenge/session state it must create, plus the RespondToAuthChallenge continuation.

Cause 3 of 3 — AdminGetUser's UserNotFoundException message echoes the requested username.
AWS: Smithy UserNotFoundException (httpError 404, 'thrown when a user isn't found'); for AdminGetUser of a user that does not exist AWS returns the fixed, request-independent message 'User does not exist.', which is what a user-migration client matches to decide that the user must be migrated rather than that the pool is broken.
Emulator: {'Code': 'UserNotFoundException', 'Message': 'User sim2real-xsvc-cip-migrant-25fbdc58747e does not exist.'} — the username is interpolated into the message. The status and code are already right.
Why: the fixed wording is deliberate — it keeps the error from confirming the existence of one specific identity. Interpolating the username turns the 404 into a user-enumeration oracle and breaks consumers that match the documented string.
Fix: the message construction for UserNotFoundException in the AdminGetUser handler's user lookup (shared with the other admin operations that resolve a username).

Traces: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims (cause 1), cognito-idp__lambda.custom-auth-challenge-triad-issues-tokens-only-after-the-secret-answer (cause 2), cognito-idp__lambda.user-migration-trigger-creates-the-user-and-returns-tokens-on-first-sign-in (cause 3).

## Evidence
Cause 1: urllib.error.HTTPError: HTTP Error 404: Not Found, raised from the check's _verify_jwt_against_jwks helper at url = f"{client.meta.endpoint_url.rstrip('/')}/{pool_id}/.well-known/jwks.json". Expected a 200 with a JWKS whose 'keys' contains the token header's kid.
Cause 2: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the InitiateAuth operation: operation InitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator. Expected a 200 with ChallengeName/Session.
Cause 3: AssertionError: AdminGetUser of the not-yet-migrated user answered with the wrong message: {'Message': 'User sim2real-xsvc-cip-migrant-25fbdc58747e does not exist.', 'Code': 'UserNotFoundException'}. Expected message 'User does not exist.' with the same 404/UserNotFoundException.
None of the three checks carried an 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
Cause 1: CreateUserPool + CreateUserPoolClient, AdminCreateUser and sign in to obtain an AccessToken, then HTTP GET <cognito-idp endpoint>/<poolId>/.well-known/jwks.json -> expect 200 with a 'keys' array containing the token header's kid, observe HTTP 404.
Cause 2: CreateUserPool with LambdaConfig {DefineAuthChallenge, CreateAuthChallenge, VerifyAuthChallengeResponse}, CreateUserPoolClient with ALLOW_CUSTOM_AUTH, then InitiateAuth(AuthFlow='CUSTOM_AUTH', AuthParameters={'USERNAME': u}) -> expect ChallengeName/Session, observe NotImplemented.
Cause 3: CreateUserPool with a UserMigration trigger, then AdminGetUser for a username that does not exist yet -> expect ('UserNotFoundException', 'User does not exist.'), observe the username interpolated into the message.

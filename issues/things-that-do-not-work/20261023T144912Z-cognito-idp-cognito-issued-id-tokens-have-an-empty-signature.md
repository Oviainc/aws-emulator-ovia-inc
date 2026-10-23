Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-group-permit-allows-member-denies-non-member cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-forged-group-claim-rejected cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-group-removal-applies-only-to-reissued-token cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-tampered-token-signature-rejected cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-foreign-pool-rejected cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-unlisted-client-rejected
Issue-Key: verifiedpermissions|IsAuthorizedWithToken|ValidationException
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# Cognito-issued ID tokens have an empty signature segment, so every Cognito -> Verified Permissions IsAuthorizedWithToken flow is refused with ValidationException

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
One root cause, grounded in all six failing cross-service checks of the group `cognito-token-authorization` (check ids carry the composite leading token `cognito-idp__verifiedpermissions`; SERVICES = ["cognito-idp", "verifiedpermissions"]). The defect is in the Cognito user-pool token minting, not in Verified Permissions: AVP's input validation is faithful to the model and correctly refuses the token the emulator's Cognito hands out.

Cause 1 of 1: the ID token returned by the user-pool authentication operations is an unsigned JWT (header `{"alg":"none","typ":"JWT"}`, third segment empty), and therefore cannot be passed to any consumer that requires a three-segment signed JWT.

AWS: recorded behavior only for the token contents — Cognito issues RS256-signed JWTs whose third segment is a non-empty base64url signature verifiable against the pool's JWKS. The wire requirement that makes this observable IS modeled: Smithy (com.amazonaws.verifiedpermissions) IsAuthorizedWithToken.identityToken targets `Token` with @length min 1 max 131072 and @pattern `^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$` — three NON-EMPTY base64url segments — and the operation models ValidationException (400). A real Cognito ID token satisfies that pattern, so on AWS the call is evaluated and returns a decision (ALLOW/DENY) with `principal`; only a token whose claims no longer match its signature is refused, with ValidationException and a message of the form "Failed to verify IdentityToken: signature is invalid".

Emulator: every call fails at input validation with HTTP 400 `ValidationException`, example message `1 validation error detected: Value 'eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiI...' at 'identityToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$`. The rejected value is the IdToken the emulator's own Cognito just returned: its header base64-decodes to `{"alg":"none","typ":"JWT"}` and its third segment is empty, so `[A-Za-z0-9-_=]+` cannot match it. The same token is rejected on the positive controls (a genuine group member's token), on the negative controls (a token from a foreign pool, from an unlisted client, with a tampered signature) and on the forged-claims case alike, so none of the six checks reaches an authorization decision at all.

Why: a client cannot use a Cognito token with Verified Permissions (or with any API Gateway/JWT authorizer or library that verifies signatures) at all — the identity-source wiring is unreachable. Worse for correctness testing, the defect masks the authorization semantics underneath it: whether a `cognito:groups` claim is bound to the signature, whether a token from another pool or an unlisted client is refused, and whether group removal only takes effect on a reissued token are all unverifiable, and the over-permissive failure mode (trusting claims without binding them to a signature, turning a DENY into an ALLOW) cannot be caught.

Fix: token minting in the user-pool auth operations of the cognito-identity-provider service — sign the ID/access tokens with a per-pool key (RS256 with a `kid`) and emit a non-empty base64url signature segment, publishing the matching JWKS so a verifier can validate them. The already-open issue `20261023T125454Z-cognito-idp-cognito-every-unsigned-anonymous-operation-answers-unknownservice-admininitiateauth.md` reports the same minting defect as its Cause 3, but it carries none of the six replay keys below (it is grounded only in an apigateway authorizer check), so a fixer could close it without ever re-running the Cognito -> AVP flows; this issue carries them.

Traces: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-group-permit-allows-member-denies-non-member, .xsvc-cognito-idp-2-avp-forged-group-claim-rejected, .xsvc-cognito-idp-2-avp-group-removal-applies-only-to-reissued-token, .xsvc-cognito-idp-2-avp-tampered-token-signature-rejected, .xsvc-cognito-idp-2-avp-token-from-foreign-pool-rejected, .xsvc-cognito-idp-2-avp-token-from-unlisted-client-rejected.

## Evidence
All six checks fail identically at the first IsAuthorizedWithToken call:

botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the IsAuthorizedWithToken operation: 1 validation error detected: Value 'eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiI1OWY0ZWFmYi00NmM0LTQzNWEtYTc5My1kNWEzMTNhMzE4ZDkiLCJpc3MiOiJodHRwczovL2NvZ25pdG8taWRwLnVzLXdlc3QtMi5hbWF6b25hd3MuY29tL3VzLXdlc3QtMl9jYmUxODlmNmEiLCJhdWQiOiJmMmQwMjZkZjM4ZmI0ZDUxOTU2ZDY0MjRhNyIsImNvZ25pdG86dXNl... [543 bytes]' at 'identityToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$

The rejected value is the emulator's own token: `eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0` decodes to {"alg":"none","typ":"JWT"} and the token's third segment is empty (nothing after the final '.'), which the modeled pattern's `[A-Za-z0-9-_=]+` third group cannot match.

Expected (AWS): the call is evaluated. The group-permit check expects decision ALLOW with principal {entityType: 'App::User', entityId: '<poolId>|<sub>'} for the admins member and DENY for the non-member; the forged-claims check expects ('ValidationException', 'Failed to verify IdentityToken: signature is invalid') for a token whose payload was rewritten while keeping the original signature (the error CODE and 400 status are the requirement; the sentence is an example).

Actual (emulator): 400 ValidationException with the identityToken pattern violation on EVERY call, including the ones that must succeed, so no decision, no `principal`, and no signature-verification behavior is ever reached.

## Steps to Reproduce
1. cognito-idp: CreateUserPool, CreateUserPoolClient, CreateGroup('admins'), AdminCreateUser + AdminSetUserPassword for two users, AdminAddUserToGroup for one of them.
2. Authenticate a user and read AuthenticationResult.IdToken. Base64-decode the header: observe {"alg":"none","typ":"JWT"}; split on '.' and observe that the third segment is empty.
3. verifiedpermissions: CreatePolicyStore, PutSchema, CreateIdentitySource(configuration={cognitoUserPoolConfiguration:{userPoolArn, clientIds:[clientId]}}), CreatePolicy permitting the admins group.
4. Call IsAuthorizedWithToken(policyStoreId, identityToken=<that IdToken>, action={actionType:'App::Action', actionId:'read'}, resource={entityType:'App::Doc', entityId:'doc-1'}).
   Expected: 200 with decision ALLOW and the principal entity. Observed: 400 ValidationException, identityToken fails the three-non-empty-segment pattern.
5. Replay the six checks listed in trace_ids to confirm.

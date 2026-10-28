Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-foreign-pool-rejected
Issue-Key: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-foreign-pool-rejected|run#31|AssertionError|assert code == ("ResourceNotFoundException", no_source), (
Issue-Commit: 508ed8e073c225e6c3709c17b5f5adc75decad9c

# IsAuthorizedWithToken: ResourceNotFoundException for a token whose issuer has no identity source carries the wrong message wording (omits the policy store)

> _Note: this issue was identified against the code base at commit `508ed8e073c225e6c3709c17b5f5adc75decad9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IsAuthorizedWithToken

## Description
One cause.

Cause 1 of 1: Verified Permissions IsAuthorizedWithToken refuses a Cognito ID/access token whose `iss` claim names a user pool that no identity source in the policy store is configured for, but the refusal message does not use AWS's wording and does not name the policy store.

AWS: Smithy (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json) binds IsAuthorizedWithToken {policyStoreId required, identityToken, accessToken, action, resource} and models ResourceNotFoundException (errorKind client, httpError 404, members message/resourceId/resourceType all required) — "The request failed because it references a resource that doesn't exist." The model does not state the message text, so for the text this is recorded behavior only: the recording shows AWS answering ResourceNotFoundException whose message is the fixed text `Identity Source does not exist for issuer ` + the token's `iss` claim + ` in policy store ` + the request's policyStoreId. Fixed text: "Identity Source does not exist for issuer " and " in policy store ". Variable parts, taken from the request: the `iss` claim decoded from the submitted identityToken/accessToken, and the policyStoreId member of the request. The same message is returned for both the identityToken and the accessToken form of the call, and the policy store itself remains usable — a token from a pool that IS covered by an identity source still evaluates to ALLOW immediately before and after the refusal.

Emulator: the status/error code are right (ResourceNotFoundException) but the message is a generic not-found string built from the issuer alone: `Identity source with id '<iss>' not found` (observed: "Identity source with id 'https://cognito-idp.us-west-2.amazonaws.com/us-west-2_180b12961' not found" — the pool id and region there are per-run values, not part of the requirement). It uses the words "Identity source with id" / "not found" instead of "Identity Source does not exist for issuer" / "in policy store", quotes the issuer, and drops the policy-store identifier entirely.

Why: a client that matches or surfaces the refusal reason gets it wrong. The AWS message distinguishes "this policy store has no identity source for that issuer" from "the identity source id you named does not exist"; the emulator's wording describes the issuer as an identity source id, so operators and tests that assert on, log, or route on the message text (and any client that reports which policy store refused the token) diverge from AWS. Suites that pin the documented refusal text fail against the emulator even though the code and HTTP status match.

Fix: the IsAuthorizedWithToken handler's token-to-identity-source resolution in the verifiedpermissions service crate — the branch that raises ResourceNotFoundException when no identity source in the named policy store matches the token's `iss` claim must format AWS's message (issuer + policy store id) rather than a generic "identity source with id ... not found", and must populate the modeled resourceId/resourceType members consistently with it. Applies to both the identityToken and accessToken request forms.

Traces: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-foreign-pool-rejected (assertion at check line 76: expected ("ResourceNotFoundException", "Identity Source does not exist for issuer <iss> in policy store <storeId>"), got ("ResourceNotFoundException", "Identity source with id '<iss>' not found")). No open issue currently covers verifiedpermissions (list_existing_issues query='verifiedpermissions' returns none).

## Evidence
Check: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-foreign-pool-rejected — verdict: mismatch.

Expected (real AWS, from the recording):
  error code: ResourceNotFoundException (HTTP 404)
  message: "Identity Source does not exist for issuer <iss> in policy store <policyStoreId>"
           fixed text = "Identity Source does not exist for issuer " ... " in policy store ";
           <iss> = the `iss` claim of the submitted token; <policyStoreId> = request policyStoreId.

Actual (emulator):
  error code: ResourceNotFoundException
  message: "Identity source with id 'https://cognito-idp.us-west-2.amazonaws.com/us-west-2_180b12961' not found"
  (the issuer URL/pool id shown is a per-run value; the diverging part is the fixed wording and the missing policy-store id)

AssertionError from the replay:
  assert code == ("ResourceNotFoundException", no_source)
  AssertionError: an ID token from a pool with no identity source should be ResourceNotFoundException, got ('ResourceNotFoundException', "Identity source with id 'https://cognito-idp.us-west-2.amazonaws.com/us-west-2_180b12961' not found")

The positive control in the same check passes: the home pool's admins token evaluates to ALLOW with principal {App::User, "<poolId>|<sub>"} before the refusal, so the store and policy wiring are intact and the refusal is about the issuer only.

## Steps to Reproduce
1. cognito-idp: CreateUserPool (pool A) + CreateUserPoolClient, CreateGroup "admins", AdminCreateUser "alice", AdminSetUserPassword (Permanent), AdminAddUserToGroup.
2. verifiedpermissions: CreatePolicyStore (validationSettings mode OFF), CreateIdentitySource with cognitoUserPoolConfiguration naming pool A's ARN and client id (groupConfiguration groupEntityType App::UserGroup), CreatePolicy permitting principal in App::UserGroup::"<poolAId>|admins" for action App::Action::"read".
3. AdminInitiateAuth (ADMIN_USER_PASSWORD_AUTH) on pool A for alice; IsAuthorizedWithToken with that identityToken -> ALLOW (positive control).
4. Create a SECOND user pool B with the same group/user/password, authenticate alice there, so only the `iss` claim differs.
5. Call IsAuthorizedWithToken on the SAME policyStoreId with identityToken = pool B's ID token (then again with accessToken = pool B's access token).
6. Observe the refusal: code is ResourceNotFoundException as AWS does, but the message is "Identity source with id '<iss>' not found" instead of "Identity Source does not exist for issuer <iss> in policy store <policyStoreId>".
7. Re-run IsAuthorizedWithToken with pool A's token -> still ALLOW, confirming the store exists and the divergence is in the issuer-mismatch refusal message.

Issue-Source: trace-sweep-divergence
Issue-Trace: cip-cfg-alias-email-duplicate-signup-and-alias-claim
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-cfg-alias-email-duplicate-signup-and-alias-claim
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminGetUser|2xx->2xx|$.UserAttributes[*].Value
Issue-Related: 20261029T184956Z-cognito-idp-username-canonicalization-and-attribute-projection-diverge.md
Issue-Commit: 97b52288e8c9303330ae8e79df94bfb75ff84489

# cognito-idp: email alias lookups resolve to the previous alias owner after an alias claim

> _Note: this issue was identified against the code base at commit `97b52288e8c9303330ae8e79df94bfb75ff84489`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminGetUser

## Description
One recorded cause in the alias-resolution path of the user read model.

Cause 1 of 1: resolving a user by an alias attribute (email) after that alias has been claimed by a second user.

AWS: AdminGetUser accepts an alias attribute in place of the username — Smithy: AdminGetUserRequest.Username documentation, "The value of this parameter is typically your user's username, but it can be any of their alias attributes." On a pool whose AliasAttributes include email, the email address maps to exactly ONE user at a time: when a second sign-up for the same address is confirmed, the alias is transferred to that second user and stops pointing at the first. A subsequent AdminGetUser by the email address therefore answers 200 with the CURRENT alias owner — the user the service last linked the address to — returning that user's own Username and that user's own service-generated `sub` in UserAttributes. Recorded behavior only for the transfer-on-confirmation step itself (the model states no rule beyond the alias-lookup sentence above); the recording shows the lookup resolving to the second, most recently confirmed user.

Emulator: the same AdminGetUser answers 200 but returns the FIRST user — the alias is never re-pointed when the second sign-up claims it. In the replayed step the response carries Username 'alias-user-one' where the recording has 'alias-user-two', and the projected `sub` attribute is correspondingly the first user's sub rather than the alias owner's. (The sub values themselves are per-run UUIDs and are not a requirement; what diverges is WHICH user's record the alias resolves to.)

Why: a client that administers a user by email address — AdminSetUserPassword, AdminAddUserToGroup, AdminDisableUser, AdminDeleteUser, AdminUpdateUserAttributes — addresses the stale account. The request succeeds against the wrong identity, so a password reset, group grant or deletion intended for the current owner of the address silently lands on a different user, and the sub a client caches from this read keys all later calls to the wrong principal.

Fix: alias-attribute resolution in the user-pool state — the alias index must be re-pointed to the claiming user when a duplicate sign-up for an alias attribute is confirmed, and the AdminGetUser handler must project the resolved alias owner's own Username and attributes rather than the first record that ever held the address.

Traces: cip-cfg-alias-email-duplicate-signup-and-alias-claim#task#13.

Related open issue that does NOT carry this replay key: 20261029T184956Z-cognito-idp-username-canonicalization-and-attribute-projection-diverge.md — its Cause 6 covers step task#9 of the same fixture (AdminGetUser for the ORIGINAL username answering 404). That is the complementary half; this issue is the alias lookup resolving to the wrong user at step task#13 and is not covered by it.

## Evidence
AdminGetUser (fixture cip-cfg-alias-email-duplicate-signup-and-alias-claim, step task#13), status 200 expected and 200 actual, so the divergence is entirely in the body:
  $.Username: value_mismatch (expected 'alias-user-two', got 'alias-user-one')
  $.UserAttributes[2].Value: value_mismatch (expected the alias owner's sub, got the first user's sub — recorded as expected '3869cb49-f5d8-4526-b0d8-3827234907a1', got '2258f017-0861-4aa0-91d0-741864b64f62'; both are per-run UUIDs, the requirement is that it is the sub of the user the alias now names)

Expected: the lookup by the shared email address returns the user that most recently claimed the alias (Username 'alias-user-two' in this recording) together with that user's own sub.
Actual: the lookup returns the original signer-up ('alias-user-one') and that user's sub; the alias was never transferred when the second sign-up was confirmed.
There is no status, error code or error-message divergence in this cluster — both sides answer HTTP 200.

## Steps to Reproduce
1. Replay fixture cip-cfg-alias-email-duplicate-signup-and-alias-claim against the emulator.
2. The fixture creates a user pool with AliasAttributes including email, signs up a first user (alias-user-one) with an email address and confirms it, then signs up a second user (alias-user-two) with the SAME email address and confirms it, which claims the email alias.
3. At step task#13 it calls AdminGetUser with Username set to the shared email address.
4. Compare the response body against the recording: the emulator returns Username 'alias-user-one' and the first user's sub, where the recording returns Username 'alias-user-two' and that user's sub.

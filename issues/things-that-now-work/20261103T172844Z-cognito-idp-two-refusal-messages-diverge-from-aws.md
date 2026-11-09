Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.op-contract-adminLinkProviderForUser cognito-idp.alias-exists-on-provider-link cognito-idp.op-contract-admin-respond-to-auth-challenge
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.op-contract-adminLinkProviderForUser|run#15|AssertionError|assert code.get("Message") == "Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser", code
Issue-Key: cognito-idp.alias-exists-on-provider-link|run#12|AssertionError|assert code_msg == 'Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser', f"expected the
Issue-Key: cognito-idp.op-contract-admin-respond-to-auth-challenge|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: d5f23b12615bd3516afbad21c99e563e38efe994

# cognito-idp: two refusal messages diverge from AWS wording — AdminLinkProviderForUser's already-linked InvalidParameterException and AdminRespondToAuthChallenge's bogus-session CodeMismatchException (2 causes)

> _Note: this issue was identified against the code base at commit `d5f23b12615bd3516afbad21c99e563e38efe994`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminLinkProviderForUser, AdminRespondToAuthChallenge

## Description
Two net-new root causes in the cognito-idp emulator, both message-text divergences on refusals whose HTTP status and error code are already correct. Neither is covered by the open message-wording issues 20261103T075017Z-cognito-idp-eight-refusal-messages-are-emulator-authored.md, 20261029T184956Z-cognito-idp-validation-and-auth-refusal-message-rendering.md or 20261030T233240Z-cognito-idp-branding-and-user-pool-not-found.md — those name other operations, and the suite does not list these checks as owned by them.

Cause 1 of 2: AdminLinkProviderForUser serves the AliasExists wording for the InvalidParameterException it raises when the SourceUser identity is already linked.
AWS: Smithy: AdminLinkProviderForUser models both InvalidParameterException (HTTP 400) and AliasExistsException (HTTP 400). Recorded behavior: while a federated SourceUser identity (ProviderName/ProviderAttributeName/ProviderAttributeValue triple) still holds a live link to one local profile, linking that same identity to a second DestinationUser is refused with HTTP 400, error code InvalidParameterException and the message — entirely fixed text, with no values taken from the request or state — 'Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser'. The recording further shows that AFTER AdminDisableProviderForUser detaches that link from the first profile, the identical link attempt is refused with error code AliasExistsException and the fixed message 'SourceUser with the specified attribute value is already linked to another user' (the alias claim is not released by disabling the link).
Emulator: the first refusal carries the right status and code but the wrong text: message 'SourceUser with the specified attribute value is already linked to another user' — i.e. AWS's AliasExistsException wording is emitted as the InvalidParameterException message, so the two distinct refusal classes are rendered with one string. The post-AdminDisableProviderForUser leg (which must become AliasExistsException with that same text) is never reached because the check aborts on the first message assertion.
Why: a client that branches on refusal text (or a test/retry wrapper that distinguishes 'this identity is claimed elsewhere' from 'this SourceUser is not a federated identity') reads the wrong cause for the rejection, and the AliasExists/InvalidParameter distinction AWS draws across the AdminDisableProviderForUser boundary is invisible — both states look identical.
Fix: the message selection in the AdminLinkProviderForUser handler's already-linked branch and the two shared message constants it chooses between (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_link_provider_for_user.rs lines 255-264, which select crate::state::SOURCE_USER_ALREADY_LINKED_MESSAGE / SOURCE_USER_LINKED_TO_ANOTHER_USER_MESSAGE, and the AliasExistsException path that branch should take once the link has been disabled).
Traces: cognito-idp.op-contract-adminLinkProviderForUser, cognito-idp.alias-exists-on-provider-link.

Cause 2 of 2: AdminRespondToAuthChallenge's CodeMismatchException for an unparseable Session carries emulator prose.
AWS: Smithy: AdminRespondToAuthChallenge models CodeMismatchException (HTTP 400); Session is @length 20..4096 and sensitive. Recorded behavior: responding to a NEW_PASSWORD_REQUIRED challenge with a Session value that is not a session the service issued is refused with HTTP 400, error code CodeMismatchException and the message 'Invalid code/session provided' — entirely fixed text, with no part taken from the request (the bogus Session is never echoed) or from state.
Emulator: same status and code, but message 'Invalid code or auth state for the user.' — emulator-authored prose (note also the trailing period, which AWS's text does not have).
Why: a client or SDK-layer assertion that matches on AWS's documented refusal text does not recognise the refusal; log/alert pipelines keyed on the message string see an error class they have no rule for, even though the code is right.
Fix: the CodeMismatchException message rendered by the AdminRespondToAuthChallenge handler's session-validation path (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_respond_to_auth_challenge.rs) and any shared message constant it takes that string from; RespondToAuthChallenge should be checked for the same constant.
Traces: cognito-idp.op-contract-admin-respond-to-auth-challenge.

## Evidence
Cause 1 — check cognito-idp.op-contract-adminLinkProviderForUser:
  assert code.get("Message") == "Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser"
  AssertionError: SourceUser with the specified attribute value is already linked to another user
Same divergence in check cognito-idp.alias-exists-on-provider-link (line 130):
  AssertionError: expected the exact InvalidParameterException message, got 'SourceUser with the specified attribute value is already linked to another user'
  Expected: HTTP 400 / InvalidParameterException / 'Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser'
  Actual:   HTTP 400 / InvalidParameterException / 'SourceUser with the specified attribute value is already linked to another user'
  (the same check's later leg requires AliasExistsException with the message the emulator used here, after AdminDisableProviderForUser — unreached)
Emulator source confirming the single string: src/ops/admin_link_provider_for_user.rs:255-264 returns invalid_parameter(SOURCE_USER_LINKED_TO_ANOTHER_USER_MESSAGE) for an identity already linked to a different profile.

Cause 2 — check cognito-idp.op-contract-admin-respond-to-auth-challenge (line 52):
  AssertionError: expected message 'Invalid code/session provided', got 'Invalid code or auth state for the user.'
  Expected: HTTP 400 / CodeMismatchException / 'Invalid code/session provided'
  Actual:   HTTP 400 / CodeMismatchException / 'Invalid code or auth state for the user.'
  (the check's code assertion passed, so only the message text diverges)

## Steps to Reproduce
Cause 1:
1. CreateUserPool (lenient password policy), then CreateIdentityProvider on it (e.g. ProviderName/ProviderType Facebook, or an OIDC provider).
2. AdminCreateUser twice (dest_a, dest_b) and AdminSetUserPassword Permanent=true on both.
3. AdminLinkProviderForUser(DestinationUser={ProviderName:'Cognito', ProviderAttributeValue:dest_a}, SourceUser={ProviderName:'Facebook', ProviderAttributeName:'Cognito_Subject', ProviderAttributeValue:<external id>}) — succeeds, link visible in dest_a's 'identities' attribute.
4. Repeat step 3 with DestinationUser=dest_b and the SAME SourceUser triple. Observe HTTP 400 InvalidParameterException with message 'SourceUser with the specified attribute value is already linked to another user'; AWS's message is 'Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser'.
5. Call AdminDisableProviderForUser for dest_a's Cognito_Subject, then repeat step 4: AWS refuses with AliasExistsException and the message 'SourceUser with the specified attribute value is already linked to another user'.

Cause 2:
1. CreateUserPool and CreateUserPoolClient with ExplicitAuthFlows=['ALLOW_ADMIN_USER_PASSWORD_AUTH','ALLOW_REFRESH_TOKEN_AUTH'], GenerateSecret=false.
2. AdminCreateUser with a TemporaryPassword and MessageAction=SUPPRESS.
3. AdminInitiateAuth(AuthFlow='ADMIN_USER_PASSWORD_AUTH') — returns ChallengeName NEW_PASSWORD_REQUIRED and a Session.
4. AdminRespondToAuthChallenge(ChallengeName='NEW_PASSWORD_REQUIRED', ChallengeResponses={USERNAME, NEW_PASSWORD}, Session='not-a-real-session-token'). Observe HTTP 400 CodeMismatchException with message 'Invalid code or auth state for the user.' instead of AWS's 'Invalid code/session provided'.

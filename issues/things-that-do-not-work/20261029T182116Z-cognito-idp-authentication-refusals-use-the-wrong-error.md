Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.get-client-token-invalid-secret-rejected cognito-idp.get-client-token-wrong-secret-rejected cognito-idp.op-contract-get-client-token cognito-idp.get-client-token-disallowed-scope-rejected cognito-idp.global-sign-out-invalidates-access-token cognito-idp.get-tokens-from-refresh-token-wrong-client-rejected cognito-idp.op-contract-get-tokens-from-refresh-token cognito-idp.auth-challenge-session-sequencing cognito-idp.op-contract-adminRespondToAuthChallenge cognito-idp.op-contract-admin-respond-to-auth-challenge cognito-idp.auth-flow-not-enabled-fails cognito-idp.unconfirmed-user-auth-error-class-precision cognito-idp.forgot-password-nonexistent-user-no-error-leak cognito-idp.prevent-user-existence-errors-masks-unknown-user cognito-idp.alias-exists-on-provider-link cognito-idp.op-contract-adminLinkProviderForUser
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.get-client-token-invalid-secret-rejected|run#8|AssertionError|assert code == ("NotAuthorizedException", "Invalid client or secret"), (
Issue-Key: cognito-idp.get-client-token-wrong-secret-rejected|run#10|AssertionError|assert code == ("NotAuthorizedException", "Invalid client or secret"), (
Issue-Key: cognito-idp.op-contract-get-client-token|run#14|AssertionError|assert code == ("NotAuthorizedException", "Invalid client or secret"), (
Issue-Key: cognito-idp.get-client-token-disallowed-scope-rejected|run#8|AssertionError|assert code == ("NotAuthorizedException", "Invalid scopes requested"), (
Issue-Key: cognito-idp.global-sign-out-invalidates-access-token|run#12|AssertionError|assert message1 == expected, f"expected message {expected!r}, got {message1!r}"
Issue-Key: cognito-idp.get-tokens-from-refresh-token-wrong-client-rejected|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.op-contract-get-tokens-from-refresh-token|run#25|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.auth-challenge-session-sequencing|run#15|AssertionError|assert code_replay_msg == 'Invalid session for the user, session can only be used once.', f"expected the exact {code_replay} message, got {code_replay_msg!r}"
Issue-Key: cognito-idp.op-contract-adminRespondToAuthChallenge|run#17|AssertionError|assert code.get("Message") == "Invalid session for the user, session can only be used once.", code.get("Message")
Issue-Key: cognito-idp.op-contract-admin-respond-to-auth-challenge|run#11|AssertionError|assert code == "CodeMismatchException", f"expected CodeMismatchException for bogus session, got {code}"
Issue-Key: cognito-idp.auth-flow-not-enabled-fails|run#16|AssertionError|assert msg == 'Auth flow not enabled for this client', f"expected the exact {code} message, got {msg!r}"
Issue-Key: cognito-idp.unconfirmed-user-auth-error-class-precision|run#19|AssertionError|assert wrong_code == "UserNotConfirmedException", (
Issue-Key: cognito-identity-provider|ForgotPassword|NotAuthorizedException
Issue-Key: cognito-idp.alias-exists-on-provider-link|run#12|AssertionError|assert code_msg == 'Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser', f"expected the
Issue-Key: cognito-idp.op-contract-adminLinkProviderForUser|run#15|AssertionError|assert code.get("Message") == "Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser", code
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: authentication refusals use the wrong error class or the wrong message, and PreventUserExistenceErrors is not honored (10 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Ten refusal paths on the authentication surface answer with a different error code or message than AWS. Two of them (causes 8 and 9) turn the error code into an oracle that leaks whether an account exists or whether a password was right, which is a security-relevant divergence, so the bundle is HIGH.

Cause 1 of 10 — token endpoint / GetClientToken with a wrong client secret.
AWS: NotAuthorizedException with the fixed text 'Invalid client or secret' (no trailing period, no mention of which half was wrong).
Emulator: NotAuthorizedException 'Invalid client secret.' — it both reveals that the client id was valid and adds a period.
Why: the message distinguishes 'bad client' from 'bad secret', which AWS deliberately does not.
Fix: the GetClientToken / client-credentials grant authentication path.
Traces: cognito-idp.get-client-token-invalid-secret-rejected, cognito-idp.get-client-token-wrong-secret-rejected, cognito-idp.op-contract-get-client-token.

Cause 2 of 10 — GetClientToken requesting a scope the client does not declare.
AWS: NotAuthorizedException (the grant is refused, not the request shape).
Emulator: InvalidParameterException 'Each scope must belong to a resource server in your user pool.' (400 instead of the 403 class).
Fix: scope checking in the client-credentials grant must refuse with NotAuthorizedException.
Traces: cognito-idp.get-client-token-disallowed-scope-rejected.

Cause 3 of 10 — using an access token after sign-out.
AWS: NotAuthorizedException 'Access Token has been revoked' (fixed text) after AdminUserGlobalSignOut/GlobalSignOut.
Emulator: NotAuthorizedException 'Invalid Access Token' — the same message it uses for a structurally bogus token, so a revoked token is indistinguishable from garbage.
Fix: the access-token validation path must distinguish revoked from invalid.
Traces: cognito-idp.global-sign-out-invalidates-access-token.

Cause 4 of 10 — refresh-token refusals.
AWS: GetTokensFromRefreshToken with a refresh token minted for a different client answers NotAuthorizedException 'Refresh Token has different Client'; a malformed/unknown refresh token answers 'Invalid Refresh Token' (no trailing period).
Emulator: both answer 'Invalid Refresh Token.' — the wrong-client case is collapsed into the generic one and a period is appended.
Fix: GetTokensFromRefreshToken token validation.
Traces: cognito-idp.get-tokens-from-refresh-token-wrong-client-rejected, cognito-idp.op-contract-get-tokens-from-refresh-token.

Cause 5 of 10 — challenge session validation message.
AWS: NotAuthorizedException with its own fixed text for a reused/out-of-order challenge Session.
Emulator: 'Invalid session for the user.'
Fix: the (Admin)RespondToAuthChallenge session validation path.
Traces: cognito-idp.auth-challenge-session-sequencing, cognito-idp.op-contract-adminRespondToAuthChallenge.

Cause 6 of 10 — AdminRespondToAuthChallenge with a bogus Session.
AWS: CodeMismatchException (Smithy lists CodeMismatchException, httpError 400, on RespondToAuthChallenge/AdminRespondToAuthChallenge).
Emulator: NotAuthorizedException.
Why: callers retry on CodeMismatch and abandon on NotAuthorized; the classes are not interchangeable.
Fix: AdminRespondToAuthChallenge challenge-response verification.
Traces: cognito-idp.op-contract-admin-respond-to-auth-challenge.

Cause 7 of 10 — auth flow not enabled for the client.
AWS: InvalidParameterException with AWS's exact text for a flow the app client does not declare.
Emulator: 'ADMIN_USER_PASSWORD_AUTH flow not enabled for this client'.
Fix: the (Admin)InitiateAuth ExplicitAuthFlows gate message.
Traces: cognito-idp.auth-flow-not-enabled-fails.

Cause 8 of 10 — UNCONFIRMED user signing in with the WRONG password.
AWS: UserNotConfirmedException — the account-status check precedes the credential check, so the code does not reveal whether the password was right.
Emulator: NotAuthorizedException, which makes the error code a password oracle on every unconfirmed account.
Fix: order the status check before credential verification in InitiateAuth/AdminInitiateAuth.
Traces: cognito-idp.unconfirmed-user-auth-error-class-precision.

Cause 9 of 10 — PreventUserExistenceErrors=ENABLED is ignored by ForgotPassword.
AWS: with PreventUserExistenceErrors=ENABLED, ForgotPassword for a nonexistent user responds like it would for a real user (a CodeDeliveryDetails response), never an existence-revealing error.
Emulator: NotAuthorizedException 'Username/client id combination not found.'
Why: user enumeration against the pool.
Fix: honor the app client's PreventUserExistenceErrors setting in ForgotPassword (and the shared user-lookup helper).
Traces: cognito-idp.forgot-password-nonexistent-user-no-error-leak, cognito-idp.prevent-user-existence-errors-masks-unknown-user.

Cause 10 of 10 — AdminLinkProviderForUser with a Cognito-native SourceUser.
AWS: InvalidParameterException 'Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser' (fixed text). Smithy AdminLinkProviderForUser lists InvalidParameterException and AliasExistsException.
Emulator: InvalidParameterException 'Provider <ProviderName> user <sub> is already linked to another user.' — it reports an unrelated reason; the related AdminDisableProviderForUser path likewise must answer 'SourceUser with the specified attribute value is already linked to another user'.
Fix: AdminLinkProviderForUser input classification.
Traces: cognito-idp.alias-exists-on-provider-link, cognito-idp.op-contract-adminLinkProviderForUser.

## Evidence
Verbatim from the replay:
- expected ('NotAuthorizedException','Invalid client or secret'), got ('NotAuthorizedException','Invalid client secret.')
- expected NotAuthorizedException for an undeclared scope, got ('InvalidParameterException','Each scope must belong to a resource server in your user pool.')
- expected 'Access Token has been revoked', got 'Invalid Access Token'
- expected 'Refresh Token has different Client', got 'Invalid Refresh Token.'; expected 'Invalid Refresh Token', got 'Invalid Refresh Token.'
- expected the exact NotAuthorizedException session message, got 'Invalid session for the user.'
- expected CodeMismatchException for a bogus session, got NotAuthorizedException
- expected the exact InvalidParameterException message, got 'ADMIN_USER_PASSWORD_AUTH flow not enabled for this client'
- 'InitiateAuth for an UNCONFIRMED user with the WRONG password must ALSO fail with UserNotConfirmedException ... got NotAuthorizedException'
- 'ForgotPassword for a nonexistent user leaked existence via error NotAuthorizedException' / uncaught NotAuthorizedException 'Username/client id combination not found.'
- expected 'Invalid SourceUser: Cognito users with a username/password may not be passed in as a SourceUser, only as a DestinationUser', got 'Provider Facebook user sim2real-behav-fb-ext-f14ff41fc8d8 is already linked to another user.'

## Steps to Reproduce
1. CreateUserPoolClient(GenerateSecret=true, AllowedOAuthFlows=['client_credentials']); call the token endpoint with a wrong secret -> compare code+message.
2. AdminCreateUser + AdminUserGlobalSignOut, then GetUser with the old access token -> compare message.
3. SignUp a user (leave UNCONFIRMED) and InitiateAuth with a wrong password -> expect UserNotConfirmedException.
4. CreateUserPoolClient(PreventUserExistenceErrors='ENABLED'), ForgotPassword for a username that does not exist -> expect a normal CodeDeliveryDetails response.
5. AdminLinkProviderForUser with SourceUser naming a Cognito username/password user -> compare message.

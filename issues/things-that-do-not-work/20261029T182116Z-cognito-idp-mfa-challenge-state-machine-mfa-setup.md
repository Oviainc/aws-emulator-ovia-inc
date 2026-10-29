Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.enable-software-token-mfa-precondition-error cognito-idp.mfa-setup-challenge-flow-software-token cognito-idp.mfa-on-without-method-fails cognito-idp.op-contract-createUserPool cognito-idp.op-contract-set-user-pool-mfa-config cognito-idp.admin-set-user-mfa-preference-conflicting-preferred-methods-rejected cognito-idp.admin-set-user-mfa-preference-conflicting-settings-rejected cognito-idp.admin-set-user-mfa-preference-cross-pool-user-not-found cognito-idp.admin-set-user-mfa-preference-cross-pool-user-not-found-rejected cognito-idp.admin-set-user-mfa-preference-invalid-parameter-rejected cognito-idp.admin-set-user-mfa-preference-method-not-enrolled-rejected cognito-idp.alias-exists-on-challenge-response cognito-idp.e2e-terms-configuration-does-not-block-signin cognito-idp.op-contract-respond-to-auth-challenge cognito-idp.op-contract-respond-to-auth-challenge-session-reuse-rejected cognito-idp.change-password-wrong-old-password-fails cognito-idp.delete-last-user-pool-client-secret-rejected cognito-idp.get-csv-header-nonexistent-pool-rejected cognito-idp.revoke-token-access-token-type-rejected cognito-idp.verify-software-token-without-associate-fails cognito-idp.user-srp-auth-wrong-password-and-unconfirmed-user-rejected cognito-idp.deep-cognito-idp-3-enforce-risk-ip-range-blocked cognito-idp.deep-cognito-idp-3-enforce-risk-user-context-ip-blocked cognito-idp.risk-configuration-set-and-describe-without-addon cognito-idp.secret-hash-required-for-confidential-client-auth cognito-idp.secret-hash-required-for-refresh-token-auth
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.enable-software-token-mfa-precondition-error|run#13|AssertionError|assert challenge.get("ChallengeName") == "MFA_SETUP", challenge.get("ChallengeName")
Issue-Key: cognito-idp.mfa-setup-challenge-flow-software-token|run#14|AssertionError|assert first.get("ChallengeName") == "MFA_SETUP", first.get("ChallengeName")
Issue-Key: cognito-idp.mfa-on-without-method-fails|_client_error#10|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.op-contract-createUserPool|run#12|AssertionError|assert raised is not None, "expected MfaConfiguration=ON with no SmsConfiguration to be rejected"
Issue-Key: cognito-idp.op-contract-set-user-pool-mfa-config|_error#13|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.admin-set-user-mfa-preference-conflicting-preferred-methods-rejected|run#7|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-idp.admin-set-user-mfa-preference-conflicting-settings-rejected|run#9|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-idp.admin-set-user-mfa-preference-cross-pool-user-not-found|run#6|AssertionError|assert code == "UserNotFoundException", f"expected UserNotFoundException, got {code}"
Issue-Key: cognito-idp.admin-set-user-mfa-preference-cross-pool-user-not-found-rejected|run#8|AssertionError|assert code == "UserNotFoundException", f"expected UserNotFoundException, got {code}"
Issue-Key: cognito-idp.admin-set-user-mfa-preference-invalid-parameter-rejected|run#7|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-idp.admin-set-user-mfa-preference-method-not-enrolled-rejected|run#15|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: cognito-identity-provider|RespondToAuthChallenge|NotImplemented
Issue-Key: cognito-idp.change-password-wrong-old-password-fails|run#13|AssertionError|assert code == "NotAuthorizedException", f"expected NotAuthorizedException, got {code}"
Issue-Key: cognito-idp.delete-last-user-pool-client-secret-rejected|run#9|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-idp.get-csv-header-nonexistent-pool-rejected|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: cognito-idp.revoke-token-access-token-type-rejected|run#14|AssertionError|assert code == "UnsupportedTokenTypeException", (
Issue-Key: cognito-idp.verify-software-token-without-associate-fails|run#8|AssertionError|assert code == "SoftwareTokenMFANotFoundException", (
Issue-Key: cognito-idp.user-srp-auth-wrong-password-and-unconfirmed-user-rejected|run#14|AssertionError|assert wrong_code == "NotAuthorizedException", (
Issue-Key: cognito-idp.deep-cognito-idp-3-enforce-risk-ip-range-blocked|run#9|AssertionError|assert blocked == "NotAuthorizedException", (
Issue-Key: cognito-idp.deep-cognito-idp-3-enforce-risk-user-context-ip-blocked|run#16|AssertionError|assert blocked == "NotAuthorizedException", (
Issue-Key: cognito-identity-provider|SetRiskConfiguration|UserPoolAddOnNotEnabledException
Issue-Key: cognito-idp.secret-hash-required-for-confidential-client-auth|_rejection#18|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.secret-hash-required-for-refresh-token-auth|_rejection#19|AssertionError|raise AssertionError(
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: MFA/challenge state machine — MFA_SETUP never issued, MfaConfiguration=ON accepted with no method, risk rules and SECRET_HASH unenforced, stubs answer NotImplemented in place of modeled errors (8 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Eight root causes on the MFA / adaptive-auth / challenge surface. Causes 6 and 8 are security-relevant (a blocked IP and a missing SECRET_HASH both authenticate successfully), so the bundle is HIGH.

Cause 1 of 8 — the MFA_SETUP challenge is never issued.
AWS: when the pool's MfaConfiguration is ON and the user has no enrolled factor, (Admin)InitiateAuth answers with ChallengeName='MFA_SETUP' and a Session, so the client can AssociateSoftwareToken.
Emulator: the response carries no ChallengeName at all (the check reads None).
Why: the whole TOTP enrollment-on-first-sign-in flow is unreachable.
Fix: the InitiateAuth/AdminInitiateAuth challenge selection must emit MFA_SETUP.
Traces: cognito-idp.enable-software-token-mfa-precondition-error, cognito-idp.mfa-setup-challenge-flow-software-token.

Cause 2 of 8 — MfaConfiguration=ON is accepted with no delivery method configured.
AWS: SetUserPoolMfaConfig / CreateUserPool with MfaConfiguration='ON' and neither SmsMfaConfiguration, SoftwareTokenMfaConfiguration nor EmailMfaConfiguration is refused with InvalidParameterException (Smithy SetUserPoolMfaConfig lists InvalidParameterException, httpError 400).
Emulator: the call succeeds and returns MfaConfiguration='ON'.
Why: a pool is left in a configuration AWS cannot produce, where MFA is required but no factor can be delivered.
Fix: SetUserPoolMfaConfig / CreateUserPool MFA validation.
Traces: cognito-idp.mfa-on-without-method-fails, cognito-idp.op-contract-createUserPool, cognito-idp.op-contract-set-user-pool-mfa-config.

Cause 3 of 8 — AdminSetUserMFAPreference answers NotImplemented instead of its modeled errors.
AWS: InvalidParameterException for conflicting preferred methods / conflicting settings / an un-enrolled factor, and UserNotFoundException for a user that is not in the named pool.
Emulator: 'NotImplemented ... operation AdminSetUserMFAPreference is scaffolded but not implemented in this emulator' for every one of them.
Why: callers see an unmodeled error code on six distinct refusal classes.
Fix: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_set_user_mfa_preference.rs. The scaffold issue the suite filed for this operation names only one blocked check; these six are its unmarked siblings and must be re-replayed with it.
Traces: cognito-idp.admin-set-user-mfa-preference-conflicting-preferred-methods-rejected, cognito-idp.admin-set-user-mfa-preference-conflicting-settings-rejected, cognito-idp.admin-set-user-mfa-preference-cross-pool-user-not-found, cognito-idp.admin-set-user-mfa-preference-cross-pool-user-not-found-rejected, cognito-idp.admin-set-user-mfa-preference-invalid-parameter-rejected, cognito-idp.admin-set-user-mfa-preference-method-not-enrolled-rejected.

Cause 4 of 8 — RespondToAuthChallenge(ChallengeName=NEW_PASSWORD_REQUIRED) is a stub.
AWS: NEW_PASSWORD_REQUIRED is a modeled ChallengeNameType value; responding to it completes authentication and returns AuthenticationResult (Smithy RespondToAuthChallenge output: AuthenticationResult / ChallengeName / Session).
Emulator: 'NotImplemented ... operation RespondToAuthChallenge with ChallengeName=NEW_PASSWORD_REQUIRED is scaffolded but not implemented'.
Why: every FORCE_CHANGE_PASSWORD user created by AdminCreateUser is permanently unable to sign in.
Fix: the RespondToAuthChallenge handler's NEW_PASSWORD_REQUIRED branch.
Traces: cognito-idp.alias-exists-on-challenge-response, cognito-idp.e2e-terms-configuration-does-not-block-signin, cognito-idp.op-contract-respond-to-auth-challenge, cognito-idp.op-contract-respond-to-auth-challenge-session-reuse-rejected.

Cause 5 of 8 — other scaffolded operations answer NotImplemented where a modeled error is required.
AWS: ChangePassword -> NotAuthorizedException for a wrong old password; DeleteUserPoolClientSecret -> InvalidParameterException when deleting the last secret; GetCSVHeader -> ResourceNotFoundException for a nonexistent pool; RevokeToken -> UnsupportedTokenTypeException for an access token; VerifySoftwareToken -> SoftwareTokenMFANotFoundException when no token was associated; InitiateAuth(USER_SRP_AUTH) -> NotAuthorizedException for a wrong password.
Emulator: 'NotImplemented ... is scaffolded but not implemented in this emulator' in all six.
Why: an unmodeled code on a negative path the SDK is expected to classify.
Fix: the respective src/ops/*.rs handlers (change_password.rs, delete_user_pool_client_secret.rs, get_csv_header.rs, revoke_token.rs, the software-token ops, and the USER_SRP_AUTH branch of initiate_auth). Scaffold issues exist for those operations; these checks are the unmarked siblings that must be re-replayed with the fix.
Traces: cognito-idp.change-password-wrong-old-password-fails, cognito-idp.delete-last-user-pool-client-secret-rejected, cognito-idp.get-csv-header-nonexistent-pool-rejected, cognito-idp.revoke-token-access-token-type-rejected, cognito-idp.verify-software-token-without-associate-fails, cognito-idp.user-srp-auth-wrong-password-and-unconfirmed-user-rejected.

Cause 6 of 8 — risk configuration (blocked IP ranges) is not enforced.
AWS: with an ENFORCED AccountTakeoverRiskConfiguration / RiskExceptionConfiguration blocking an IP range, a sign-in from that range is refused with NotAuthorizedException — both when the IP is the caller's and when it arrives in UserContextData from a client with EnablePropagateAdditionalUserContextData.
Emulator: the correct password from the blocked range returns OK.
Why: adaptive-auth controls are inert, so a scenario that must be denied is allowed.
Fix: the risk evaluation step of InitiateAuth/AdminInitiateAuth against the stored risk configuration.
Traces: cognito-idp.deep-cognito-idp-3-enforce-risk-ip-range-blocked, cognito-idp.deep-cognito-idp-3-enforce-risk-user-context-ip-blocked.

Cause 7 of 8 — SetRiskConfiguration is refused when the pool has no advanced-security add-on.
AWS: the configuration is accepted and stored even without the add-on (it simply is never evaluated); DescribeRiskConfiguration then reflects it. UserPoolAddOnNotEnabledException is modeled but not used for this input.
Emulator: UserPoolAddOnNotEnabledException 'User pool add-ons is not enabled.' (400).
Why: callers cannot pre-provision risk configuration.
Fix: remove the add-on precondition from the SetRiskConfiguration handler.
Traces: cognito-idp.risk-configuration-set-and-describe-without-addon.

Cause 8 of 8 — SECRET_HASH is not required for a confidential client.
AWS: a client created with GenerateSecret=true must supply SECRET_HASH in AuthParameters; omitting it is NotAuthorizedException, for both the password flows and REFRESH_TOKEN_AUTH.
Emulator: authentication SUCCEEDS and returns a full AuthenticationResult without SECRET_HASH.
Why: the client secret is not actually a credential in the emulator — tests that assert a confidential client is enforced pass against the emulator and fail against AWS.
Fix: SECRET_HASH verification in the (Admin)InitiateAuth / RespondToAuthChallenge credential path.
Traces: cognito-idp.secret-hash-required-for-confidential-client-auth, cognito-idp.secret-hash-required-for-refresh-token-auth.

## Evidence
Verbatim from the replay:
- assert challenge.get('ChallengeName') == 'MFA_SETUP' -> AssertionError: None (twice)
- SetUserPoolMfaConfig: 'expected this call to be rejected, but it SUCCEEDED and returned {MfaConfiguration: ON}'
- 'expected InvalidParameterException, got NotImplemented' / 'expected UserNotFoundException, got NotImplemented' (AdminSetUserMFAPreference x6)
- ClientError (NotImplemented) when calling RespondToAuthChallenge: 'operation RespondToAuthChallenge with ChallengeName=NEW_PASSWORD_REQUIRED is scaffolded but not implemented in this emulator'
- 'expected NotAuthorizedException, got NotImplemented' (ChangePassword), 'expected ResourceNotFoundException, got NotImplemented' (GetCSVHeader), 'expected UnsupportedTokenTypeException, got NotImplemented' (RevokeToken), 'expected SoftwareTokenMFANotFoundException, got NotImplemented', 'USER_SRP_AUTH completed from the WRONG password must fail with NotAuthorizedException, got NotImplemented'
- 'ENFORCED pool: correct password from blocked-range IP 203.0.113.77 returned OK, expected NotAuthorizedException'
- uncaught UserPoolAddOnNotEnabledException: 'User pool add-ons is not enabled.' on SetRiskConfiguration
- 'expected NotAuthorizedException, but the call SUCCEEDED and returned {AuthenticationResult: ...}' (no SECRET_HASH)

## Steps to Reproduce
1. CreateUserPool + SetUserPoolMfaConfig(MfaConfiguration='ON') with no Sms/SoftwareToken/Email config -> expect InvalidParameterException.
2. With MfaConfiguration='ON' and a user with no enrolled factor, InitiateAuth(USER_PASSWORD_AUTH) -> expect ChallengeName='MFA_SETUP'.
3. AdminSetUserMFAPreference with two preferred methods -> expect InvalidParameterException.
4. AdminCreateUser (FORCE_CHANGE_PASSWORD) then AdminInitiateAuth + RespondToAuthChallenge(NEW_PASSWORD_REQUIRED) -> expect AuthenticationResult.
5. SetRiskConfiguration on a pool without advanced security -> expect success, then DescribeRiskConfiguration.
6. CreateUserPoolClient(GenerateSecret=true) and InitiateAuth without SECRET_HASH -> expect NotAuthorizedException.

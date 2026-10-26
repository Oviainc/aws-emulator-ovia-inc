Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.set-user-mfa-preference-conflicting-preferred-method-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.set-user-mfa-preference-conflicting-preferred-method-rejected|run#6|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Commit: 9d990885182cf7376ea90557ef044b425999cd93

# cognito-idp: SetUserMFAPreference answers 501 NotImplemented instead of refusing a preferred TOTP factor with InvalidParameterException when no TOTP authenticator is registered

> _Note: this issue was identified against the code base at commit `9d990885182cf7376ea90557ef044b425999cd93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetUserMFAPreference

## Description
One root cause, grounded in the single failing check the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 1 — SetUserMFAPreference never evaluates the software-token registration precondition; the whole operation answers the unmodelled error code 'NotImplemented'.

AWS: Smithy (cognito-idp) SetUserMFAPreference takes a required AccessToken (TokenModelType, pattern ^[A-Za-z0-9-_=.]+$, sensitive) plus the optional SMSMfaSettings, SoftwareTokenMfaSettings, EmailMfaSettings and WebAuthnMfaSettings structures, and returns an empty output (SetUserMFAPreferenceResponse). Its modeled error set is exactly ForbiddenException (403), InternalErrorException (500), InvalidParameterException (400), NotAuthorizedException (403), OperationNotEnabledException (400), PasswordResetRequiredException (400), ResourceNotFoundException (404), UserNotConfirmedException (400) and UserNotFoundException (404). 'NotImplemented' is NOT a member of that set. The model documents the precondition under SetUserMFAPreference.SoftwareTokenMfaSettings: "Users must register a TOTP authenticator before they set this as their preferred MFA method." So when SoftwareTokenMfaSettings={Enabled:true, PreferredMfa:true} is sent for a user who has never completed AssociateSoftwareToken + VerifySoftwareToken, AWS refuses the call with HTTP 400 InvalidParameterException; the recorded message is the fixed text "User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA" (this message class has NO variable parts — it is constant text, not a template filled from the request). The refusal has NO effect on state: the user's PreferredMfaSetting stays absent and SOFTWARE_TOKEN_MFA does not appear in UserMFASettingList. The precondition is a state precondition, not an unfixably-bad input: once the same user completes AssociateSoftwareToken and VerifySoftwareToken (Status SUCCESS), the IDENTICAL SetUserMFAPreference request succeeds (200, empty body) and GetUserAuthFactors then reports PreferredMfaSetting == "SOFTWARE_TOKEN_MFA" with SOFTWARE_TOKEN_MFA in UserMFASettingList.

Emulator: every SetUserMFAPreference request — valid or invalid — is refused with HTTP 501 and error code 'NotImplemented', message of the form "operation SetUserMFAPreference is scaffolded but not implemented in this emulator". The handler binds its input and immediately returns the scaffold's not-implemented error, so neither the TOTP-registration precondition nor the success path exists, and no per-user MFA preference state is ever written or read back.

Why: a client gets a non-2xx with a code no SDK error handler models, so it cannot distinguish "you must register a TOTP authenticator first" from "this server cannot answer". Every MFA-preference flow is broken in both directions: the negative path (selecting TOTP as preferred before registration) cannot be detected and retried after registration, and the positive path (setting a preference at all, then observing it via GetUserAuthFactors) can never be reached. Because the status is 501 rather than 400, SDK retry policies treat it as a server fault and may retry a request AWS would have rejected outright.

Related already-open issue, which does NOT cover this check: 20261024T114943Z-cognito-idp-five-modeled-error-paths-answer-notimplemented.md, whose Cause 5 names SetUserMFAPreference but is grounded only in the SMS/verified-phone precondition (cognito-idp.set-user-mfa-preference-sms-without-verified-phone-rejected) and carries only that replay key in Issue-Trace-Ids. The precondition here is a different one (software-token registration, with a different error message and a different restore sequence), and this check's replay key appears in no open issue, so a fixer could satisfy that issue's SMS guard and close it while this check still fails.

Fix: the SetUserMFAPreference handler (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_user_mfa_preference.rs) — replace the scaffolded not-implemented return with real behaviour: validate the access token, then gate SoftwareTokenMfaSettings.PreferredMfa on the user actually having a verified software token (the state AssociateSoftwareToken/VerifySoftwareToken write), returning InvalidParameterException with the fixed message above and leaving the user's MFA state untouched; otherwise persist the per-user MFA settings and preferred factor into this service's user state so GetUserAuthFactors reflects it. Remove the '//! SCAFFOLDED, NOT IMPLEMENTED:' marker line once implemented.

Traces: cognito-idp.set-user-mfa-preference-conflicting-preferred-method-rejected.

## Evidence
Failing check cognito-idp.set-user-mfa-preference-conflicting-preferred-method-rejected ([mismatch]):

  File "check_cognito_idp_set_user_mfa_preference_conflicting_preferred_method_rejected.py", line 44, in run
    assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
  AssertionError: expected InvalidParameterException, got NotImplemented

Expected (AWS): HTTP 400, error code InvalidParameterException, message "User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA" (entirely fixed text for this error class — no request- or state-derived parts). No state change: GetUserAuthFactors still reports PreferredMfaSetting absent and SOFTWARE_TOKEN_MFA not in UserMFASettingList. After AssociateSoftwareToken + VerifySoftwareToken the identical request returns 200 and GetUserAuthFactors reports PreferredMfaSetting == "SOFTWARE_TOKEN_MFA".

Actual (emulator): HTTP 501, error code NotImplemented, message "operation SetUserMFAPreference is scaffolded but not implemented in this emulator". Source: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_user_mfa_preference.rs line 4 ('SCAFFOLDED, NOT IMPLEMENTED' marker) and line 204 (Err(crate::scaffolded::Scaffolded::not_implemented("SetUserMFAPreference"))).

Model citation: Smithy SetUserMFAPreference errors = ForbiddenException(403), InternalErrorException(500), InvalidParameterException(400), NotAuthorizedException(403), OperationNotEnabledException(400), PasswordResetRequiredException(400), ResourceNotFoundException(404), UserNotConfirmedException(400), UserNotFoundException(404) — 'NotImplemented' is not modeled. Smithy SetUserMFAPreference.SoftwareTokenMfaSettings doc: "Users must register a TOTP authenticator before they set this as their preferred MFA method."

## Steps to Reproduce
1. CreateUserPool(PoolName=<any>).
2. SetUserPoolMfaConfig(UserPoolId, SoftwareTokenMfaConfiguration={Enabled: true}, MfaConfiguration="OPTIONAL") — so the pool itself supports software-token MFA and the refusal under test is about the USER never registering a token.
3. CreateUserPoolClient(UserPoolId, ExplicitAuthFlows=["ALLOW_ADMIN_USER_PASSWORD_AUTH","ALLOW_REFRESH_TOKEN_AUTH"], GenerateSecret=false).
4. AdminCreateUser(UserPoolId, Username, TemporaryPassword, MessageAction="SUPPRESS", UserAttributes=[email, email_verified=true]), then AdminSetUserPassword(..., Permanent=true).
5. AdminInitiateAuth(AuthFlow="ADMIN_USER_PASSWORD_AUTH") and take AuthenticationResult.AccessToken.
6. Call SetUserMFAPreference(AccessToken, SoftwareTokenMfaSettings={"Enabled": true, "PreferredMfa": true}) WITHOUT ever calling AssociateSoftwareToken/VerifySoftwareToken.
   -> Expect HTTP 400 InvalidParameterException with message "User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA"; observe HTTP 501 NotImplemented.
7. GetUserAuthFactors(AccessToken) -> expect PreferredMfaSetting absent and SOFTWARE_TOKEN_MFA not in UserMFASettingList (the rejected call must have had no effect).
8. Restore the precondition: AssociateSoftwareToken(AccessToken) -> SecretCode; VerifySoftwareToken(AccessToken, UserCode=<RFC 6238 SHA1/6-digit/30s TOTP of SecretCode>) until Status==SUCCESS.
9. Repeat the IDENTICAL call from step 6 -> expect 200, then GetUserAuthFactors(AccessToken) reports PreferredMfaSetting == "SOFTWARE_TOKEN_MFA" and SOFTWARE_TOKEN_MFA in UserMFASettingList.

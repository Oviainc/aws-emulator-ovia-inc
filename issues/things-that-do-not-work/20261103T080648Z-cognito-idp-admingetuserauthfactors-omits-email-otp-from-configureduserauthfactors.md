Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-user-signup-confirm-and-mfa
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-user-signup-confirm-and-mfa
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminGetUserAuthFactors|2xx->2xx|$.ConfiguredUserAuthFactors[*]
Issue-Commit: 1c0413a0db44f6903ae94b4a2eeb83f1af34b87e

# cognito-idp: AdminGetUserAuthFactors omits EMAIL_OTP from ConfiguredUserAuthFactors

> _Note: this issue was identified against the code base at commit `1c0413a0db44f6903ae94b4a2eeb83f1af34b87e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminGetUserAuthFactors

## Description
Cause 1 of 1: AdminGetUserAuthFactors under-reports the user's available USER_AUTH authentication factors — it returns only the password factor and never reports the email one-time-password factor, even after the user's email address has been confirmed/verified and email OTP is available on the pool.

AWS: Smithy model services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice — AdminGetUserAuthFactors(input UserPoolId, Username; output Username, PreferredMfaSetting, UserMFASettingList, ConfiguredUserAuthFactors). ConfiguredUserAuthFactors targets ConfiguredUserAuthFactorsListType (list of AuthFactorType, @length min 0 max 8); AuthFactorType enum values are PASSWORD, EMAIL_OTP, SMS_OTP, WEB_AUTHN, SOFTWARE_TOKEN. The member is documented as "The authentication types that are available to the user with USER_AUTH sign-in, for example [\"PASSWORD\", \"WEB_AUTHN\"]" and states that "EMAIL_OTP, SMS_OTP, and WEB_AUTHN can be used as either a first authentication factor or an MFA factor". So the list is the complete set of factors the user can actually present, not just the password factor: a user with a verified email usable for email OTP must have EMAIL_OTP enumerated alongside PASSWORD. Status 200, no error involved.

Emulator: for the same user, after the admin sign-up/confirm + MFA setup sequence, AdminGetUserAuthFactors answers 200 but ConfiguredUserAuthFactors carries only the single password entry — the second element the recording asserts is absent. Replay detail: "$.ConfiguredUserAuthFactors[1]: missing (expected 'EMAIL_OTP', got None)" (expected status 200, actual status 200). No other member of the response diverges, so this is purely the factor enumeration, not the overall shape.

Why: a client that uses AdminGetUserAuthFactors to decide which USER_AUTH factors to offer (choice-based sign-in UIs, admin consoles, support tooling that reports a user's recoverable sign-in methods) concludes the user can only sign in with a password. It will not offer the email OTP path, will not treat the user as having a second factor configured, and any assertion that a confirmed-email user exposes EMAIL_OTP fails — while an InitiateAuth USER_AUTH attempt with EMAIL_OTP may still be accepted, leaving the two surfaces mutually inconsistent.

Fix: AdminGetUserAuthFactors' handler/serializer must build ConfiguredUserAuthFactors from the user's actual factor state rather than from a hard-coded password-only list: derive it from the user's verified attributes and MFA settings plus the pool's enabled factors — PASSWORD always, EMAIL_OTP when the user's email is verified and email OTP is available, SMS_OTP when the phone number is verified and SMS is available, SOFTWARE_TOKEN when a software token is registered, WEB_AUTHN when a passkey is registered and passkey MFA is enabled at the pool level — emitting them as AuthFactorType enum values within the modeled 0..8 length bound. The same derivation should back GetUserAuthFactors so admin and self-service reads agree.

Note: no existing open cognito-idp issue covers this read path; the nearest open issues are 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md (MFA/challenge state machine) and 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md (other read-path omissions), and the replay report attributes this cluster to neither.

Traces: cognito-idp-admin-user-signup-confirm-and-mfa#task#9 (the only member of the cluster).

## Evidence
Fixture cognito-idp-admin-user-signup-confirm-and-mfa, step task#9, operation AdminGetUserAuthFactors (service cognito-idp), verdict mismatch.

Expected (recorded AWS response): HTTP 200 with ConfiguredUserAuthFactors containing at least two entries, the second being the AuthFactorType value EMAIL_OTP.
Actual (emulator): HTTP 200 with ConfiguredUserAuthFactors containing only the first (password) entry; index 1 absent.
Replay detail, verbatim: "$.ConfiguredUserAuthFactors[1]: missing (expected 'EMAIL_OTP', got None)".
divergence_key: service=cognito-idp|AdminGetUserAuthFactors|status=200->200|keys=$.ConfiguredUserAuthFactors[1]
affected_count: 1 (cognito-idp-admin-user-signup-confirm-and-mfa#task#9)

Model evidence: AuthFactorType enum = [PASSWORD, EMAIL_OTP, SMS_OTP, WEB_AUTHN, SOFTWARE_TOKEN]; ConfiguredUserAuthFactorsListType @length min 0 max 8; member doc: "The authentication types that are available to the user with USER_AUTH sign-in" and "EMAIL_OTP, SMS_OTP, and WEB_AUTHN can be used as either a first authentication factor or an MFA factor."

## Steps to Reproduce
1. Replay wire fixture cognito-idp-admin-user-signup-confirm-and-mfa against the emulator and stop at step task#9.
2. Equivalently, by hand: CreateUserPool with email as a usable sign-in/verification attribute and an app client, create a user and confirm the account so the email attribute is verified, then complete the MFA/email-OTP setup the fixture performs.
3. Call AdminGetUserAuthFactors with that pool's UserPoolId and the user's Username.
4. Observe: response is 200 but ConfiguredUserAuthFactors lists only the password factor. Expected: the list also contains EMAIL_OTP (recorded AWS response has it at index 1), i.e. every factor the user can present for USER_AUTH sign-in is enumerated.

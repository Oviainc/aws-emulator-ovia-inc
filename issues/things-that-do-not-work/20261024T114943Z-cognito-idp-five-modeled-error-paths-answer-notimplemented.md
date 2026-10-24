Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.get-ui-customization-cross-pool-access-denied-rejected cognito-idp.get-ui-customization-cross-pool-access-denied cognito-idp.list-webauthn-credentials-without-access-token-denied cognito-idp.op-contract-update-auth-event-feedback cognito-idp.invalid-user-pool-configuration-device-ops cognito-idp.set-user-mfa-preference-sms-without-verified-phone-rejected cognito-idp.group-precedence-ties-suppress-the-preferred-role-claim
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.get-ui-customization-cross-pool-access-denied-rejected|run#20|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: cognito-idp.get-ui-customization-cross-pool-access-denied|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: cognito-idp.list-webauthn-credentials-without-access-token-denied|run#12|AssertionError|assert code == "NotAuthorizedException", f"expected NotAuthorizedException, got {code}"
Issue-Key: cognito-idp.op-contract-update-auth-event-feedback|run#9|AssertionError|assert code == "UserPoolAddOnNotEnabledException", code
Issue-Key: cognito-idp.invalid-user-pool-configuration-device-ops|run#10|AssertionError|assert code1 == "InvalidUserPoolConfigurationException", (
Issue-Key: cognito-idp.set-user-mfa-preference-sms-without-verified-phone-rejected|run#11|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-idp.group-precedence-ties-suppress-the-preferred-role-claim|_poll_id_claims#21|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# cognito-idp: five modeled error paths answer NotImplemented instead of the modeled exception, and a group-precedence check hangs to the 60 s cap (6 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUICustomization

## Description
Six root causes, all grounded in failing checks the replay report marked neither ALREADY FILED nor OWNED. Five are operations whose refusal path is reached but answered with the unmodelled error code 'NotImplemented' instead of the exception the model declares; these are separate operations from the per-operation stub issues already filed on this service (SetUserPoolMfaConfig, SetUICustomization, ListDevices, GetUserAttributeVerificationCode, ListUsers, …), none of which name the handlers below.

Cause 1 of 6 — GetUICustomization answers NotImplemented.
AWS: Smithy (cognito-idp) GetUICustomization takes UserPoolId (+ optional ClientId) and models InvalidParameterException (400), NotAuthorizedException (403), OperationNotEnabledException (400), ResourceNotFoundException (404), TooManyRequestsException (429) and InternalErrorException (500); 'NotImplemented' is not among them. Recorded behavior: a caller whose IAM policy grants access only to a DIFFERENT pool's ARN is refused with AccessDeniedException (403) before the pool's customization is read.
Emulator: error code 'NotImplemented'. The sibling check that polls this same operation never converges and is killed at the harness's 60 s cap instead of failing fast.
Why: an authorization refusal is reported as a server-side gap, so a client cannot tell 'you may not read this pool' from 'this emulator cannot answer', and no negative authorization test can pass.
Fix: the GetUICustomization handler and the per-pool UI-customization state it reads, then the IAM-resource-scope evaluation that produces AccessDeniedException.
Traces: cognito-idp.get-ui-customization-cross-pool-access-denied-rejected, cognito-idp.get-ui-customization-cross-pool-access-denied.

Cause 2 of 6 — ListWebAuthnCredentials answers NotImplemented.
AWS: Smithy ListWebAuthnCredentials requires AccessToken (pattern ^[A-Za-z0-9-_=.]+$) and models NotAuthorizedException (403) among ForbiddenException (403), InvalidParameterException (400), LimitExceededException, OperationNotEnabledException, PasswordResetRequiredException and TooManyRequestsException. An invalid/garbage access token is NotAuthorizedException — not ForbiddenException (reserved for WAF rejections) and not an empty success.
Emulator: error code 'NotImplemented'.
Why: a passkey-enabled client cannot list credentials at all, and the invalid-token path — the one that must never be mistaken for 'this user has no credentials' — returns a code no SDK error handler models.
Fix: the ListWebAuthnCredentials handler, including access-token validation ahead of the listing.
Traces: cognito-idp.list-webauthn-credentials-without-access-token-denied.

Cause 3 of 6 — UpdateAuthEventFeedback answers NotImplemented.
AWS: Smithy UpdateAuthEventFeedback models UserPoolAddOnNotEnabledException (400) alongside InvalidParameterException, NotAuthorizedException, ResourceNotFoundException, UserNotFoundException and TooManyRequestsException. On a pool with no advanced-security (threat-protection) add-on the call is refused with UserPoolAddOnNotEnabledException before the user, event or FeedbackToken is considered.
Emulator: error code 'NotImplemented'.
Why: the documented 'this pool has no threat protection' boundary is unreachable, so a caller cannot discover that it must enable the add-on; the only other path (a real encrypted FeedbackToken) is unobtainable, making this the operation's whole observable contract.
Fix: the UpdateAuthEventFeedback handler and its add-on/feedback-token precondition checks.
Traces: cognito-idp.op-contract-update-auth-event-feedback.

Cause 4 of 6 — AdminListDevices answers NotImplemented.
AWS: Smithy AdminListDevices takes UserPoolId + Username and models InvalidUserPoolConfigurationException (400) explicitly, plus InvalidParameterException, NotAuthorizedException, ResourceNotFoundException and TooManyRequestsException. A device operation against a pool that has no DeviceConfiguration is refused with InvalidUserPoolConfigurationException.
Emulator: error code 'NotImplemented'. (The separately filed ListDevices stub issue names only the self-service operation, not this admin one.)
Why: the device surface has no diagnosable failure mode — a pool that was never configured for device remembering and a pool the emulator simply cannot serve produce the same unmodelled answer.
Fix: the AdminListDevices handler and the pool-configuration precondition shared with the other Admin*Device operations.
Traces: cognito-idp.invalid-user-pool-configuration-device-ops.

Cause 5 of 6 — SetUserMFAPreference answers NotImplemented.
AWS: Smithy SetUserMFAPreference requires AccessToken and models InvalidParameterException (400) among ForbiddenException, NotAuthorizedException, PasswordResetRequiredException, ResourceNotFoundException, UserNotConfirmedException and UserNotFoundException. Enabling SMSMfaSettings for a user with no verified phone_number is refused with InvalidParameterException.
Emulator: error code 'NotImplemented'. (This is a different operation from SetUserPoolMfaConfig, whose stub issue is already filed.)
Why: a user can neither set an MFA preference nor learn why one is invalid; the guard that stops SMS MFA being selected without a deliverable phone number is absent and unmodelled.
Fix: the SetUserMFAPreference handler, including the verified-attribute precondition for SMS/email MFA settings.
Traces: cognito-idp.set-user-mfa-preference-sms-without-verified-phone-rejected.

Cause 6 of 6 — the group-precedence / preferred-role check never converges and is killed at the harness's 60 s cap.
AWS: recorded behavior only — when two groups a user belongs to carry the SAME Precedence, Cognito omits the cognito:preferred_role claim from the issued token (the tie is unresolvable), while distinct precedences select the winning group's role; the polled sign-in and token-read calls answer promptly.
Emulator: 'Error: Check timed out after 60.0s' with no further diagnosis — the polled operation never produces the state the check waits for, so the refusal is absorbed by the retry loop rather than raised. Hypothesis: the same inability to vend/inspect group-role claims that open issue 20261023T185902Z-cognito-identity-cognito-idp-both-group-role-mapping.md records for the two cognito-identity-side checks; that issue carries neither this check's replay key nor this service token, so a fixer could close it without re-running this one.
Why: a client built on precedence-ordered group roles cannot observe which role was selected, and the tie case — the one that must yield NO preferred role — cannot be distinguished from a hang.
Fix: group/precedence resolution in the token-minting path of the auth handlers (the cognito:groups / cognito:preferred_role claims), and failing fast rather than silently not converging.
Traces: cognito-idp.group-precedence-ties-suppress-the-preferred-role-claim.

## Evidence
Cause 1: AssertionError: expected AccessDeniedException, got NotImplemented (cognito-idp.get-ui-customization-cross-pool-access-denied-rejected); the sibling cognito-idp.get-ui-customization-cross-pool-access-denied reports 'Error: Check timed out after 60.0s'.
Cause 2: AssertionError: expected NotAuthorizedException, got NotImplemented.
Cause 3: AssertionError: NotImplemented (assert code == 'UserPoolAddOnNotEnabledException').
Cause 4: AssertionError: AdminListDevices: expected InvalidUserPoolConfigurationException, got NotImplemented.
Cause 5: AssertionError: expected InvalidParameterException, got NotImplemented.
Cause 6: Error: Check timed out after 60.0s.
In every case the emulator answers the error code 'NotImplemented', which is not a member of the operation's modeled error set, where a 400/403 modeled exception is required.

## Steps to Reproduce
Cause 1: CreateUserPool (two pools), assume a role whose policy allows cognito-idp:GetUICustomization only on pool A's ARN, then GetUICustomization on pool B -> expect AccessDeniedException (403), observe NotImplemented.
Cause 2: CreateUserPool, then ListWebAuthnCredentials(AccessToken='garbage') -> expect NotAuthorizedException, observe NotImplemented.
Cause 3: CreateUserPool with no UserPoolAddOns, then UpdateAuthEventFeedback(UserPoolId, Username, EventId, FeedbackToken='x', FeedbackValue='valid') -> expect UserPoolAddOnNotEnabledException, observe NotImplemented.
Cause 4: CreateUserPool with NO DeviceConfiguration, AdminCreateUser, then AdminListDevices -> expect InvalidUserPoolConfigurationException, observe NotImplemented.
Cause 5: CreateUserPool, create and sign in a user with no verified phone_number, then SetUserMFAPreference(AccessToken, SMSMfaSettings={'Enabled': True, 'PreferredMfa': True}) -> expect InvalidParameterException, observe NotImplemented.
Cause 6: CreateUserPool, two groups with EQUAL Precedence both carrying RoleArns, add one user to both, sign in and decode the id token -> expect no cognito:preferred_role claim; observe the check hang to 60 s.

Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-account-recovery-and-attribute-verification
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-account-recovery-and-attribute-verification cognito-idp-self-service-signup-confirm-and-forgot-password cognito-idp-self-service-profile-view-update-and-attribute-verification cognito-idp-attribute-management-edge-cases-and-invalid-input cognito-idp-self-service-signup-and-token-session cognito-idp-self-service-settings-and-refresh
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|ForgotPassword|2xx->2xx|$.CodeDeliveryDetails.Destination
Issue-Key: cognito-idp|GetUserAttributeVerificationCode|2xx->2xx|$.CodeDeliveryDetails.Destination
Issue-Key: cognito-idp|UpdateUserAttributes|2xx->2xx|$.CodeDeliveryDetailsList[*].Destination
Issue-Key: cognito-idp|SignUp|2xx->2xx|$.CodeDeliveryDetails.Destination
Issue-Key: cognito-idp|ResendConfirmationCode|2xx->2xx|$.CodeDeliveryDetails.Destination
Issue-Key: cognito-idp|UpdateUserAttributes|2xx->2xx|$
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: code-delivery Destination masking keeps the domain suffix

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ForgotPassword

## Description
AWS: CodeDeliveryDetailsType.Destination is a masked rendering of the email address the code went to. For an EMAIL medium the recorded form is `<first-char-of-local>***@<first-char-of-domain>***` — the mask replaces the remainder of the local part AND the whole remainder of the domain, including the TLD. The fixed text is the two `***` runs and the `@`; the variable parts are the first character of the local part and the first character of the domain label, taken from the user's email attribute.
Emulator: the emulator masks the local part and the domain label but keeps the TLD, producing `<x>***@<y>***.com`.
Why: masking exists so a client can echo the destination to a user without disclosing the address. Keeping the TLD discloses more than AWS does, and any client that renders or compares the masked string against an expected template disagrees with AWS on every email destination.
Fix: the shared code-delivery projection used by SignUp, ResendConfirmationCode, ForgotPassword, GetUserAttributeVerificationCode and UpdateUserAttributes — mask everything after the first character of the domain, not just the domain's first label.

Cause 1 of 6: ForgotPassword returns `s***@e***.com` where AWS returns `s***@e***`. Fixture cognito-idp-account-recovery-and-attribute-verification, step task#5.
Cause 2 of 6: GetUserAttributeVerificationCode returns the same over-long mask. Fixture cognito-idp-account-recovery-and-attribute-verification, step task#8.
Cause 3 of 6: UpdateUserAttributes returns the same over-long mask in CodeDeliveryDetailsList[0].Destination. Fixture cognito-idp-attribute-management-edge-cases-and-invalid-input, step task#6.
Cause 4 of 6: SignUp returns the same over-long mask. Fixture cognito-idp-self-service-signup-and-token-session, step task#3.
Cause 5 of 6: ResendConfirmationCode returns the same over-long mask. Fixture cognito-idp-self-service-signup-and-token-session, step task#4.
Cause 6 of 6: UpdateUserAttributes that triggers no verification returns `{"CodeDeliveryDetailsList": []}`; AWS returns an empty object `{}` and omits the list entirely when nothing was delivered. Fixture cognito-idp-self-service-settings-and-refresh, step task#9.

## Evidence
ForgotPassword (cognito-idp-account-recovery-and-attribute-verification#task#5, 200 both sides): $.CodeDeliveryDetails.Destination: expected 's***@e***', got 's***@e***.com'.
UpdateUserAttributes (cognito-idp-attribute-management-edge-cases-and-invalid-input#task#6): $.CodeDeliveryDetailsList[0].Destination: expected 's***@e***', got 's***@e***.com'.
UpdateUserAttributes with nothing to verify (cognito-idp-self-service-settings-and-refresh#task#9): $: expected {}, got {'CodeDeliveryDetailsList': []}.

## Steps to Reproduce
1. Replay cognito-idp-account-recovery-and-attribute-verification steps task#5 and task#8 and read CodeDeliveryDetails.Destination.
2. Replay cognito-idp-self-service-signup-and-token-session steps task#3 and task#4.
3. Replay cognito-idp-self-service-settings-and-refresh step task#9 and compare the whole 200 body against {}.

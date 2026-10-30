Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.complete-webauthn-registration-challenge-replay-rejected cognito-idp.webauthn-registration-stale-challenge-rejected cognito-idp.complete-webauthn-registration-malformed-attestation-rejected cognito-idp.complete-webauthn-registration-malformed-credential-rejected cognito-idp.complete-webauthn-registration-not-enabled-rejected cognito-idp.start-webauthn-registration-invalid-token-rejected cognito-idp.delete-webauthn-credential-nonexistent-rejected cognito-idp.delete-webauthn-credential-wrong-owner-rejected cognito-idp.webauthn-credential-lifecycle cognito-idp.list-webauthn-credentials-password-reset-required-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.complete-webauthn-registration-challenge-replay-rejected|run#21|AssertionError|assert code_msg == 'The challenge does not exist or has expired', f"expected the exact {code} message, got {code_msg!r}"
Issue-Key: cognito-idp.webauthn-registration-stale-challenge-rejected|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.complete-webauthn-registration-malformed-attestation-rejected|run#27|AssertionError|assert code_msg == 'Credential data is not valid', f"expected the exact {code} message, got {code_msg!r}"
Issue-Key: cognito-idp.complete-webauthn-registration-malformed-credential-rejected|run#15|AssertionError|assert code_msg == 'Credential data is not valid', f"expected the exact {code} message, got {code_msg!r}"
Issue-Key: cognito-idp.complete-webauthn-registration-not-enabled-rejected|run#10|AssertionError|assert code_msg == 'WebAuthn not enabled for this pool.', f"expected the exact {code} message, got {code_msg!r}"
Issue-Key: cognito-idp.start-webauthn-registration-invalid-token-rejected|run#17|AssertionError|assert same_op_message == expected_same_op_message, f"expected message {expected_same_op_message!r}, got {same_op_message!r}"
Issue-Key: cognito-idp.delete-webauthn-credential-nonexistent-rejected|run#22|AssertionError|assert message == "The credential does not exist", f"unexpected message {message!r}"
Issue-Key: cognito-idp.delete-webauthn-credential-wrong-owner-rejected|run#10|AssertionError|assert message == "The credential does not exist", f"unexpected message {message!r}"
Issue-Key: cognito-identity-provider|DeleteWebAuthnCredential|ResourceNotFoundException
Issue-Key: cognito-idp.list-webauthn-credentials-password-reset-required-rejected|_error#12|AssertionError|raise AssertionError("real AWS rejects this call, but it SUCCEEDED")
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: WebAuthn error messages are emulator prose and a password-reset-required token is accepted (5 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteWebAuthnRegistration

## Description
The WebAuthn surface answers with the right error codes but its own message text, and one refusal is missing entirely.

Cause 1 of 5 — challenge not found / expired.
AWS: WebAuthnChallengeNotFoundException with the fixed text 'The challenge does not exist or has expired' (no mention of the originating operation).
Emulator: 'The challenge from StartWebAuthnRegistration was not found.'
Why: a client distinguishing 'expired' from other failures by text sees a message AWS never sends.
Fix: the CompleteWebAuthnRegistration challenge-lookup path.
Traces: cognito-idp.complete-webauthn-registration-challenge-replay-rejected, cognito-idp.webauthn-registration-stale-challenge-rejected.

Cause 2 of 5 — malformed credential/attestation.
AWS: InvalidParameterException with AWS's exact text naming the malformed member of the credential.
Emulator: 'Credential does not carry the required member type.' for both the malformed-attestation and malformed-credential inputs (recorded behavior only for AWS's precise text, which the check pins).
Fix: CompleteWebAuthnRegistration credential parsing/validation messages.
Traces: cognito-idp.complete-webauthn-registration-malformed-attestation-rejected, cognito-idp.complete-webauthn-registration-malformed-credential-rejected.

Cause 3 of 5 — WebAuthn not enabled for the pool.
AWS: WebAuthnNotEnabledException 'WebAuthn not enabled for this pool.' (fixed text).
Emulator: 'WebAuthn is not enabled for this user pool.'
Fix: the WebAuthn-enabled gate shared by StartWebAuthnRegistration and CompleteWebAuthnRegistration.
Traces: cognito-idp.complete-webauthn-registration-not-enabled-rejected, cognito-idp.start-webauthn-registration-invalid-token-rejected.

Cause 4 of 5 — credential not found / wrong owner.
AWS: 'The credential does not exist' (fixed text, no trailing period) for DeleteWebAuthnCredential on an unknown credential and on one owned by another user — the wrong-owner case is deliberately indistinguishable.
Emulator: 'Credential does not exist.'
Fix: DeleteWebAuthnCredential lookup path.
Traces: cognito-idp.delete-webauthn-credential-nonexistent-rejected, cognito-idp.delete-webauthn-credential-wrong-owner-rejected, cognito-idp.webauthn-credential-lifecycle.

Cause 5 of 5 — ListWebAuthnCredentials with an access token for a user in a password-reset-required state.
AWS: refuses the call.
Emulator: the call SUCCEEDED.
Why: a user who must reset their password can still enumerate credentials.
Fix: ListWebAuthnCredentials must apply the user-status precondition before listing.
Traces: cognito-idp.list-webauthn-credentials-password-reset-required-rejected.

## Evidence
Verbatim from the replay:
- expected message 'The challenge does not exist or has expired', got 'The challenge from StartWebAuthnRegistration was not found.'
- expected the exact InvalidParameterException message, got 'Credential does not carry the required member type.' (twice)
- expected message 'WebAuthn not enabled for this pool.', got 'WebAuthn is not enabled for this user pool.'
- expected message 'The credential does not exist', got 'Credential does not exist.'
- cognito-idp.list-webauthn-credentials-password-reset-required-rejected: 'real AWS rejects this call, but it SUCCEEDED'

## Steps to Reproduce
1. Create a pool with WebAuthn disabled and call StartWebAuthnRegistration -> compare WebAuthnNotEnabledException.Message.
2. StartWebAuthnRegistration, CompleteWebAuthnRegistration twice with the same challenge -> compare WebAuthnChallengeNotFoundException.Message on the replay.
3. DeleteWebAuthnCredential with an unknown CredentialId -> compare message.
4. Put a user into password-reset-required state and call ListWebAuthnCredentials with its access token -> expect a refusal.

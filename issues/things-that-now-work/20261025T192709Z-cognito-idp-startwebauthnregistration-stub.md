Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.deep-cognito-idp-3-enforce-user-auth-omitted-first-factor-rejected cognito-idp.e2e-webauthn-register-authenticate-revoke cognito-idp.e2e-webauthn-registration-to-authentication cognito-idp.list-webauthn-credentials-ordered-by-creation cognito-idp.list-webauthn-credentials-returns-in-registration-order cognito-idp.webauthn-credential-list-order-matches-creation-order cognito-idp.webauthn-registration-exceeds-credential-limit-rejected cognito-idp.webauthn-registration-incomplete-challenge-leaves-no-credential
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|StartWebAuthnRegistration|NotImplemented
Issue-Commit: 9d990885182cf7376ea90557ef044b425999cd93

# Implement cognito-idp StartWebAuthnRegistration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9d990885182cf7376ea90557ef044b425999cd93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartWebAuthnRegistration

## Description
`StartWebAuthnRegistration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/start_web_authn_registration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `StartWebAuthnRegistration` call, so each one unblocks only when `StartWebAuthnRegistration` works: `cognito-idp.deep-cognito-idp-3-enforce-user-auth-omitted-first-factor-rejected`, `cognito-idp.e2e-webauthn-register-authenticate-revoke`, `cognito-idp.e2e-webauthn-registration-to-authentication`, `cognito-idp.list-webauthn-credentials-ordered-by-creation`, `cognito-idp.list-webauthn-credentials-returns-in-registration-order`, `cognito-idp.webauthn-credential-list-order-matches-creation-order`, `cognito-idp.webauthn-registration-exceeds-credential-limit-rejected`, `cognito-idp.webauthn-registration-incomplete-challenge-leaves-no-credential`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/start_web_authn_registration.rs`. Replaying the corpus, the emulator refused the `StartWebAuthnRegistration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartWebAuthnRegistration operation: operation StartWebAuthnRegistration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cognito-idp emulator
Call StartWebAuthnRegistration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

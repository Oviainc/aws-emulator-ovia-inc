Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.enable-software-token-mfa-precondition-error
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.enable-software-token-mfa-precondition-error|run#15|AssertionError|assert code0 == ("NotAuthorizedException", "Invalid session for the user."), (
Issue-Commit: e56f1153b059bce6a929f0aeca74333a3bd5954e

# Implement cognito-idp VerifySoftwareToken: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e56f1153b059bce6a929f0aeca74333a3bd5954e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
VerifySoftwareToken

## Description
`VerifySoftwareToken` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/verify_software_token.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `VerifySoftwareToken` call, so each one unblocks only when `VerifySoftwareToken` works: `cognito-idp.enable-software-token-mfa-precondition-error`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/verify_software_token.rs`. Replaying the corpus, the emulator refused the `VerifySoftwareToken` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cognito_idp_enable_software_token_mfa_precondition_error.py", line 117, in run
    assert code0 == ("NotAuthorizedException", "Invalid session for the user."), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected NotAuthorizedException verifying a software token before AssociateSoftwareToken was ever called for this session, got ('NotImplemented', 'operation VerifySoftwareToken is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cognito-idp emulator
Call VerifySoftwareToken and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

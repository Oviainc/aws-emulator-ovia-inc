Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.op-contract-confirmSignUp
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.op-contract-confirmSignUp|run#14|AssertionError|assert code1.get("Code") == "UserNotFoundException", code1
Issue-Commit: afc21d5623f1a8f7411b21bbb87a0dff6c2e7024

# Implement cognito-idp ConfirmSignUp: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `afc21d5623f1a8f7411b21bbb87a0dff6c2e7024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConfirmSignUp

## Description
`ConfirmSignUp` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/confirm_sign_up.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ConfirmSignUp` call, so each one unblocks only when `ConfirmSignUp` works: `cognito-idp.op-contract-confirmSignUp`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/confirm_sign_up.rs`. Replaying the corpus, the emulator refused the `ConfirmSignUp` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cognito_idp_op_contract_confirmSignUp.py", line 58, in run
    assert code1.get("Code") == "UserNotFoundException", code1
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: {'Message': 'operation ConfirmSignUp is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the cognito-idp emulator
Call ConfirmSignUp and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

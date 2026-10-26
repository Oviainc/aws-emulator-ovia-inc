Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-disable-provider-for-user-without-link-fails
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-disable-provider-for-user-without-link-fails|run#7|AssertionError|assert (code, message) == ("InvalidParameterException", "Account isn't linked to anyone else, verify that the account has been linked"), f"expected InvalidParam
Issue-Commit: afc21d5623f1a8f7411b21bbb87a0dff6c2e7024

# Implement cognito-idp AdminDisableProviderForUser: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `afc21d5623f1a8f7411b21bbb87a0dff6c2e7024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminDisableProviderForUser

## Description
`AdminDisableProviderForUser` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_disable_provider_for_user.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AdminDisableProviderForUser` call, so each one unblocks only when `AdminDisableProviderForUser` works: `cognito-idp.admin-disable-provider-for-user-without-link-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_disable_provider_for_user.rs`. Replaying the corpus, the emulator refused the `AdminDisableProviderForUser` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cognito_idp_admin_disable_provider_for_user_without_link_fails.py", line 40, in run
    assert (code, message) == ("InvalidParameterException", "Account isn't linked to anyone else, verify that the account has been linked"), f"expected InvalidParameterException, got {(code, message)!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidParameterException, got ('NotImplemented', 'operation AdminDisableProviderForUser is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cognito-idp emulator
Call AdminDisableProviderForUser and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.update-identity-provider-missing-required-oidc-detail-rejected cognito-idp.update-identity-provider-preserves-omitted-fields
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.update-identity-provider-missing-required-oidc-detail-rejected|run#10|AssertionError|assert (code, message) == ("InvalidParameterException", expected_message), (
Issue-Key: cognito-identity-provider|UpdateIdentityProvider|NotImplemented
Issue-Commit: afc21d5623f1a8f7411b21bbb87a0dff6c2e7024

# Implement cognito-idp UpdateIdentityProvider: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `afc21d5623f1a8f7411b21bbb87a0dff6c2e7024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIdentityProvider

## Description
`UpdateIdentityProvider` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_identity_provider.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateIdentityProvider` call, so each one unblocks only when `UpdateIdentityProvider` works: `cognito-idp.update-identity-provider-missing-required-oidc-detail-rejected`, `cognito-idp.update-identity-provider-preserves-omitted-fields`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_identity_provider.rs`. Replaying the corpus, the emulator refused the `UpdateIdentityProvider` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cognito_idp_update_identity_provider_missing_required_oidc_detail_rejected.py", line 74, in run
    assert (code, message) == ("InvalidParameterException", expected_message), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidParameterException 'sim2real-behav.example.com' for ProviderDetails missing token_url, got 'NotImplemented': 'operation UpdateIdentityProvider is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the cognito-idp emulator
Call UpdateIdentityProvider and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

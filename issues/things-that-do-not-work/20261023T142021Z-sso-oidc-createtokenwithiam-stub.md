Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-oidc.create-token-with-iam-invalid-client-rejected sso-oidc.create-token-with-iam-unsupported-grant-type-rejected
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: sso-oidc|CreateTokenWithIAM|NotImplemented
Issue-Commit: 17c220c4f82dc0c00315f2111baf55aa4ec61156

# Implement sso-oidc CreateTokenWithIAM: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17c220c4f82dc0c00315f2111baf55aa4ec61156`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTokenWithIAM

## Description
`CreateTokenWithIAM` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token_with_iam.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateTokenWithIAM` call, so each one unblocks only when `CreateTokenWithIAM` works: `sso-oidc.create-token-with-iam-invalid-client-rejected`, `sso-oidc.create-token-with-iam-unsupported-grant-type-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token_with_iam.rs`. Replaying the corpus, the emulator refused the `CreateTokenWithIAM` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sso_oidc_create_token_with_iam_invalid_client_rejected.py", line 43, in run
    assert err.response["Error"]["Code"] == "InvalidGrantException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidGrantException, got 'NotImplemented'

## Steps to Reproduce
Boot the sso-oidc emulator
Call CreateTokenWithIAM and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

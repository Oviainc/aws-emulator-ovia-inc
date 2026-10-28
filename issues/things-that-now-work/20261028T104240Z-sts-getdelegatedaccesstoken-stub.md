Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.get-delegated-access-token-without-required-permission-denied
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.get-delegated-access-token-without-required-permission-denied|run#21|AssertionError|assert target_denied == ("ValidationError", _INVALID_TOKEN_MESSAGE), (
Issue-Commit: 48883a331e1fae405ebe84b115d9b023bc18cbad

# Implement sts GetDelegatedAccessToken: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `48883a331e1fae405ebe84b115d9b023bc18cbad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDelegatedAccessToken

## Description
`GetDelegatedAccessToken` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/get_delegated_access_token.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetDelegatedAccessToken` call, so each one unblocks only when `GetDelegatedAccessToken` works: `sts.get-delegated-access-token-without-required-permission-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/get_delegated_access_token.rs`. Replaying the corpus, the emulator refused the `GetDelegatedAccessToken` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_sts_get_delegated_access_token_without_required_permission_denied.py", line 117, in run
    assert target_denied == ("ValidationError", _INVALID_TOKEN_MESSAGE), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationError (request-shape validation preempts IAM auth for this operation) even with an explicit Deny in place, got ('NotImplemented', 'operation GetDelegatedAccessToken is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sts emulator
Call GetDelegatedAccessToken and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

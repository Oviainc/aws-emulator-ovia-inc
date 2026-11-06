Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant|run#30|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Commit: e0e096f7847ddbab99498abf6c574ac0da8fbc09

# Implement cloudwatchomni CreateAlert: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e0e096f7847ddbab99498abf6c574ac0da8fbc09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlert

## Description
`CreateAlert` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_alert.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateAlert` call, so each one unblocks only when `CreateAlert` works: `cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_alert.rs`. Replaying the corpus, the emulator refused the `CreateAlert` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cloudwatchomni_alert_requires_an_explicit_assume_access_profile_trust_grant.py", line 114, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: 'operation CreateAlert is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the cloudwatchomni emulator
Call CreateAlert and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

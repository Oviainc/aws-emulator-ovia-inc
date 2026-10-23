Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mpa.get-resource-policy-invalid-policy-name-rejected
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa.get-resource-policy-invalid-policy-name-rejected|run#6|AssertionError|assert err == ("ResourceNotFoundException", NOT_FOUND), err
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# Implement mpa GetResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
`GetResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/get_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetResourcePolicy` call, so each one unblocks only when `GetResourcePolicy` works: `mpa.get-resource-policy-invalid-policy-name-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/get_resource_policy.rs`. Replaying the corpus, the emulator refused the `GetResourcePolicy` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_mpa_get_resource_policy_invalid_policy_name_rejected.py", line 41, in run
    assert err == ("ResourceNotFoundException", NOT_FOUND), err
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation GetResourcePolicy is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the mpa emulator
Call GetResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

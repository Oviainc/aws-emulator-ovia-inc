Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.concurrent-put-permission-optimistic-lock-contention codeguruprofiler.concurrent-put-permission-race-single-revision-winner codeguruprofiler.put-permission-concurrent-revision-race codeguruprofiler.put-permission-concurrent-stale-revision-race codeguruprofiler.put-permission-creates-policy-with-revision-id codeguruprofiler.put-permission-get-policy-roundtrip codeguruprofiler.put-permission-multiple-principals-policy-content codeguruprofiler.put-permission-replaces-principal-set-not-merges codeguruprofiler.put-permission-revision-id-changes-on-repeated-update codeguruprofiler.put-permission-revision-id-changes-on-update codeguruprofiler.put-permission-stale-revision-conflict-rejected codeguruprofiler.put-permission-then-get-policy-matches codeguruprofiler.remove-permission-clears-policy codeguruprofiler.remove-permission-deletes-policy codeguruprofiler.remove-permission-stale-revision-conflict-rejected codeguruprofiler.remove-permission-stale-revision-id-rejected
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler|PutPermission|NotImplemented
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Implement codeguruprofiler PutPermission: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPermission

## Description
`PutPermission` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/put_permission.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

16 recorded unit(s) fail on their `PutPermission` call, so each one unblocks only when `PutPermission` works: `codeguruprofiler.concurrent-put-permission-optimistic-lock-contention`, `codeguruprofiler.concurrent-put-permission-race-single-revision-winner`, `codeguruprofiler.put-permission-concurrent-revision-race`, `codeguruprofiler.put-permission-concurrent-stale-revision-race`, `codeguruprofiler.put-permission-creates-policy-with-revision-id`, `codeguruprofiler.put-permission-get-policy-roundtrip`, `codeguruprofiler.put-permission-multiple-principals-policy-content`, `codeguruprofiler.put-permission-replaces-principal-set-not-merges`, `codeguruprofiler.put-permission-revision-id-changes-on-repeated-update`, `codeguruprofiler.put-permission-revision-id-changes-on-update`, `codeguruprofiler.put-permission-stale-revision-conflict-rejected`, `codeguruprofiler.put-permission-then-get-policy-matches`, `codeguruprofiler.remove-permission-clears-policy`, `codeguruprofiler.remove-permission-deletes-policy`, `codeguruprofiler.remove-permission-stale-revision-conflict-rejected`, `codeguruprofiler.remove-permission-stale-revision-id-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/put_permission.rs`. Replaying the corpus, the emulator refused the `PutPermission` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutPermission operation: operation PutPermission is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codeguruprofiler emulator
Call PutPermission and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.delete-namespace-blocked-by-nonempty-precondition
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight.delete-namespace-blocked-by-nonempty-precondition|run#7|AssertionError|assert err["Code"] == "InvalidParameterValueException", (
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# Implement quicksight DeleteNamespace: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNamespace

## Description
`DeleteNamespace` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/delete_namespace.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteNamespace` call, so each one unblocks only when `DeleteNamespace` works: `quicksight.delete-namespace-blocked-by-nonempty-precondition`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/delete_namespace.rs`. Replaying the corpus, the emulator refused the `DeleteNamespace` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_quicksight_delete_namespace_blocked_by_nonempty_precondition.py", line 32, in run
    assert err["Code"] == "InvalidParameterValueException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidParameterValueException deleting the default namespace, got {'Message': 'operation DeleteNamespace is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the quicksight emulator
Call DeleteNamespace and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

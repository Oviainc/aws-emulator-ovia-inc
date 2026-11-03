Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resource-explorer-2.batch-get-view-partial-errors
Issue-Shape-Id: com.amazonaws.resourceexplorer2#ResourceExplorer
Issue-Key: resource-explorer-2.batch-get-view-partial-errors|run#8|AssertionError|assert error == ("UnauthorizedException", "Unauthorized"), (
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# Implement resource-explorer-2 BatchGetView: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetView

## Description
`BatchGetView` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/batch_get_view.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `BatchGetView` call, so each one unblocks only when `BatchGetView` works: `resource-explorer-2.batch-get-view-partial-errors`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/batch_get_view.rs`. Replaying the corpus, the emulator refused the `BatchGetView` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_resource_explorer_2_batch_get_view_partial_errors.py", line 31, in run
    assert error == ("UnauthorizedException", "Unauthorized"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected UnauthorizedException / Unauthorized for the whole call when one of two ViewArns does not resolve to a real, accessible view, got ('NotImplemented', 'operation BatchGetView is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the resource-explorer-2 emulator
Call BatchGetView and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

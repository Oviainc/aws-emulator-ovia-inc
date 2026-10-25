Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.update-index-optimistic-lock-conflicting-mapping-writes
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.update-index-optimistic-lock-conflicting-mapping-writes|run#12|AssertionError|assert len(successes) == 1, f"expected exactly one racer to win, got {len(successes)}: {results!r}"
Issue-Commit: 85a3e609e0397cc56b94fbca2b474c505c96ae80

# Implement opensearch UpdateIndex: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `85a3e609e0397cc56b94fbca2b474c505c96ae80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIndex

## Description
`UpdateIndex` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_index.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateIndex` call, so each one unblocks only when `UpdateIndex` works: `opensearch.update-index-optimistic-lock-conflicting-mapping-writes`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_index.rs`. Replaying the corpus, the emulator refused the `UpdateIndex` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_opensearch_update_index_optimistic_lock_conflicting_mapping_writes.py", line 53, in run
    assert len(successes) == 1, f"expected exactly one racer to win, got {len(successes)}: {results!r}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected exactly one racer to win, got 0: [('integer', ('error', ClientError('An error occurred (NotImplemented) when calling the UpdateIndex operation: operation UpdateIndex is scaffolded but not implemented in this emulator'))), ('keyword', ('error', ClientError('An error occurred (NotImplemented) when calling the UpdateIndex operation: operation UpdateIndex is scaffolded but not implemented in this emulator')))]

## Steps to Reproduce
Boot the opensearch emulator
Call UpdateIndex and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

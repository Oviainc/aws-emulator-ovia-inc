Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.ddbglobal-legacy-2017-global-table-apis-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.ddbglobal-legacy-2017-global-table-apis-rejected|run#14|AssertionError|assert code == ERR_VALIDATION, (
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# Implement dynamodb CreateGlobalTable: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGlobalTable

## Description
`CreateGlobalTable` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/create_global_table.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateGlobalTable` call, so each one unblocks only when `CreateGlobalTable` works: `dynamodb.ddbglobal-legacy-2017-global-table-apis-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/create_global_table.rs`. Replaying the corpus, the emulator refused the `CreateGlobalTable` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_dynamodb_ddbglobal_legacy_2017_global_table_apis_rejected.py", line 98, in run
    assert code == ERR_VALIDATION, (
           ^^^^^^^^^^^^^^^^^^^^^^
AssertionError: CreateGlobalTable over two identically-named stream-enabled tables returned 'NotImplemented', expected 'ValidationException'; message was 'operation CreateGlobalTable is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the dynamodb emulator
Call CreateGlobalTable and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

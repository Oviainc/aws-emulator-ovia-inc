Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.delete-current-table-version-rejected glue.table-version-deletion
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.delete-current-table-version-rejected|run#15|AssertionError|assert refused == ("InvalidInputException", "VersionId cannot be equal to the latest."), (
Issue-Key: glue|DeleteTableVersion|NotImplemented
Issue-Commit: 006ebdb99ede2ef0c738495f5c60cb240146f0ca

# Implement glue DeleteTableVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `006ebdb99ede2ef0c738495f5c60cb240146f0ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTableVersion

## Description
`DeleteTableVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-glue-awsglue/src/ops/delete_table_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteTableVersion` call, so each one unblocks only when `DeleteTableVersion` works: `glue.delete-current-table-version-rejected`, `glue.table-version-deletion`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-glue-awsglue/src/ops/delete_table_version.rs`. Replaying the corpus, the emulator refused the `DeleteTableVersion` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_glue_delete_current_table_version_rejected.py", line 84, in run
    assert refused == ("InvalidInputException", "VersionId cannot be equal to the latest."), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: deleting the current table version 2 must be refused with InvalidInputException, got ('NotImplemented', 'operation DeleteTableVersion is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the glue emulator
Call DeleteTableVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

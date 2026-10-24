Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.server-side-service-software-rollback-window-enforced-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|RollbackServiceSoftwareUpdate|NotImplemented
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Implement opensearch RollbackServiceSoftwareUpdate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RollbackServiceSoftwareUpdate

## Description
`RollbackServiceSoftwareUpdate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/rollback_service_software_update.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RollbackServiceSoftwareUpdate` call, so each one unblocks only when `RollbackServiceSoftwareUpdate` works: `opensearch.server-side-service-software-rollback-window-enforced-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/rollback_service_software_update.rs`. Replaying the corpus, the emulator refused the `RollbackServiceSoftwareUpdate` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_opensearch_server_side_service_software_rollback_window_enforced_rejected.py", line 39, in run
    assert code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException (or a 200 declining with RollbackAvailable=False), got 'NotImplemented'

## Steps to Reproduce
Boot the opensearch emulator
Call RollbackServiceSoftwareUpdate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

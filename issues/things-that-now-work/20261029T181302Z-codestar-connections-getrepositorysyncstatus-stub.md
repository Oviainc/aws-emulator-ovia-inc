Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codestar-connections.get-repository-sync-status-without-history-rejected
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections.get-repository-sync-status-without-history-rejected|run#10|AssertionError|assert err["Code"] == "ResourceNotFoundException", (
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# Implement codestar-connections GetRepositorySyncStatus: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositorySyncStatus

## Description
`GetRepositorySyncStatus` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/get_repository_sync_status.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetRepositorySyncStatus` call, so each one unblocks only when `GetRepositorySyncStatus` works: `codestar-connections.get-repository-sync-status-without-history-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/get_repository_sync_status.rs`. Replaying the corpus, the emulator refused the `GetRepositorySyncStatus` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_codestar_connections_get_repository_sync_status_without_history_rejected.py", line 37, in run
    assert err["Code"] == "ResourceNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException for a branch with no sync history, got {'Message': 'operation GetRepositorySyncStatus is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the codestar-connections emulator
Call GetRepositorySyncStatus and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

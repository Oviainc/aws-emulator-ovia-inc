Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.delete-custom-engine-version-boundary
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.delete-custom-engine-version-boundary|run#5|AssertionError|assert error.get("Code") == ERR_ACCESS_DENIED, f"unexpected error code: {error!r}"
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Implement rds DeleteCustomDBEngineVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomDBEngineVersion

## Description
`DeleteCustomDBEngineVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/delete_custom_db_engine_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteCustomDBEngineVersion` call, so each one unblocks only when `DeleteCustomDBEngineVersion` works: `rds.delete-custom-engine-version-boundary`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/delete_custom_db_engine_version.rs`. Replaying the corpus, the emulator refused the `DeleteCustomDBEngineVersion` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_rds_delete_custom_engine_version_boundary.py", line 52, in run
    assert error.get("Code") == ERR_ACCESS_DENIED, f"unexpected error code: {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected error code: {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation DeleteCustomDBEngineVersion is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the rds emulator
Call DeleteCustomDBEngineVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

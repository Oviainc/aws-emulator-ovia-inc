Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appflow.update-flow-updates-task-configuration
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|UpdateFlow|NotImplemented
Issue-Commit: a2b95246df63384d1b9b9d3ee28f05a727c934df

# Implement appflow UpdateFlow: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a2b95246df63384d1b9b9d3ee28f05a727c934df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFlow

## Description
`UpdateFlow` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/src/ops/update_flow.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateFlow` call, so each one unblocks only when `UpdateFlow` works: `appflow.update-flow-updates-task-configuration`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/src/ops/update_flow.rs`. Replaying the corpus, the emulator refused the `UpdateFlow` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_appflow_update_flow_updates_task_configuration.py", line 65, in run
    raise AssertionError(f"update_flow failed ({len(msg)} chars): {msg!r}") from err
AssertionError: update_flow failed (71 chars): 'operation UpdateFlow is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the appflow emulator
Call UpdateFlow and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

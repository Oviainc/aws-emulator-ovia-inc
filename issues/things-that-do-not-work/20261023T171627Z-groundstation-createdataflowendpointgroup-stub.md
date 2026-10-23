Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: groundstation.dataflow-endpoint-group-lifecycle
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation.dataflow-endpoint-group-lifecycle|run#6|AssertionError|assert refused == (
Issue-Commit: 326b33f63f33ac22c419b7169b10d47cfd8c011f

# Implement groundstation CreateDataflowEndpointGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `326b33f63f33ac22c419b7169b10d47cfd8c011f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataflowEndpointGroup

## Description
`CreateDataflowEndpointGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_dataflow_endpoint_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateDataflowEndpointGroup` call, so each one unblocks only when `CreateDataflowEndpointGroup` works: `groundstation.dataflow-endpoint-group-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_dataflow_endpoint_group.rs`. Replaying the corpus, the emulator refused the `CreateDataflowEndpointGroup` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_groundstation_dataflow_endpoint_group_lifecycle.py", line 59, in run
    assert refused == (
           ^^^^^^^^^^^^
    ...<2 lines>...
    ), f"unexpected outcome without securityDetails: {refused!r}"
    ^
AssertionError: unexpected outcome without securityDetails: ('NotImplemented', 'operation CreateDataflowEndpointGroup is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the groundstation emulator
Call CreateDataflowEndpointGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

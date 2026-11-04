Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.serverless-v2-capacity-modify-current-capacity-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.serverless-v2-capacity-modify-current-capacity-rejected|run#10|AssertionError|assert error["Code"] == "InvalidParameterValue", f"unexpected error {error!r}"
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Implement rds ModifyCurrentDBClusterCapacity: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCurrentDBClusterCapacity

## Description
`ModifyCurrentDBClusterCapacity` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_current_db_cluster_capacity.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ModifyCurrentDBClusterCapacity` call, so each one unblocks only when `ModifyCurrentDBClusterCapacity` works: `rds.serverless-v2-capacity-modify-current-capacity-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_current_db_cluster_capacity.rs`. Replaying the corpus, the emulator refused the `ModifyCurrentDBClusterCapacity` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_rds_serverless_v2_capacity_modify_current_capacity_rejected.py", line 50, in run
    assert error["Code"] == "InvalidParameterValue", f"unexpected error {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected error {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation ModifyCurrentDBClusterCapacity is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the rds emulator
Call ModifyCurrentDBClusterCapacity and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

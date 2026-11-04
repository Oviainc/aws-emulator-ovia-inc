Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.backtrack-cluster-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.backtrack-cluster-wrong-state-rejected|run#11|AssertionError|assert code == "InvalidDBClusterStateFault", f"unexpected error code {code!r}: {message!r}"
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Implement rds BacktrackDBCluster: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BacktrackDBCluster

## Description
`BacktrackDBCluster` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/backtrack_db_cluster.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `BacktrackDBCluster` call, so each one unblocks only when `BacktrackDBCluster` works: `rds.backtrack-cluster-wrong-state-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/backtrack_db_cluster.rs`. Replaying the corpus, the emulator refused the `BacktrackDBCluster` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_rds_backtrack_cluster_wrong_state_rejected.py", line 54, in run
    assert code == "InvalidDBClusterStateFault", f"unexpected error code {code!r}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected error code 'NotImplemented': 'operation BacktrackDBCluster is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the rds emulator
Call BacktrackDBCluster and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

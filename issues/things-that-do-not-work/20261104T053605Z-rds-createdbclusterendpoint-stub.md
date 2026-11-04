Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.concurrent-create-cluster-endpoint-duplicate-name-race rds.modify-cluster-endpoint-empty-membership-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.concurrent-create-cluster-endpoint-duplicate-name-race|run#15|AssertionError|assert len(successes) == 1, f"expected exactly 1 success, got {len(successes)}: {outcomes!r}"
Issue-Key: rds|CreateDBClusterEndpoint|NotImplemented
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Implement rds CreateDBClusterEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBClusterEndpoint

## Description
`CreateDBClusterEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateDBClusterEndpoint` call, so each one unblocks only when `CreateDBClusterEndpoint` works: `rds.concurrent-create-cluster-endpoint-duplicate-name-race`, `rds.modify-cluster-endpoint-empty-membership-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster_endpoint.rs`. Replaying the corpus, the emulator refused the `CreateDBClusterEndpoint` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_rds_concurrent_create_cluster_endpoint_duplicate_name_race.py", line 65, in run
    assert len(successes) == 1, f"expected exactly 1 success, got {len(successes)}: {outcomes!r}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected exactly 1 success, got 0: [{'ok': False, 'value': None, 'code': 'NotImplemented', 'message': 'operation CreateDBClusterEndpoint is scaffolded but not implemented in this emulator'}, {'ok': False, 'value': None, 'code': 'NotImplemented', 'message': 'operation CreateDBClusterEndpoint is scaffolded but not implemented in this emulator'}]

## Steps to Reproduce
Boot the rds emulator
Call CreateDBClusterEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

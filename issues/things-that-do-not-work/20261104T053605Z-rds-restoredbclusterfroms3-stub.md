Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-restore-db-cluster-from-s3-role-missing-listbucket-negative
Issue-Key: rds__s3.rds-restore-db-cluster-from-s3-role-missing-listbucket-negative|run#11|AssertionError|assert error["Code"] == "InvalidParameterValue", (
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Implement rds RestoreDBClusterFromS3: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreDBClusterFromS3

## Description
`RestoreDBClusterFromS3` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/restore_db_cluster_from_s3.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RestoreDBClusterFromS3` call, so each one unblocks only when `RestoreDBClusterFromS3` works: `rds__s3.rds-restore-db-cluster-from-s3-role-missing-listbucket-negative`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/restore_db_cluster_from_s3.rs`. Replaying the corpus, the emulator refused the `RestoreDBClusterFromS3` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_rds__s3_rds_restore_db_cluster_from_s3_role_missing_listbucket_negative.py", line 52, in run
    assert error["Code"] == "InvalidParameterValue", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidParameterValue, got NotImplemented: operation RestoreDBClusterFromS3 is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the rds emulator
Call RestoreDBClusterFromS3 and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

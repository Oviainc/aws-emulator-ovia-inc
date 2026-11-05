Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.op-jobflow-style-ops-reject-unknown-cluster
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|SetVisibleToAllUsers|NotImplemented
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# Implement emr SetVisibleToAllUsers: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetVisibleToAllUsers

## Description
`SetVisibleToAllUsers` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-emr-elasticmapreduce/src/ops/set_visible_to_all_users.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `SetVisibleToAllUsers` call, so each one unblocks only when `SetVisibleToAllUsers` works: `emr.op-jobflow-style-ops-reject-unknown-cluster`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-emr-elasticmapreduce/src/ops/set_visible_to_all_users.rs`. Replaying the corpus, the emulator refused the `SetVisibleToAllUsers` call: File "check_emr_op_jobflow_style_ops_reject_unknown_cluster.py", line 263, in run
    _expect_success(lambda: emr.set_visible_to_all_users(
  File "check_emr_op_jobflow_style_ops_reject_unknown_cluster.py", line 189, in _expect_success
    raise AssertionError(
AssertionError: expected the positive control to succeed against a real, live cluster, got ClientError('An error occurred (NotImplemented) when calling the SetVisibleToAllUsers operation: operation SetVisibleToAllUsers is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the emr emulator
Call SetVisibleToAllUsers and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

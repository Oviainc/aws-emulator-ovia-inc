Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.op-managed-scaling-policy-ops-reject-unknown-cluster
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.op-managed-scaling-policy-ops-reject-unknown-cluster|_assert_rejections#4|AssertionError|assert not wrong, (
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# Implement emr GetManagedScalingPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetManagedScalingPolicy

## Description
`GetManagedScalingPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-emr-elasticmapreduce/src/ops/get_managed_scaling_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetManagedScalingPolicy` call, so each one unblocks only when `GetManagedScalingPolicy` works: `emr.op-managed-scaling-policy-ops-reject-unknown-cluster`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-emr-elasticmapreduce/src/ops/get_managed_scaling_policy.rs`. Replaying the corpus, the emulator refused the `GetManagedScalingPolicy` call: File "check_emr_op_managed_scaling_policy_ops_reject_unknown_cluster.py", line 54, in run
    _assert_rejections(
    ~~~~~~~~~~~~~~~~~~^
        {
        ^
    ...<31 lines>...
        },
        ^^
    )
    ^
  File "check_emr_op_managed_scaling_policy_ops_reject_unknown_cluster.py", line 32, in _assert_rejections
    assert not wrong, (
           ^^^^^^^^^
AssertionError: expected (ValidationException, <exact message>) for every op, got mismatches: {'GetManagedScalingPolicy': ('NotImplemented', 'operation GetManagedScalingPolicy is scaffolded but not implemented in this emulator'), 'PutManagedScalingPolicy': ('NotImplemented', 'operation PutManagedScalingPolicy is scaffolded but not implemented in this emulator'), 'PutManagedScalingPolicyInverted': ('NotImplemented', 'operation PutManagedScalingPolicy is scaffolded but not implemented in this emulator'), 'RemoveManagedScalingPolicy': ('NotImplemented', 'operation RemoveManagedScalingPolicy is scaffolded but not implemented in this emulator')}

## Steps to Reproduce
Boot the emr emulator
Call GetManagedScalingPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

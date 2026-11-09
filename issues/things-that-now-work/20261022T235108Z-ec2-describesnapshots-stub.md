Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-store-image-task-unsupported-bucket-region-rejected ec2.store-image-task-lifecycle-to-completion
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-store-image-task-unsupported-bucket-region-rejected|run#10|WaiterError|ec2.get_waiter("snapshot_completed").wait(SnapshotIds=[snap_id])
Issue-Key: ec2|DescribeSnapshots|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 DescribeSnapshots: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSnapshots

## Description
`DescribeSnapshots` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DescribeSnapshots` call, so each one unblocks only when `DescribeSnapshots` works: `ec2.create-store-image-task-unsupported-bucket-region-rejected`, `ec2.store-image-task-lifecycle-to-completion`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs`. Replaying the corpus, the emulator refused the `DescribeSnapshots` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "waiter.py", line 363, in wait
    raise WaiterError(
    ...<6 lines>...
    )
botocore.exceptions.WaiterError: Waiter SnapshotCompleted failed: An error occurred (NotImplemented): operation DescribeSnapshots is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call DescribeSnapshots and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

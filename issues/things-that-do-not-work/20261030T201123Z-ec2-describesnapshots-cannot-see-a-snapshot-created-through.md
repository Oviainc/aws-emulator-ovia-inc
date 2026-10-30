Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.store-image-task-lifecycle-to-completion
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeSnapshots|InvalidSnapshot.NotFound
Issue-Commit: 610eb4b5abbe12768656cecbe381c20cd7e570c7

# ec2: DescribeSnapshots cannot see a snapshot created through the EBS direct API, so a StartSnapshot/CompleteSnapshot snapshot is reported InvalidSnapshot.NotFound (1 cause)

> _Note: this issue was identified against the code base at commit `610eb4b5abbe12768656cecbe381c20cd7e570c7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSnapshots

## Description
One net-new behavioral root cause, grounded in the one surviving ec2 failing check listed under Traces. It is NOT the already-filed scaffold-stub cause: `DescribeSnapshots` is now implemented (its handler header reads `//! @op DescribeSnapshots //! IMPLEMENTED: reports the EBS snapshots crate::state holds ...`) and answers a modeled-style EC2 refusal rather than 501 NotImplemented. Two open issues describe the OLD 501 behaviour and therefore do not cover this: `20261022T235108Z-ec2-describesnapshots-stub.md` ("Implement ec2 DescribeSnapshots: unimplemented scaffold stub", which happens to list this check id but whose cause — an unimplemented stub answering 501 — no longer reproduces) and cause 1 of `20261022T224826Z-ebs-direct-apis-ec2-describesnapshots-stub-blocks-23.md` (same 501 framing, for the `ebs.*` checks). This is a state-sharing defect in the implemented handler, not a missing handler.

=== Cause 1 of 1: a snapshot minted by the EBS direct API (StartSnapshot/PutSnapshotBlock/CompleteSnapshot) is invisible to EC2 DescribeSnapshots ===
AWS: the EBS direct API and EC2 describe ONE snapshot resource. `ebs:StartSnapshot` creates a real EBS snapshot and returns its id and `Status` `pending` (Smithy com.amazonaws.ebs: StartSnapshot -> StartSnapshotResponse.SnapshotId/Status, HTTP 201; CompleteSnapshot -> HTTP 202 returning Status); the ebs model exposes no read of snapshot state at all, so `ec2:DescribeSnapshots` is the documented companion read — it is the only way a caller learns the snapshot moved to `completed` with `Progress` `100%`. Smithy com.amazonaws.ec2#DescribeSnapshots takes `SnapshotIds` and returns `Snapshots` (SnapshotList) and models NO errors (ec2Query protocol), so the refusal code for an unknown id is recorded behavior only: the recording shows `InvalidSnapshot.NotFound` with the request-derived message `The snapshot '<snapshot-id>' does not exist.` — fixed text `The snapshot '` + the SnapshotId from the request + `' does not exist.`. For an id that ebs:StartSnapshot just minted, AWS does NOT take that branch: it returns the snapshot record (State `pending` then `completed`).
Emulator: `ec2:DescribeSnapshots(SnapshotIds=[snap-...])` on a snapshot created seconds earlier by `ebs:StartSnapshot` + `ebs:PutSnapshotBlock` + `ebs:CompleteSnapshot` (all of which answered 2xx) refuses with `An error occurred (InvalidSnapshot.NotFound) when calling the DescribeSnapshots operation: The snapshot 'snap-...' does not exist.` The two services keep separate stores: `services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs` persists the snapshot in the ebs crate's `crate::state`, while `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs` reports only what the ec2 crate's `crate::state` holds, so no id minted on the EBS direct-API side is ever resolvable on the EC2 side.
Why: every EBS direct-API workflow is unusable end to end. A client that writes blocks and then waits for the snapshot to become usable can never observe completion, and any EC2 operation that consumes such a snapshot (RegisterImage with a BlockDeviceMappings SnapshotId, CreateStoreImageTask, CopySnapshot, DeleteSnapshot cleanup) sees a snapshot that "does not exist" even though the writing API reported success. The emulator also contradicts itself: the id is readable through ebs:ListSnapshotBlocks and not through ec2:DescribeSnapshots.
Fix: the snapshot state the two crates read and write — back `DescribeSnapshots` (and the other ec2 snapshot readers/writers) with the same snapshot records the EBS direct-API handlers mutate, so `StartSnapshot` leaves a `pending` snapshot that EC2 reports, `CompleteSnapshot` drives it to `completed` with `Progress` `100%`, and `ec2:DeleteSnapshot` removes it. The `InvalidSnapshot.NotFound` code and message template themselves are correct and must stay for ids that really do not exist.
Traces: ec2.store-image-task-lifecycle-to-completion.

## Evidence
Check `ec2.store-image-task-lifecycle-to-completion` (verdict: error), after `ebs.start_snapshot(VolumeSize=1)` -> SnapshotId, `ebs.put_snapshot_block(BlockIndex=0, DataLength=524288, Checksum=<b64 sha256>, ChecksumAlgorithm='SHA256')`, `ebs.complete_snapshot(ChangedBlocksCount=1)` all succeeding:

  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (InvalidSnapshot.NotFound) when calling the DescribeSnapshots operation: The snapshot 'snap-df7d398f60d7294bb' does not exist.

Expected: a 200 DescribeSnapshots reply whose `Snapshots[0]` names that SnapshotId with `State` `pending` and then `completed` (and `Progress` `100%`), which is what the check polls for (timeout 180s, interval 5s) before it ever reaches RegisterImage / CreateStoreImageTask. The snapshot id shown is the per-run value; the requirement is that ANY id returned by ebs:StartSnapshot resolves on the EC2 side.

Code pointers: ebs snapshot records are written in services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs ("persists it in [`crate::state`]"); ec2 reads a different store in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs lines 3-11 ("reports the EBS snapshots `crate::state` holds").

## Steps to Reproduce
1. Boot the emulator and point botocore's `ebs`, `ec2` and `s3` clients at it.
2. `snap = ebs.start_snapshot(VolumeSize=1)['SnapshotId']` -> 201 with Status 'pending'.
3. `ebs.put_snapshot_block(SnapshotId=snap, BlockIndex=0, BlockData=<524288 random bytes>, DataLength=524288, Checksum=<base64 sha256>, ChecksumAlgorithm='SHA256')` -> 2xx.
4. `ebs.complete_snapshot(SnapshotId=snap, ChangedBlocksCount=1)` -> 2xx.
5. `ec2.describe_snapshots(SnapshotIds=[snap])` -> observe `InvalidSnapshot.NotFound` / "The snapshot '<snap>' does not exist." instead of the snapshot record with State pending/completed.
6. Replay `ec2.store-image-task-lifecycle-to-completion` for the full end-to-end repro (it then cannot reach RegisterImage, CreateStoreImageTask or the S3 head_object assertions).

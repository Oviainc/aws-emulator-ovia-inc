Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain
Issue-Key: cloudformation__ec2|DescribeSnapshots|NotImplemented
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# cloudformation__ec2: EC2 DescribeSnapshots answers 501 NotImplemented, so a CFN AWS::EC2::Volume DeletionPolicy=Snapshot stack cannot be verified (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSnapshots

## Description
One net-new root cause for this corpus. The replay report did NOT mark this check as covered: the open issue 20261022T235108Z-ec2-describesnapshots-stub.md names the same operation but belongs to corpus simparity-behavioral-aws-v1 and its Issue-Trace-Ids list only `ec2.create-store-image-task-unsupported-bucket-region-rejected` and `ec2.store-image-task-lifecycle-to-completion` — not this check — so a fixer closing that issue would never re-replay this one. Close this one by making the check below pass; it shares a root cause with that issue and should be closed together with it.

Cause 1 of 1: EC2 DescribeSnapshots is an unimplemented scaffold stub, so nothing downstream of a snapshot can be observed.

AWS: DescribeSnapshots answers HTTP 200 with DescribeSnapshotsResult. Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): input DescribeSnapshotsInput {DryRun, Filters (FilterList — including `volume-id`, `snapshot-id`, `status`, `tag:<key>`), MaxResults, NextToken, OwnerIds, RestorableByUserIds, SnapshotIds}; output {Snapshots: SnapshotList, NextToken}. The modeled documentation states that an invalid snapshot id is refused (EC2's `InvalidSnapshot.NotFound`, HTTP 400, awsQuery error envelope) and that a snapshot the caller cannot see is simply omitted from the result rather than being an error. A CloudFormation stack whose AWS::EC2::Volume carries `DeletionPolicy: Snapshot` leaves exactly one completed snapshot of that volume behind after DELETE_COMPLETE, and DescribeSnapshots filtered on `volume-id` is the only way a client learns its SnapshotId and state.

Emulator: the call is refused HTTP 501 with error code `NotImplemented` and the message `operation DescribeSnapshots is scaffolded but not implemented in this emulator`. That message is entirely emulator-authored — real EC2 has no such code or message for this operation. The handler still carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` and returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeSnapshots"))`.

Why: a client cannot read any snapshot at all. Concretely, the CFN deletion-policy contract is unverifiable: after deleting the stack the caller cannot tell a `DeletionPolicy: Snapshot` volume (snapshot kept, volume gone) from a `DeletionPolicy: Retain` volume (volume kept, no snapshot), and every backup/restore flow, every `snapshot_completed` waiter and every `aws ec2 describe-snapshots` poll fails with a 501 that carries no AWS error code to branch on.

Fix: the operation's handler — services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` in `DescribeSnapshots::handle` with a real handler reading the EC2 crate's snapshot state (filtering, owner scoping and NextToken paging), then delete that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

Traces: cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain (verdict error; the check reaches DescribeSnapshots after the stack round trip). Handler citation: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs lines 4 and 196.

## Evidence
Expected (AWS): DescribeSnapshots -> HTTP 200, {"Snapshots": [ {SnapshotId, VolumeId, State: "completed", Progress: "100%", VolumeSize, StartTime, OwnerId, Tags, ...} ], optional NextToken}; the snapshot a DeletionPolicy=Snapshot volume left behind is found by Filters=[{Name: "volume-id", Values: [<volume id>]}]. An unknown snapshot id is refused InvalidSnapshot.NotFound (HTTP 400).

Actual (emulator): HTTP 501, error code `NotImplemented`, message `operation DescribeSnapshots is scaffolded but not implemented in this emulator`. Replay detail: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeSnapshots operation: operation DescribeSnapshots is scaffolded but not implemented in this emulator`. Of that message, nothing is AWS's — the fixed text and the operation name are both emulator-authored; AWS returns no error at all for a well-formed request.

Supporting: the handler file carries the stub census marker `//! SCAFFOLDED, NOT IMPLEMENTED:` (describe_snapshots.rs:4) and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeSnapshots"))` (describe_snapshots.rs:196).

## Steps to Reproduce
1. Boot the emulator and call ec2:DescribeSnapshots with no parameters. Expected: HTTP 200 with a Snapshots list (possibly empty). Actual: HTTP 501 NotImplemented.
2. Full replay: cloudformation:CreateStack with two AWS::EC2::Volume resources (1 GiB, gp3, same AZ), one with `DeletionPolicy: Snapshot` and one with `DeletionPolicy: Retain`, outputs Ref-ing both volume ids; wait for CREATE_COMPLETE; cloudformation:DeleteStack and wait for DELETE_COMPLETE; then ec2:DescribeSnapshots Filters=[{Name:"volume-id",Values:[<SnapVolId>]}] to confirm exactly one completed snapshot exists, and ec2:DescribeVolumes to confirm the Retain volume survived.
3. Re-replay check cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain to confirm the fix.

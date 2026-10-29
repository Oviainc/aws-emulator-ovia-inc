Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.deep-cloudformation-2-snapshot-volume
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeSnapshots|NotImplemented
Issue-Commit: 37852a0be4ac38d01c19b49960a2b41fed2b2c68

# cloudformation: EC2 DescribeSnapshots answers 501 NotImplemented, so a DeletionPolicy=Snapshot AWS::EC2::Volume stack cannot be observed at all (1 cause)

> _Note: this issue was identified against the code base at commit `37852a0be4ac38d01c19b49960a2b41fed2b2c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSnapshots

## Description
One net-new root cause for this corpus, grounded in the single failing check the replay report left unmarked (it is neither "ALREADY FILED" nor "OWNED by" any open issue).

Related-but-not-covering issues, named here so a fixer can close them together: 20261029T065746Z-cloudformation-ec2-ec2-describesnapshots-answers-501-notimplemented-so.md (same operation, but its Issue-Key is `cloudformation__ec2|DescribeSnapshots|NotImplemented` and its Issue-Trace-Ids list only `cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain`), and 20261022T235108Z-ec2-describesnapshots-stub.md (corpus simparity-behavioral-aws-v1, trace ids `ec2.create-store-image-task-unsupported-bucket-region-rejected` / `ec2.store-image-task-lifecycle-to-completion`). Neither carries the replay key of the check below, so a fixer closing either would never re-replay it. This issue is the work item for `cloudformation.deep-cloudformation-2-snapshot-volume`.

Cause 1 of 1: EC2 DescribeSnapshots is an unimplemented scaffold stub, so a CloudFormation stack that snapshots a volume on delete is unobservable — failing check: cloudformation.deep-cloudformation-2-snapshot-volume.

AWS: DescribeSnapshots answers HTTP 200 with DescribeSnapshotsResult and no error for a well-formed request. Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): input DescribeSnapshotsRequest {DryRun, Filters (FilterList, xmlName Filter — includes `volume-id`, `snapshot-id`, `status`, `tag:<key>`), MaxResults, NextToken, OwnerIds (xmlName Owner — accepts `self`), RestorableByUserIds, SnapshotIds}; output DescribeSnapshotsResult {Snapshots: SnapshotList (xmlName snapshotSet), NextToken}; `errors: []` and `paginated {inputToken: NextToken, outputToken: NextToken, items: Snapshots, pageSize: MaxResults}`, so the operation is pageable and a listing that matches nothing is an empty Snapshots list, never a refusal. Recorded behavior for the surrounding contract: a stack whose AWS::EC2::Volume carries `DeletionPolicy: Snapshot` deletes the volume on DeleteStack (DescribeVolumes then refuses the id with EC2's `InvalidVolume.NotFound`) and leaves exactly one snapshot of that volume, whose VolumeId is the deleted volume id, whose VolumeSize is the volume's size and whose State is not `error`; that snapshot is addressable both by Filters=[{Name:"volume-id",Values:[<volume id>]}] with OwnerIds=["self"] and directly by SnapshotIds=[<snapshot id>].

Emulator: every DescribeSnapshots call — including the pre-delete call that must return an EMPTY Snapshots list — is refused with HTTP 501, error code `NotImplemented` and the message `operation DescribeSnapshots is scaffolded but not implemented in this emulator`. The whole message is emulator-authored: its only variable part is the operation name `DescribeSnapshots`, taken from the request; AWS has no such code or message for this operation, and for this request returns no error at all. The handler still carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` and returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeSnapshots"))`.

Why: a client can read no snapshot whatsoever. It cannot establish the pre-condition that no snapshot of a volume exists yet, and after DeleteStack it cannot distinguish a `DeletionPolicy: Snapshot` volume (volume gone, one snapshot kept) from `Retain` or `Delete`, so the CloudFormation deletion-policy contract is unverifiable. Every backup/restore flow, every `snapshot_completed` waiter, every `aws ec2 describe-snapshots` poll and every cleanup path that enumerates snapshots before deleting them fails with a 501 carrying no AWS error code to branch on; code written against AWS sees an unmodelled error where AWS returns 200.

Fix: the operation's handler — services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs. Replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` in `DescribeSnapshots::handle` with a real handler that reads the EC2 crate's snapshot state (owner scoping for `self`, Filters including `volume-id`, SnapshotIds selection, NextToken/MaxResults paging) and have CloudFormation's AWS::EC2::Volume delete path record the snapshot it takes for `DeletionPolicy: Snapshot` in that same state; then delete the file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line so the stub census stops counting it.

Traces: cloudformation.deep-cloudformation-2-snapshot-volume (verdict error). Code citations: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs line 4 (scaffold marker) and its `handle` body returning `Scaffolded::not_implemented("DescribeSnapshots")`.

## Evidence
Expected (AWS): ec2:DescribeSnapshots -> HTTP 200. Before the stack delete, OwnerIds=["self"] with Filters=[{Name:"volume-id",Values:[<vol id>]}] returns {"Snapshots": []}. After DeleteStack reaches DELETE_COMPLETE on a stack whose AWS::EC2::Volume has DeletionPolicy=Snapshot, the same call returns exactly one entry {SnapshotId, VolumeId: <the deleted volume id>, VolumeSize: 1, State != "error", Progress, StartTime, OwnerId, Tags}, and DescribeSnapshots with SnapshotIds=[that SnapshotId] returns the same VolumeId. DescribeVolumes for the deleted volume is refused with code InvalidVolume.NotFound (HTTP 400).

Actual (emulator): HTTP 501 with error code `NotImplemented` and message `operation DescribeSnapshots is scaffolded but not implemented in this emulator` on the FIRST DescribeSnapshots call, so the check never reaches the delete. Replay detail: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeSnapshots operation: operation DescribeSnapshots is scaffolded but not implemented in this emulator`. Message breakdown: the fixed text `operation ... is scaffolded but not implemented in this emulator` and the code `NotImplemented` are entirely emulator-authored (AWS returns neither); the only request-derived part is the operation name `DescribeSnapshots`.

Supporting source: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs:4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and `DescribeSnapshots::handle` returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeSnapshots"))`. Smithy models the operation with `errors: []` and a paginated Snapshots list, i.e. no refusal is modelled for a well-formed listing.

## Steps to Reproduce
1. Boot the emulator and call ec2:DescribeSnapshots with OwnerIds=["self"] and no other parameters. Expected: HTTP 200 with a (possibly empty) Snapshots list. Actual: HTTP 501 `NotImplemented`.
2. Full replay: ec2:DescribeAvailabilityZones to pick an available zone; cloudformation:CreateStack with a single resource `Vol` of type AWS::EC2::Volume, `DeletionPolicy: Snapshot`, Properties {AvailabilityZone: <zone>, Size: 1, VolumeType: gp3, Tags}; poll DescribeStacks by StackId to CREATE_COMPLETE; DescribeStackResource(LogicalResourceId="Vol") to get the `vol-` physical id; ec2:DescribeVolumes to confirm Size 1; ec2:DescribeSnapshots(OwnerIds=["self"], Filters=[{Name:"volume-id",Values:[<vol id>]}]) — AWS returns an empty list here, the emulator answers 501.
3. Continue past that point (once DescribeSnapshots answers): cloudformation:DeleteStack, poll to DELETE_COMPLETE, confirm ec2:DescribeVolumes on the id is refused InvalidVolume.NotFound, then ec2:DescribeSnapshots filtered on `volume-id` returns exactly one snapshot with VolumeId = the deleted volume and VolumeSize 1, and DescribeSnapshots by SnapshotIds returns the same VolumeId.
4. Re-replay check cloudformation.deep-cloudformation-2-snapshot-volume to confirm the fix.

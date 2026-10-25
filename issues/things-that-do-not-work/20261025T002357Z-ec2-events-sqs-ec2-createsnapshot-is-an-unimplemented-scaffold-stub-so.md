Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.events-ec2-lifecycle-ebs-snapshot-notification-carries-snapshot-state ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously
Issue-Key: ec2__events__sqs|CreateSnapshot|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EC2 CreateSnapshot is an unimplemented scaffold stub, so no EBS Snapshot Notification event can be produced and no asynchronous CopySnapshot failure can be observed (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshot

## Description
Cause 1 of 1 — EC2 `CreateSnapshot` answers NotImplemented.
AWS: `CreateSnapshot` takes VolumeId (plus Description/TagSpecifications) and returns the snapshot's SnapshotId, State (`pending`), VolumeId, VolumeSize, StartTime, OwnerId and Progress; the snapshot then settles to `completed`. Each transition publishes an `aws.ec2` event with detail-type "EBS Snapshot Notification" to the default event bus, whose `detail.state` carries the snapshot state and whose `detail.result`/`detail.cause` describe a failure. A CopySnapshot naming a source that does not exist fails ASYNCHRONOUSLY — the call is accepted and the failure surfaces as a snapshot in `error` state and the corresponding event, not as a synchronous refusal.
Emulator: `CreateSnapshot` answers HTTP 400 with error code `NotImplemented` and "operation CreateSnapshot is scaffolded but not implemented in this emulator" (EC2's query protocol wraps this as the request's Error code). `NotImplemented` is not a modeled EC2 error code.
Why: snapshots are the foundation of EBS backup, AMI creation and cross-region copy; none of that works. Downstream, no "EBS Snapshot Notification" event can ever be emitted, so a rule over snapshot lifecycle receives nothing, and the asynchronous-failure semantics of CopySnapshot — accepted call, failure reported only through the event/state — cannot be exercised at all. The client receives an error code that appears in no EC2 API reference and cannot classify or retry on it.
Fix: the CreateSnapshot handler, services/service-com-amazonaws-ec2-amazonec2/src/ops/create_snapshot.rs, the snapshot state it must create and settle, and the `aws.ec2` "EBS Snapshot Notification" publisher that must emit on each transition.
Traces: ec2__events__sqs.events-ec2-lifecycle-ebs-snapshot-notification-carries-snapshot-state, ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously.

## Evidence
Both checks fail with "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSnapshot operation: operation CreateSnapshot is scaffolded but not implemented in this emulator". Each had already created the VPC/subnet/volume and the EventBridge rule plus SQS target successfully. Expected: HTTP 200 with SnapshotId and State 'pending', the snapshot settling to 'completed', and an `aws.ec2` "EBS Snapshot Notification" event per transition.

## Steps to Reproduce
1. CreateVolume(AvailabilityZone=<az>, Size=1) -> 200.
2. CreateSnapshot(VolumeId=<vol>) -> HTTP 400 with code NotImplemented instead of 200 with SnapshotId/State='pending'.
3. CopySnapshot(SourceSnapshotId='snap-0000000000000dead', SourceRegion=<region>) -> the asynchronous-failure path cannot be reached because no snapshot can be created in the first place.

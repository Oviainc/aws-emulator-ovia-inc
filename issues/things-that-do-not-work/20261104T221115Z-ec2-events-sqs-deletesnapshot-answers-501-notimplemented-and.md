Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously ec2__events__sqs.events-ec2-lifecycle-ebs-snapshot-notification-carries-snapshot-state
Issue-Key: ec2__events__sqs|DeleteSnapshot|NotImplemented
Issue-Key: ec2__events__sqs.events-ec2-lifecycle-ebs-snapshot-notification-carries-snapshot-state|_drain#16|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# ec2__events__sqs: DeleteSnapshot answers 501 NotImplemented, and a completed EBS snapshot publishes no "EBS Snapshot Notification" event to EventBridge (2 causes)

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSnapshot

## Description
Two net-new root causes in the EC2 -> EventBridge -> SQS snapshot-lifecycle surface. Both are reached only after CreateSnapshot started answering, so neither is carried by an existing issue's replay key.

Cause 1 of 2: ec2 DeleteSnapshot is a scaffold stub and refuses every call. Failing check: ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously.
  AWS:      Smithy: ec2 DeleteSnapshot input is {SnapshotId (required), DryRun}; output is Unit (an empty ec2Query DeleteSnapshotResponse, HTTP 200). A snapshot id that does not exist is refused with HTTP 400 and error code InvalidSnapshot.NotFound, message "The snapshot '<snapshot-id>' does not exist." — fixed text "The snapshot '" / "' does not exist.", variable part the SnapshotId taken from the request. A snapshot that is still in use by a registered AMI is refused InvalidSnapshot.InUse. The check uses DeleteSnapshot to delete the source snapshot so that a subsequent CopySnapshot of the now-missing source fails asynchronously.
  Emulator: the call is refused outright: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteSnapshot operation: "operation DeleteSnapshot is scaffolded but not implemented in this emulator". Neither the success path nor the InvalidSnapshot.NotFound / InvalidSnapshot.InUse refusals exist, so the asynchronous CopySnapshot failure the check is about can never be provoked.
  Why:      a client cannot delete a snapshot at all; snapshot lifecycle/cleanup code and every negative path that depends on a deleted or missing source snapshot (CopySnapshot from a missing source, DLM/rbin flows, teardown in test harnesses) aborts on an error code AWS never returns and that no SDK retry or error handler expects.
  Fix:      the DeleteSnapshot operation handler in the EC2 service crate (services/service-com-amazonaws-ec2-amazonec2, ops/delete_snapshot) must be implemented against the snapshot state store rather than returning the scaffold NotImplemented, including the InvalidSnapshot.NotFound and InvalidSnapshot.InUse refusals.
  Related:  open issue 20261023T060504Z-ec2-deletesnapshot-stub.md names the same operation, and 20261025T002357Z-ec2-events-sqs-ec2-createsnapshot-is-an-unimplemented-scaffold-stub-so.md describes the downstream CopySnapshot-failure consequence while blaming CreateSnapshot (which now answers). Neither carries this check's replay key, so this check is still unowned.
  Traces:   ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously

Cause 2 of 2: a snapshot that reaches state `completed` emits no EBS Snapshot Notification event, so an EventBridge rule created BEFORE the snapshot never receives anything. Failing check: ec2__events__sqs.events-ec2-lifecycle-ebs-snapshot-notification-carries-snapshot-state.
  AWS:      recorded/documented behavior only — this is an eventing side effect, not modeled in the Smithy API. When a CreateSnapshot-initiated snapshot finishes, EC2/EBS publishes one event to the account's default event bus with source "aws.ec2" and detail-type "EBS Snapshot Notification". Its detail carries event="createSnapshot", result="succeeded", the snapshot's ARN in `snapshot_id`, the source volume's ARN in `source`, and `startTime`/`endTime` with startTime <= endTime; `resources` names the snapshot ARN. DescribeSnapshots reports that snapshot's State as `completed` over the same interval. All of those values are taken from the request and the emulator's own state (region, account, snapshot id, volume id, clock) — no literal id or timestamp from one run is part of the requirement.
  Emulator: nothing is ever published. The rule is created first, the snapshot completes, and the SQS target receives no message; the check is cut off at the 60 s harness cap ("Check timed out after 60.0s"). No EBS Snapshot Notification envelope of any shape reaches the bus.
  Why:      every snapshot-driven automation is dead: rules that fan a finished snapshot out to SQS/Lambda/SNS for copy, tagging, compliance or cost workflows never fire, and a client that waits on the event instead of polling DescribeSnapshots hangs forever rather than being told the snapshot is complete.
  Fix:      the EC2 snapshot state machine must publish the EBS Snapshot Notification envelope to the default event bus when a snapshot transitions to `completed` (and on the failure/copy variants) — i.e. the EC2 snapshot state transition code plus the EventBridge default-bus publish path, not the EventBridge rule matcher, which demonstrably matches events it is given.
  Traces:   ec2__events__sqs.events-ec2-lifecycle-ebs-snapshot-notification-carries-snapshot-state

## Evidence
Cause 1 — ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously [error]
  Expected: DeleteSnapshot returns HTTP 200 with an empty DeleteSnapshotResponse, and the snapshot is gone; a later CopySnapshot naming that source fails asynchronously.
  Actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteSnapshot operation: operation DeleteSnapshot is scaffolded but not implemented in this emulator

Cause 2 — ec2__events__sqs.events-ec2-lifecycle-ebs-snapshot-notification-carries-snapshot-state [mismatch]
  Expected: one message on the rule's SQS target whose envelope is source="aws.ec2", detail-type="EBS Snapshot Notification", detail.event="createSnapshot", detail.result="succeeded", detail.snapshot_id = the new snapshot's ARN, detail.source = the source volume's ARN, detail.startTime <= detail.endTime, resources = [snapshot ARN]; DescribeSnapshots reports State="completed".
  Actual:   no message ever arrives on the target queue. Error: Check timed out after 60.0s.

## Steps to Reproduce
Cause 1: create a volume and a snapshot of it with ec2 CreateSnapshot, wait for the snapshot to reach `completed`, then call ec2 DeleteSnapshot with that SnapshotId. Observe NotImplemented instead of an empty 200. Then call CopySnapshot naming the deleted snapshot as source and observe that the asynchronous failure path cannot be reached.

Cause 2: put an EventBridge rule on the default bus with pattern {"source":["aws.ec2"],"detail-type":["EBS Snapshot Notification"]} and an SQS queue target (queue policy granting events.amazonaws.com SendMessage). THEN create a volume and call ec2 CreateSnapshot on it and poll DescribeSnapshots until State=completed. Receive from the queue: no message arrives.

Replay: run_behavioral_suite and re-run the two checks listed in trace_ids.

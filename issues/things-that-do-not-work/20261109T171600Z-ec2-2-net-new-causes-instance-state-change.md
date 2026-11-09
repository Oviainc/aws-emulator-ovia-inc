Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.instance-state-change-notifications-are-delivered-per-state-to-their-own-rule ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously
Issue-Key: ec2__events__sqs.instance-state-change-notifications-are-delivered-per-state-to-their-own-rule|_collect#5|AssertionError|assert want <= seen, (
Issue-Key: ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters|run#31|AssertionError|assert "stopped" in tx_states, (
Issue-Key: ec2__events__sqs|CopySnapshot|InvalidParameterValue
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# EC2: 2 net-new causes — instance state-change notifications never reach EventBridge, and CopySnapshot from a deleted source is refused synchronously instead of failing asynchronously

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopySnapshot

## Description
Two independent defects grounded in three failing checks under the `ec2__events__sqs` heading.

Cause 1 of 2: `EC2 Instance State-change Notification` events are never published.
AWS: every instance state transition publishes an `EC2 Instance State-change Notification` event (source `aws.ec2`) onto the default bus with `detail.instance-id` and `detail.state`, one event per state entered. A rule whose pattern matches those states delivers each one to its targets, and a target with an InputTransformer receives the transformed payload while a sibling target whose queue rejects the delivery dead-letters it.
Emulator: nothing is delivered. A stop transition that must yield both `stopping` and `stopped` to the subscribed queue yields none within 420s (`only [] arrived`), and the transformed-delivery check waits 600s for target A to receive the transformed `stopped` notification and receives `[]`.
Why: a client driving automation off instance lifecycle (cleanup on stop, registration on run) gets nothing: no delivery, no dead letter, and no error explaining it.
Fix: the EC2 instance state machine's event emission — each state transition must publish the state-change notification onto the default bus so EventBridge's matching, input transformation and dead-letter paths run. Open issue 20261107T075601Z-ec2-events-sqs-ec2-eventbridge-volume-instance-lifecycle-notifications-never-arrive.md reports the same family for OTHER checks; it does not carry these two replay keys.
Traces: ec2__events__sqs.instance-state-change-notifications-are-delivered-per-state-to-their-own-rule; ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters.

Cause 2 of 2: CopySnapshot from a deleted source snapshot is refused synchronously.
AWS: `CopySnapshot` with a `SourceSnapshotId` that no longer exists is ACCEPTED — it returns HTTP 200 with a new `SnapshotId` — and the copy then fails asynchronously: the new snapshot settles in state `error` and an EBS snapshot notification reports the failure. The referential problem is reported through the snapshot's state, not through a synchronous error.
Emulator: the call is refused synchronously with error code `InvalidParameterValue` (a 400), so no snapshot is created and no asynchronous failure or notification is produced.
Why: a client that models the documented asynchronous contract (poll the returned SnapshotId, react to the `error` state / notification) gets a synchronous exception with the wrong code at request time, and the asynchronous failure path it depends on never exists.
Fix: the CopySnapshot handler — accept the request, mint the destination snapshot, and drive it to `error` asynchronously (with the corresponding notification) rather than validating the source synchronously.
Traces: ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously.

## Evidence
Cause 1: `AssertionError: the stop transition: the states ['stopped', 'stopping'] must all be delivered to '<queue-url>' within 420.0s; only [] arrived (0 messages for other instances were skipped). A target that never publishes state-change events fails exactly here`; `AssertionError: target A must receive the transformed 'stopped' notification for i-00000000000000001 within 600.0s; it received []`.
Cause 2: `AssertionError: CopySnapshot from deleted snap-00000000000000084 must be accepted and fail asynchronously; it was refused synchronously with 'InvalidParameterValue'`.

## Steps to Reproduce
1. Cause 1: run an instance, PutRule on the default bus matching `{"source":["aws.ec2"],"detail-type":["EC2 Instance State-change Notification"]}` with an SQS target (and a variant with an InputTransformer plus a rejecting sibling target with a DLQ), then stop the instance and poll the queue(s).
2. Cause 2: create a snapshot, delete it, then `ec2.copy_snapshot(SourceRegion=<r>, SourceSnapshotId=<deleted id>)` — expect HTTP 200 with a SnapshotId that later reaches state `error`; observe a synchronous `InvalidParameterValue`.
3. Replay the three checks in trace_ids.

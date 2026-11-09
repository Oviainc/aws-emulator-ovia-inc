Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sqs__sts.start-message-move-task-requires-composite-source-and-destination-permissions
Issue-Key: iam__sqs__sts.start-message-move-task-requires-composite-source-and-destination-permissions|expect_error#20|AssertionError|raise AssertionError(f"expected {code} but the call succeeded: {result!r}")
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# SQS StartMessageMoveTask is not authorized: the composite source and destination permissions are never required

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMessageMoveTask

## Description
Cause 1 of 1: StartMessageMoveTask is served without charging the permissions it needs on the source and destination queues, so a principal holding only part of them (or none) still starts a move task.

AWS: StartMessageMoveTask requires a composite grant — the caller must be permitted on the source (dead-letter) queue AND on the destination queue. A caller missing either side is refused with `AccessDenied` and AWS's standard no-policy wording naming the specific action and queue ARN it lacked (`User: <principal arn> is not authorized to perform: sqs:<Action> on resource: <queue arn> because no identity-based policy allows the sqs:<Action> action`); the variable parts are the principal, the action and the queue ARN from the request. Smithy: sqs#StartMessageMoveTask takes SourceArn and optional DestinationArn and returns TaskHandle, declaring the service's access-denied/unsupported-operation errors — the composite permission requirement is a service fact, recorded behavior only, and the recording shows the partial-grant caller refused.

Emulator: the call succeeds, returning a TaskHandle with HTTP 200.

Why: message redrive moves customer data between queues. With no authorization, a principal that may only read one queue can drain a dead-letter queue into a destination it was never granted — a data-movement and isolation defect. Because the fully-granted leg also succeeds, the client cannot distinguish an enforced policy from an inert one.

Fix: the StartMessageMoveTask handler's authorization step must evaluate the caller's permissions on both SourceArn and DestinationArn before starting the task.

Traces: iam__sqs__sts.start-message-move-task-requires-composite-source-and-destination-permissions.

## Evidence
Expected: AccessDenied for the caller missing part of the composite grant.
Actual: AssertionError: expected AccessDenied but the call succeeded: {'TaskHandle': 'eyJ0YXNrSWQiOi…', … 'HTTPStatusCode': 200}

## Steps to Reproduce
1. Create a source (dead-letter) queue and a destination queue.
2. Create a role granted on only one of them; AssumeRole.
3. Call StartMessageMoveTask with SourceArn/DestinationArn; observe a TaskHandle instead of AccessDenied.

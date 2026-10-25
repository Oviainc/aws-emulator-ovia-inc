Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered
Issue-Key: events__sqs|StartReplay|NotImplemented
Issue-Key: events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns|_collect#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Key: events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered|_collect#24|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Commit: 60e26e2c88da1b6d2b10e3e27220278d072c01c3

# EventBridge archive replay is a 501 scaffold stub: StartReplay answers NotImplemented, so no archived event is ever re-emitted to a bus

> _Note: this issue was identified against the code base at commit `60e26e2c88da1b6d2b10e3e27220278d072c01c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplay

## Description
One root cause, stated plainly: EventBridge's archive-replay path does not exist in the emulator because StartReplay is still an unimplemented scaffold handler. No open issue names StartReplay (the nearest open EventBridge issues, 20261025T002357Z-events-eventbridge-accepts-requests-aws-refuses-reserved-bus-names.md and 20261025T002357Z-events-eventbridge-target-delivery-never-completes-for-sns-step.md, are about request validation and target delivery, not archive replay), so this is net-new.

Cause 1 of 1: StartReplay is a 501 scaffold stub, so an archive can never be replayed.

AWS: Smithy (com.amazonaws.eventbridge#StartReplay) models input ReplayName (required, len 1..64, pattern ^[\.\-_A-Za-z0-9]+$), EventSourceArn (ArchiveArn, required), EventStartTime (required timestamp), EventEndTime (required timestamp) and Destination (ReplayDestination, required, carrying Arn and optional FilterArns); output ReplayArn, State (ReplayState: STARTING | RUNNING | COMPLETED | CANCELLING | CANCELLED | FAILED), StateReason and ReplayStartTime. Its modeled errors are exactly InternalException (HTTP 500), InvalidEventPatternException (400), LimitExceededException (400), ResourceAlreadyExistsException (400) and ResourceNotFoundException (400). The modeled documentation states: "Starts the specified replay. Events are not necessarily replayed in the exact same order that they were added to the archive. A replay processes events to replay based on the time in the event, and replays them using 1 minute intervals... You can use DescribeReplay to determine the progress of a replay." So a well-formed StartReplay is accepted with 200, the replay progresses to a terminal DescribeReplay State of COMPLETED, the archived events inside [EventStartTime, EventEndTime] are re-emitted onto Destination.Arn, routing is confined to Destination.FilterArns when that list is supplied, and each redelivered event carries a replay-name field that a live delivery does not.

Emulator: every StartReplay call is refused with HTTP 501 and error code NotImplemented, message "operation StartReplay is scaffolded but not implemented in this emulator" (an example of the stub text; the status and code are the requirement being broken). The handler body is the generated scaffold that returns crate::scaffolded::Scaffolded::not_implemented("StartReplay"). Because no replay job is ever created, nothing is ever re-emitted, DescribeReplay has no replay to report on, and neither FilterArns scoping nor the replay-name stamp can be observed. Note that NotImplemented/501 is not one of the five errors the model declares for this operation.

Why: a client that uses archive + replay for recovery, backfill or reprocessing (the documented EventBridge durability story) gets a hard refusal instead of a replay, and because 501/NotImplemented is unmodeled the SDK cannot map it onto any StartReplay failure it knows how to handle — it surfaces as an opaque client error rather than, for example, ResourceNotFoundException for a missing archive. Any consumer downstream of the destination bus silently receives nothing, so a backfill appears to have been requested and simply never happens.

Fix: the StartReplay handler, services/service-com-amazonaws-eventbridge-awsevents/src/ops/start_replay.rs (and the identically scaffolded sibling services/service-com-amazonaws-cloudwatchevents-awsevents/src/ops/start_replay.rs), plus the replay state/runtime it needs: create the replay record, validate EventSourceArn against an existing archive (ResourceNotFoundException otherwise) and ReplayName uniqueness (ResourceAlreadyExistsException), re-emit archived events in the requested time window onto Destination.Arn stamping replay-name, confine routing to Destination.FilterArns when present, and drive DescribeReplay's State through to COMPLETED with EventLastReplayedTime.

Traces: events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived and events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window both fail with the ClientError above at the start_replay call. events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns and events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered exercise the same path (FilterArns scoping and the replay-name stamp) and never complete; they are listed as corroborating examples of the same cause rather than as independent evidence, since their own polling budgets are longer than the suite's 60 s per-check cap.

## Evidence
Expected (AWS / Smithy com.amazonaws.eventbridge#StartReplay): HTTP 200 with ReplayArn and State in {STARTING, RUNNING}; DescribeReplay subsequently reports State=COMPLETED; the archived events in the window are re-emitted on Destination.Arn, only through the rules named in Destination.FilterArns, each carrying a replay-name field equal to the ReplayName. Refusals, when they happen, come from the modeled set: InvalidEventPatternException / LimitExceededException / ResourceAlreadyExistsException / ResourceNotFoundException (400) or InternalException (500).

Actual (emulator): HTTP 501, Error.Code = NotImplemented. Replay from events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartReplay operation: operation StartReplay is scaffolded but not implemented in this emulator
and identically from events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window. The handler at services/service-com-amazonaws-eventbridge-awsevents/src/ops/start_replay.rs still carries the generated marker "SCAFFOLDED, NOT IMPLEMENTED" (line 4) and its handle body ends in Err(crate::scaffolded::Scaffolded::not_implemented("StartReplay")) (line 187). No replay record is created, so DescribeReplay never reports a terminal state and no event is re-emitted. events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns and events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered do not complete at all (Error: Check timed out after 60.0s), consistent with the same missing replay runtime.

## Steps to Reproduce
1. Create a custom event bus and two rules on it with the same event pattern, each targeting its own SQS queue.
2. CreateArchive against the bus ARN with that event pattern and wait for the archive to be ENABLED.
3. PutEvents one matching event onto the bus (with an explicit Time in the past) and wait for DescribeArchive to report EventCount >= 1.
4. Call StartReplay with ReplayName set, EventSourceArn = the archive ARN, EventStartTime/EventEndTime bracketing the event's Time, and Destination = {Arn: <bus arn>, FilterArns: [<rule A arn>]}.
   Observed: HTTP 501, Error.Code = NotImplemented, message "operation StartReplay is scaffolded but not implemented in this emulator".
   Expected: HTTP 200 with ReplayArn and State; DescribeReplay then reaching COMPLETED.
5. Replay the suite checks: events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived, events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window, events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns, events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered.

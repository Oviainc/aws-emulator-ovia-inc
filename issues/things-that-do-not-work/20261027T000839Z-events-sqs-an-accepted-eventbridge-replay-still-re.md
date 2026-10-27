Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered
Issue-Key: events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns|_collect#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Key: events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered|_collect#24|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# events__sqs: an accepted EventBridge replay still re-emits nothing — Destination.FilterArns scoping and `replay-name` at pattern-matching time remain ungradeable (1 cause)

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplay

## Description
One net-new root cause. Cause count: 1.

Neither check is marked OWNED or ALREADY FILED in this replay report. What is already filed and does NOT cover them: 20261025T171339Z-events-sqs-2-net-new-causes-an-accepted.md names BOTH of these check ids in its Issue-Trace-Ids and is marked FIXED, yet both still fail with the identical signature (`Check timed out after 60.0s`, no replay delivery observed) — a resolved issue never covers a failure, so the re-emission path it described as the fix is still absent. 20261025T084539Z-events-sqs-archive-replay-is-unreachable-startreplay-is.md (fixed) and 20261025T002357Z-events-eventbridge-rule-state-connection-archive-api-destination-and.md (fixed) also name these ids but were written against StartReplay/CreateArchive answering NotImplemented, which is no longer the signature. The open issue 20261025T115007Z-events-sqs-an-accepted-startreplay-never-produces-an.md carries only the replay key of a DIFFERENT check (events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window) and says nothing about FilterArns scoping or about `replay-name` being matchable. 20261025T084539Z-events-eventbridge-archive-and-replay-control-plane-is-unimplemented.md (fixed) is about six control-plane stubs, not the re-emission path.

=== Cause 1 of 1: an accepted StartReplay never re-emits the archived events, so neither Destination.FilterArns scoping nor the presence of `replay-name` at pattern-matching time can be observed ===

AWS: Smithy (com.amazonaws.eventbridge#StartReplay) models required {ReplayName (@length 1..64, @pattern ^[\.\-_A-Za-z0-9]+$), EventSourceArn (an ArchiveArn), EventStartTime, EventEndTime, Destination (ReplayDestination{Arn, FilterArns})} and output {ReplayArn, State, StateReason, ReplayStartTime}; its errors are InternalException (500), InvalidEventPatternException, LimitExceededException, ResourceAlreadyExistsException and ResourceNotFoundException (400). DescribeReplay reports {State, StateReason, Destination, EventStartTime, EventEndTime, EventLastReplayedTime, ReplayStartTime, ReplayEndTime}, and the modeled documentation states a replay 'processes events to replay based on the time in the event, and replays them using 1 minute intervals' and that DescribeReplay is how progress is determined. So the behavior: the replay settles at State=COMPLETED; the archived events whose own envelope `time` lies inside [EventStartTime, EventEndTime] are re-emitted onto Destination.Arn; routing is CONFINED to the rule ARNs named in Destination.FilterArns, so a rule on the same bus whose pattern matches the same event but that is NOT in FilterArns receives nothing from the replay; DescribeReplay echoes Destination.FilterArns back unchanged; and each redelivered envelope carries a `replay-name` field equal to the ReplayName while a LIVE delivery carries no such key. `replay-name` is a real, matchable envelope field: a rule whose pattern requires `"replay-name": [{"exists": false}]` receives live events but must NOT receive replayed ones even when that rule IS named in FilterArns — i.e. the field must be present when the pattern engine evaluates the event, not stamped on afterwards during target serialization.
Emulator: both checks report `Check timed out after 60.0s` and never reach their replay assertions. Each had already succeeded at CreateEventBus, two PutRule+PutTargets pairs with SQS queues, the two delivery-path settles, CreateArchive, the ENABLED poll and the live PutEvents leg; nothing from a replay is ever observed arriving at the destination bus's rule targets, so the FilterArns scoping (rule A in, rule B out) and the `replay-name exists:false` opt-out (rule Y receives the replay, rule X must not) are both ungraded. StartReplay itself is no longer refused.
Why: archive + replay is EventBridge's documented backfill/recovery story, and Destination.FilterArns is the only way a consumer bounds which rules a backfill re-runs. A consumer that asks for a scoped backfill gets an accepted StartReplay and then nothing at all, so a failed recovery is indistinguishable from a successful one; and if re-emission is later added without the scoping, the same consumer gets duplicated side effects on rules it deliberately excluded. Because `replay-name` is never observed, the documented opt-out that stops a consumer re-processing history cannot be relied on at all.
Fix: the replay re-emission path in the eventbridge service crate — the StartReplay handler plus the replay progression/settling in crate::state, the window selection over the archive, the FilterArns restriction applied when choosing which rules the re-emitted event is evaluated against, and the point at which `replay-name` is attached to the envelope (BEFORE pattern evaluation, not during target serialization) — and the rule->SQS target delivery branch each replayed event is handed to.
Traces: events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns, events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered.

## Evidence
Cause 1, both checks [mismatch] with `Error: Check timed out after 60.0s`.

events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns:
  Expected: on one custom bus with two rules A and B carrying the identical pattern {"source":[S]} and one SQS queue each, both delivery paths settled before the archive exists, an ENABLED archive over the same pattern, one PutEvents entry with Time=now-1h (FailedEntryCount 0) whose LIVE copy reaches both queues with NO `replay-name` key; then start_replay(Destination={'Arn': <bus arn>, 'FilterArns': [<rule A arn>]}) settles at State='COMPLETED' with DescribeReplay reporting Destination.FilterArns == [<rule A arn>]; rule A's queue receives the event carrying replay-name == <ReplayName>; rule B's queue receives NOTHING over a 60 s hold anchored on A's receipt; and a fresh live PutEvents afterwards still reaches rule B, proving B's path was alive throughout.
  Actual: no replay delivery is observed on either queue; the check never reaches the FilterArns assertions.

events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered:
  Expected: rules X (pattern {"source":[S],"replay-name":[{"exists": false}]}) and Y (pattern {"source":[S]}) BOTH named in Destination.FilterArns; the live event reaches X and Y and neither live body has a `replay-name` key; the replay reaches Y carrying replay-name == <ReplayName>; X's queue receives nothing from the replay; a fresh live event reaches X again afterwards.
  Actual: nothing from the replay arrives at Y, so X's required silence proves nothing and the check times out.

What the 60 s cap does and does not establish: the harness caps each check at 60 s while these checks' own documented budgets are far longer (180-200 s delivery waits, 1500 s archive-ingest, 400 s replay-settle), so the cap alone does not localise the stall to a single call. What it does establish is that on this emulator no replay redelivery reaches a target queue inside the cap, and that the live-delivery leg these checks assert on first is the only thing between them and the replay assertions. Per-run values (generated bus/rule/archive/replay names, nonces, the account id and timestamps) are not part of the requirement; the COMPLETED settle, the FilterArns echo, the confinement to the named rules and the presence of `replay-name` at matching time are.

## Steps to Reproduce
FilterArns scoping:
1. events.create_event_bus(Name=B) -> note EventBusArn.
2. Create SQS queues QA and QB; events.put_rule(Name=A, EventBusName=B, EventPattern='{"source":["S"]}') with QA as its only target, and events.put_rule(Name=RB, EventBusName=B, same pattern) with QB as its only target. Drive warm-up PutEvents until BOTH queues deliver twice consecutively (settle the paths before the archive exists).
3. events.create_archive(ArchiveName=ARC, EventSourceArn=<B's arn>, EventPattern='{"source":["S"]}'); poll describe_archive until State='ENABLED'; drain QA and QB.
4. events.put_events(Entries=[{EventBusName:B, Source:'S', DetailType:'Replayable', Time: now-1h, Detail:'{"nonce":"N"}'}]) -> FailedEntryCount 0; confirm the LIVE copy reaches QA and QB and that neither body has a `replay-name` key.
5. Poll describe_archive until EventCount>=1 and SizeBytes>0; drain QA and QB.
6. events.start_replay(ReplayName=R, EventSourceArn=<ARC's arn>, EventStartTime=now-1h-5m, EventEndTime=now-1h+5m, Destination={'Arn': <B's arn>, 'FilterArns': [<A's rule arn>]}).
7. Poll describe_replay(R) to a terminal state -> expect 'COMPLETED' with Destination.FilterArns == [<A's rule arn>]; read QA -> expect the event with replay-name == R. Observed: nothing arrives on QA and the check times out at the 60 s cap.
8. (Then) read QB for 60 s -> must be empty; one more live put_events -> must reach QB.

replay-name matchability:
1. Same bus; rule X with EventPattern '{"source":["S"],"replay-name":[{"exists":false}]}' -> QX, rule Y with '{"source":["S"]}' -> QY; settle both.
2. Archive + put_events(Time=now-1h) + ingest as above; events.start_replay with Destination={'Arn': <B's arn>, 'FilterArns': [<X arn>, <Y arn>]}.
3. Expect: QY receives the event with replay-name == R; QX receives nothing; a fresh live event then reaches QX. Observed: QY receives nothing and the check times out.

Replay the two check ids in trace_ids to confirm a fix.

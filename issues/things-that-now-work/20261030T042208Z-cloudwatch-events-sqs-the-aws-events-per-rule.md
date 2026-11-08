Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied
Issue-Key: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied|_poll_until#50|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 6d7340cf17b489ade57e03c787c898551b69c6c6

# cloudwatch__events__sqs: the AWS/Events per-rule counters (TriggeredRules, SuccessfulInvocationAttempts, FailedInvocations, InvocationsSentToDlq) are never published, so a bus-to-bus forward denied by the destination bus policy cannot be told apart from an event that was never sent

> _Note: this issue was identified against the code base at commit `6d7340cf17b489ade57e03c787c898551b69c6c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause. The failing check was reported neither ALREADY FILED nor OWNED by an open issue, so no open work item carries this run's replay key for it.

NOTE ON RELATED OPEN ISSUES (neither covers this failure): 20261029T014025Z-cloudwatch-events-sqs-a-bus-to-bus-forward.md names this same check, but it was filed against an EARLIER failure signature (`_drain#34|Timeout|resp = sqs.receive_message(`) in which the forward could not even be ESTABLISHED — no forwarded event ever reached the destination bus's queue. In this replay the forwarding path itself now works: the destination-bus rule+target, the source-bus control rule, the forwarding role, the PutPermission Deny and DescribeEventBus round-trip, the control delivery of the denied event, the 10s absence hold on the destination consumer and its zero queue depth ALL pass. The failure has moved to a later, narrower step, with a different signature (`_poll_until#45|Timeout`). 20261029T182228Z-cloudwatch-events-cloudwatch-publishes-no-eventbridge-rule-metrics-matchedevents-triggeredrules.md describes the same missing AWS/Events metric family but is grounded only in cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule and does not carry this check's key, and it scopes itself to MatchedEvents/TriggeredRules on a plain matching rule, not to the invocation-accounting series this check reads.

=== Cause 1 of 1: no AWS/Events per-rule datapoint is ever published, so the rule-invocation accounting for a refused event-bus forward is unobservable ===
AWS: Smithy (cloudwatch) GetMetricStatistics — input Namespace (required, @length 1..255, @pattern ^[^:]), MetricName (required, @length 1..255), Dimensions (@length 0..30), StartTime/EndTime/Period (required), Statistics (@length 1..5); output Label + Datapoints; modelled errors InternalServiceFault (500 InternalServiceError), InvalidParameterCombinationException (400 InvalidParameterCombination), InvalidParameterValueException (400 InvalidParameterValue), MissingRequiredParameterException (400 MissingParameter). The metric vocabulary itself is not in any Smithy model, so for the metric semantics this is recorded behavior only: EventBridge publishes per-rule series in namespace AWS/Events under the dimensions {EventBusName, RuleName} (and {RuleName} alone). When a rule matches an event, TriggeredRules for that rule gets a non-zero Sum — the rule triggered even if the target refused it. For a target that was accepted and delivered, SuccessfulInvocationAttempts for that rule gets a non-zero Sum. For a same-account event-bus target whose DESTINATION bus resource policy Denies the forwarding role, the recording shows FailedInvocations is NOT incremented (Sum 0 over 0 datapoints — the series emits nothing) and InvocationsSentToDlq is NOT incremented either, although the event genuinely reaches the DeadLetterConfig queue. The three observations together are what distinguish "the destination POLICY refused it" from "nothing was ever sent".
Emulator: GetMetricStatistics on Namespace=AWS/Events, MetricName=TriggeredRules, Dimensions=[{EventBusName,<source bus>},{RuleName,<forwarding rule>}] returns an empty Datapoints list forever. The check polls that read for up to 300s and ends with `TimeoutError: AWS/Events TriggeredRules for <forwarding rule> shows the rule MATCHED the denied event — without this, 'nothing arrived' is indistinguishable from 'nothing was ever sent': the run budget ended first; not graded` (check_cloudwatch__events__sqs_failed_invocations_records_the_bus_forwarding_the_destination_policy_denied.py line 428 -> _poll_until line 45). Because that read never converges, the three graded observations after it are never reached either: the NO_PERMISSIONS dead letter, FailedInvocations == (0 Sum, 0 datapoints), InvocationsSentToDlq == 0, and the CONTROL rule's SuccessfulInvocationAttempts >= 1. The in-check positive control the emulator DOES satisfy (the control rule delivered the denied event to its SQS queue, the destination consumer stayed empty and reported depth 0) proves the matching and the policy enforcement happened — only the metric emission is missing.
Why: a client that watches the AWS/Events per-rule counters to operate a cross-bus fan-out is blind. TriggeredRules is the only signal that says "the rule matched, the delivery was refused"; with no datapoint, a mis-scoped destination-bus policy is indistinguishable from a producer that stopped publishing, and an alarm on these series sits in INSUFFICIENT_DATA forever instead of firing. Equally, the deliberate AWS distinction that a permissions refusal does NOT raise FailedInvocations cannot be relied on, because a client cannot tell an intentionally-withheld zero from a store that publishes nothing at all.
Fix: the EventBridge rule target-delivery path must publish its per-rule AWS/Events telemetry into the CloudWatch metric store — TriggeredRules (and MatchedEvents) on every pattern match under the {EventBusName, RuleName} dimension set, SuccessfulInvocationAttempts on an accepted delivery, and the deliberate non-emission of FailedInvocations / InvocationsSentToDlq for a same-account event-bus target refused by the destination bus policy — so that CloudWatch GetMetricStatistics/ListMetrics can serve those series.
Traces: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied

## Evidence
From this behavioral replay (416/822 passed; cloudwatch__events__sqs 0/5, and this is the one of the five the suite marked neither ALREADY FILED nor OWNED).

cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied [error]:
  File "check_cloudwatch__events__sqs_failed_invocations_records_the_bus_forwarding_the_destination_policy_denied.py", line 428, in run
    triggered, _ = _poll_until(
  File "check_cloudwatch__events__sqs_failed_invocations_records_the_bus_forwarding_the_destination_policy_denied.py", line 45, in _poll_until
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: AWS/Events TriggeredRules for sim2real-nd-fwd-rule-<id> shows the rule MATCHED the denied event — without this, 'nothing arrived' is indistinguishable from 'nothing was ever sent': the run budget ended first; not graded

(The rule name in that message is a per-run generated name, not a requirement; the requirement is that SOME datapoint exists for the forwarding rule's RuleName dimension.)

Steps that PASSED before the failing read, establishing that the gap is the metric emission and not the forwarding path:
  - CreateEventBus (source) and CreateEventBus (destination); three SQS queues (control / destination consumer / DLQ) with policies allowing events.amazonaws.com sqs:SendMessage under ArnLike aws:SourceArn.
  - PutRule + PutTargets on the destination bus; PutRule + PutTargets (control queue) on the source bus; IAM role trusting events.amazonaws.com with events:PutEvents on the destination bus ARN; PutRule + PutTargets {Arn: <dst bus ARN>, RoleArn, DeadLetterConfig} on the source bus. All FailedEntryCount == 0.
  - The forward was ESTABLISHED (an emitted event reached the destination consumer) — this is the step the earlier issue 20261029T014025Z-cloudwatch-events-sqs-a-bus-to-bus-forward.md timed out on; it no longer times out.
  - PutPermission attached the Deny and DescribeEventBus returned a Policy containing the Deny Sid.
  - PutEvents on the source bus returned FailedEntryCount == 0; the CONTROL rule delivered the denied nonce to its queue.
  - The destination consumer received nothing over a 10s hold and GetQueueAttributes reported depth 0.

Expected vs actual at the failing read:
  Expected: GetMetricStatistics(Namespace="AWS/Events", MetricName="TriggeredRules", Dimensions=[{Name:"EventBusName",Value:<source bus>},{Name:"RuleName",Value:<forwarding rule>}], Period=60, Statistics=["Sum"]) returns at least one Datapoint with Sum >= 1.
  Actual: Datapoints is empty on every poll for the whole 300s window; the series does not exist. ListMetrics likewise reports no AWS/Events series for that rule.
  Also never reachable as a consequence: a single DLQ message whose MessageAttributes carry ERROR_CODE = "NO_PERMISSIONS"; FailedInvocations for the forwarding rule summing to 0 over 0 datapoints; InvocationsSentToDlq summing to 0; SuccessfulInvocationAttempts >= 1 for the control rule.

## Steps to Reproduce
1. CreateEventBus <src>; CreateEventBus <dst> (note its EventBusArn).
2. Create three SQS queues (control, destination consumer, DLQ), each with a policy allowing Principal {Service: events.amazonaws.com} sqs:SendMessage with Condition ArnLike aws:SourceArn = arn:*:events:<region>:<account>:rule/*.
3. On <dst>: PutRule with pattern {"source": ["sim2real.nd.busdeny"], "detail-type": ["BusForwardProbe"]}, State=ENABLED; PutTargets the destination consumer queue.
4. On <src>: PutRule with the same pattern; PutTargets the control queue (the twin control).
5. Create an IAM role trusting events.amazonaws.com with an inline policy allowing events:PutEvents on <dst>'s bus ARN. On <src>: PutRule with the same pattern; PutTargets {Arn: <dst> bus ARN, RoleArn: that role, DeadLetterConfig: {Arn: the DLQ ARN}}.
6. PutEvents on <src> with that source/detail-type until an event reaches the destination consumer queue (establish the forward).
7. PutPermission on <dst> with a statement Denying that forwarding role events:PutEvents; confirm via DescribeEventBus that the Deny Sid is in the returned Policy.
8. PutEvents on <src> again with a fresh nonce. Observe: the control queue receives it; the destination consumer stays empty with depth 0 (the emulator gets this far).
9. Call cloudwatch GetMetricStatistics(Namespace="AWS/Events", MetricName="TriggeredRules", Dimensions=[{EventBusName: <src>}, {RuleName: <forwarding rule>}], StartTime=now-1m, EndTime=now, Period=60, Statistics=["Sum"]).
   Expected: at least one Datapoint with Sum >= 1.
   Actual: Datapoints == [] indefinitely.
10. Also read AWS/Events FailedInvocations and InvocationsSentToDlq for the forwarding rule (expected: Sum 0 over 0 datapoints) and SuccessfulInvocationAttempts for the control rule (expected: Sum >= 1); none of these series exists either. Receive from the DLQ: expected exactly one message whose body detail.nonce equals the denied nonce and whose MessageAttributes carry ERROR_CODE = "NO_PERMISSIONS".

## Resolution
The code base drifted past this report: the AWS/Events per-rule telemetry the failing read asks for
is published and served now. The routing counters (`MatchedEvents`, `TriggeredRules`) are counted on
every pattern match, whatever became of the handover, and the handover counters (`Invocations`,
`SuccessfulInvocationAttempts`, `FailedInvocations`) on what each target's API call answered, all into
the CloudWatch store of the same engine, where `GetMetricStatistics` and `ListMetrics` serve them. A
forward onto another event bus is a publication onto that bus rather than an invocation of a target's
own API, so a refused one is dead-lettered with its condition and counted on neither
`FailedInvocations` nor `InvocationsSentToDlq` — those series are not published at all, which is the
observation the recording makes.

What was missing was a test over the WHOLE read this check makes, so the three observations cannot
regress apart: `services/service-com-amazonaws-eventbridge-awsevents/tests/all/bus_forward_refusal_telemetry.rs`
pins, over the wire through the router, the forwarding rule's `TriggeredRules`/`MatchedEvents`
datapoints, the twin control rule's `Invocations`/`SuccessfulInvocationAttempts` under both dimension
sets, and the absence of any `FailedInvocations`/`InvocationsSentToDlq` datapoint or series for the
withheld forward — read both over the aligned 60-second period ending now and over a window around
now, across a table that varies the round's event count, whether the forwarding target carries a
dead-letter queue at all, and whether the forward is withheld.

Replay: the recorded unit
`cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied`
passes.

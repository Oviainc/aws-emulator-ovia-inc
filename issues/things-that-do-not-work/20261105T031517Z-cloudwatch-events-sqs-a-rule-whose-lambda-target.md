Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed
Issue-Key: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed|_poll_until#52|Timeout|time.sleep(interval_s)
Issue-Commit: 8de6682f074d8bd25eca3d1022f82b7ad69da0d3

# cloudwatch__events__sqs: a rule whose Lambda target has no invoke grant never dead-letters to its DeadLetterConfig queue, and AWS/Events InvocationsSentToDlq is never published, so per-rule DLQ attribution never reaches a verdict (1 cause)

> _Note: this issue was identified against the code base at commit `8de6682f074d8bd25eca3d1022f82b7ad69da0d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause. This is the only failing check in this replay that the suite marked neither ALREADY FILED nor OWNED by an open issue, so no open work item carries its replay key.

NOTE ON RELATED OPEN ISSUES (none of them covers this failure):
- 20261030T042208Z-cloudwatch-events-sqs-the-aws-events-per-rule.md describes the same missing AWS/Events per-rule counter family, but it is grounded only in cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied (an event-bus-to-event-bus forward refused by the DESTINATION bus resource policy) and does not carry this check's key. Its requirement on InvocationsSentToDlq is the deliberate NON-emission (Sum 0 over 0 datapoints) for a refused bus target; the check here requires the opposite — the POSITIVE emission of InvocationsSentToDlq for a Lambda target that had no invoke grant, attributed to the failing rule and withheld from its healthy twin. Fixing one does not satisfy the other.
- 20261029T182228Z-cloudwatch-events-sqs-eventbridge-bus-to-bus-forwarding-ignores-the-destination.md also names InvocationsSentToDLQ, but only within the bus-to-bus forwarding scenario.
- 20261101T210635Z-events-lambda-sqs-6-net-new-causes-eventbridge.md ("EventBridge never dead-letters a failed Lambda delivery") is filed under the `events` service heading and is not grounded in this check.

=== Cause 1 of 1: a failed EventBridge target delivery is neither dead-lettered with its attribution attributes nor accounted for in the AWS/Events per-rule InvocationsSentToDlq series, so the scenario never produces an observable and the check exhausts its budget ===
Failing check ids for this cause: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed

AWS: Smithy (eventbridge) Target carries `DeadLetterConfig` (structure, member Arn -> ResourceArn @length 1..1600, documented as "The ARN of the SQS queue specified as the target for the dead-letter queue") and `RetryPolicy` (MaximumRetryAttempts @range 0..185, MaximumEventAgeInSeconds @range 60..86400), and the Target doc points at the EventBridge dead-letter-queue guide. Smithy (cloudwatch) GetMetricStatistics — input Namespace (required, @length 1..255, @pattern ^[^:]), MetricName (required, @length 1..255), Dimensions (@length 0..30), StartTime/EndTime/Period (required), Statistics (@length 1..5); output Label + Datapoints; modelled errors InternalServiceFault (500 InternalServiceError), InvalidParameterCombinationException (400 InvalidParameterCombination), InvalidParameterValueException (400 InvalidParameterValue), MissingRequiredParameterException (400 MissingParameter). The metric vocabulary and the DLQ message-attribute set are in no Smithy model, so for those this is recorded behavior only. The recording shows: one PutEvents entry fanned out to two rules on the same custom bus with the same pattern and the SAME shared DLQ; the rule whose SQS target is permitted delivers the envelope; the rule whose Lambda target carries no resource policy cannot be invoked and — per the EventBridge docs, a permissions failure is not retried — dead-letters immediately. The shared DLQ then holds EXACTLY ONE message, whose body is byte-identical to the envelope the healthy twin received on every envelope key (id, version, time, region, account, source, detail-type, detail) because both are the same fanned-out entry, and whose MessageAttributes carry three attributions: RULE_ARN = the ARN of the FAILING rule, TARGET_ARN = the ARN of the target that could not be invoked, and ERROR_CODE whose fixed text is exactly "NO_PERMISSIONS" for a target with no invoke grant. In the same window AWS/Events publishes, under the dimension set {EventBusName, RuleName}, a non-zero Sum for InvocationsSentToDlq against the FAILING rule only (both documented spellings "InvocationsSentToDlq" and "InvocationsSentToDLQ" are probed because two AWS pages disagree), a non-zero Sum for SuccessfulInvocationAttempts against the healthy rule, and NO sent-to-DLQ datapoint at all for the healthy rule — a shared DLQ must not move the healthy rule's counters. Finally, once the Lambda invoke grant is added, the identical call succeeds and nothing further is dead-lettered. In the DLQ attributes above, "NO_PERMISSIONS" and the attribute names RULE_ARN / TARGET_ARN / ERROR_CODE are the fixed text; the rule ARN, target/function ARN, bus name, rule names and the nonce are values taken from the request and the emulator's state and are never part of the template.
Emulator: the check never reaches any of those observables. It ends with `Check timed out after 60.0s` — the run budget is exhausted with no verdict, even though the check's own internal waits are generous (180 s for the healthy SQS delivery, 180 s for the dead letter, 300 s for the AWS/Events read, 240 s for the post-repair invocation). The emulator therefore served none of the awaited side effects inside the harness budget: no dead-lettered copy with RULE_ARN / TARGET_ARN / ERROR_CODE="NO_PERMISSIONS", and no AWS/Events datapoint under either spelling of the sent-to-DLQ metric. Hypothesis: the EventBridge target-delivery path performs no invoke-permission evaluation and has no dead-letter leg, and publishes no AWS/Events per-rule telemetry into the CloudWatch metric store, so every poll in the check reads an empty queue and an empty Datapoints list; this is consistent with the already-filed cloudwatch__events and cloudwatch__events__sqs counter findings above, but the suite's output for this check carries only the budget-exhaustion frame and no per-step frame, so the exact leg that first stalls is not established by this replay.
Why: a client that operates an EventBridge fan-out relies on exactly these two records to tell a refused delivery apart from an event that was never produced. With no dead-lettered copy the event is silently lost instead of parked for inspection, and the RULE_ARN / TARGET_ARN / ERROR_CODE attributes are the only thing that makes a DLQ shared between several rules diagnostic at all — without them an operator cannot tell which rule or which target failed, or that the cause was a missing invoke grant rather than a throttle or a timeout. With no InvocationsSentToDlq datapoint, any alarm on non-delivery sits in INSUFFICIENT_DATA forever rather than firing, and the deliberate asymmetry AWS guarantees (the healthy rule's counters do not move when it shares a DLQ with a failing rule) cannot be relied on, because an intentionally-withheld zero is indistinguishable from a store that publishes nothing.
Fix: the EventBridge rule target-delivery path (the target invoker and its failure handling) must evaluate the target's invoke permission, and on a permissions failure send the unmodified event envelope once to the target's DeadLetterConfig queue — no retries — with the RULE_ARN, TARGET_ARN and ERROR_CODE message attributes set from the failing rule and target; and that same path must publish its per-rule AWS/Events telemetry into the CloudWatch metric store under the {EventBusName, RuleName} dimension set (InvocationsSentToDlq on a dead-lettered delivery, SuccessfulInvocationAttempts on an accepted one, and nothing for a rule whose delivery succeeded) so that CloudWatch GetMetricStatistics and ListMetrics can serve those series. The dead-lettered copy must be the carried envelope, not a re-synthesised one, so its id and time match the copy the healthy twin received.
Traces: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed

## Evidence
From this behavioral replay (520/924 passed, 56.3%; cloudwatch__events__sqs 1/5 with 4 failed, and this is the one of the four the suite marked neither ALREADY FILED nor OWNED).

cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed [mismatch]:
  Error: Check timed out after 60.0s

The check's own budgets are far larger than the 60 s harness budget it exhausted (healthy SQS delivery 180 s, dead letter 180 s, AWS/Events sent-to-DLQ read 300 s, post-repair invocation 240 s), so the emulator produced none of the awaited side effects; the suite reports no per-step frame, so the first stalling leg is not pinned down by this replay.

Expected vs actual, per graded observable of the scenario (one PutEvents entry, two rules on one custom bus with the same pattern, one SHARED DLQ):
  1. Healthy twin — Expected: the permitted SQS target receives exactly one message whose body's detail.nonce is the published nonce and which carries a service-generated `id`. Actual: not reached within budget.
  2. Dead letter — Expected: the shared DLQ holds EXACTLY ONE message (two would mean the healthy rule also dead-lettered a success). Actual: not reached within budget.
  3. DLQ attribution — Expected MessageAttributes on that copy: RULE_ARN = the FAILING rule's ARN (not the healthy rule's), TARGET_ARN = the Lambda function ARN that could not be invoked, ERROR_CODE = "NO_PERMISSIONS" (fixed text). Actual: no dead-lettered copy, therefore no attributes.
  4. Envelope identity — Expected: the dead-lettered body equals the healthy twin's body on every key of (id, version, time, region, account, source, detail-type, detail), because both are the same fanned-out entry. Actual: not reached.
  5. Absence hold — Expected: no further message reaches the shared DLQ during a hold of 2x the measured delivery latency (floor 10 s), and GetQueueAttributes reports total depth 0 behind the empty receive. Actual: not reached.
  6. Service accounting (the read this check is named for) — Expected: cloudwatch GetMetricStatistics(Namespace="AWS/Events", MetricName in ("InvocationsSentToDlq","InvocationsSentToDLQ"), Dimensions=[{EventBusName: <bus>},{RuleName: <failing rule>}], Period=60, Statistics=["Sum"]) returns at least one Datapoint with Sum >= 1 under at least one spelling. Actual: no such series is ever served.
  7. Positive control for 6 — Expected: SuccessfulInvocationAttempts for the HEALTHY rule under the same dimension set sums to >= 1 (without this, "no datapoints" for the healthy rule would be ambiguous). Actual: no such series is ever served.
  8. Withheld counter — Expected: the HEALTHY rule's sent-to-DLQ Sum is 0 under BOTH spellings — sharing a DLQ must not move its counters. Actual: not gradeable, because nothing is published for either rule.
  9. Repair leg — Expected: after the Lambda invoke grant is added, the identical call succeeds, the function processes exactly the newly published nonce (the dead-lettered event is NOT replayed), and nothing further reaches the DLQ. Actual: not reached.

Bus names, queue names, rule names, function names, ARNs, the generated envelope `id`/`time` and the per-run nonce in the messages above are per-run values, not requirements; the requirements are the exactly-one-copy count, the three attribution attributes with ERROR_CODE's fixed text "NO_PERMISSIONS", the envelope-key identity between the two copies, and the per-rule presence/absence of the AWS/Events Sums.

## Steps to Reproduce
1. events CreateEventBus <bus> (custom bus).
2. Create three SQS queues — `ok`, `dlq`, `result` — each with a Policy allowing Principal {Service: events.amazonaws.com} (for `result`: lambda.amazonaws.com) sqs:SendMessage on its own ARN with Condition ArnLike aws:SourceArn = arn:*:events:<region>:<account>:rule/<bus>/* (for `result`: arn:*:lambda:<region>:<account>:function:*). Note each QueueArn via GetQueueAttributes.
3. Create an IAM role for Lambda and lambda CreateFunction <fn> with env RESULT_QUEUE_URL=<result queue url>; wait for State == Active. Confirm with GetPolicy that the function carries NO resource policy — this missing invoke grant is the structural break under test.
4. events PutRule <ok-rule> on <bus> with EventPattern {"source":["sim2real.nd.dlqattrib"],"detail-type":["DlqAttributionProbe"]}, State=ENABLED; PutTargets [{Id:"t", Arn:<ok queue ARN>, DeadLetterConfig:{Arn:<dlq ARN>}}]. Assert FailedEntryCount == 0.
5. events PutRule <bad-rule> on <bus> with the SAME pattern; PutTargets [{Id:"t", Arn:<fn ARN>, RetryPolicy:{MaximumRetryAttempts:0, MaximumEventAgeInSeconds:60}, DeadLetterConfig:{Arn:<dlq ARN>}}]. Assert FailedEntryCount == 0. Both rules now share one DLQ.
6. events PutEvents one entry on <bus> with that Source/DetailType and Detail {"nonce":"<nonce>"}. Assert FailedEntryCount == 0.
7. ReceiveMessage on the `ok` queue.
   Expected: exactly one message, body detail.nonce == <nonce>, body has a non-empty `id`.
8. ReceiveMessage on the `dlq` queue with MessageAttributeNames=["All"].
   Expected: exactly one message; MessageAttributes RULE_ARN == <bad-rule ARN>, TARGET_ARN == <fn ARN>, ERROR_CODE == "NO_PERMISSIONS"; its body equals the step-7 body on every key of (id, version, time, region, account, source, detail-type, detail).
   Actual: nothing is dead-lettered.
9. Hold the `dlq` queue for 2x the measured step-7 latency (floor 10 s) draining it.
   Expected: no further message, and GetQueueAttributes(ApproximateNumberOfMessages + NotVisible + Delayed) == 0.
10. cloudwatch GetMetricStatistics(Namespace="AWS/Events", MetricName="InvocationsSentToDlq" and again "InvocationsSentToDLQ", Dimensions=[{Name:"EventBusName",Value:<bus>},{Name:"RuleName",Value:<bad-rule>}], StartTime=<before step 6>, EndTime=now+2m, Period=60, Statistics=["Sum"]).
    Expected: at least one Datapoint with Sum >= 1 under at least one spelling.
    Actual: Datapoints == [] for both spellings, indefinitely.
11. Same call for MetricName="SuccessfulInvocationAttempts" with RuleName=<ok-rule>.
    Expected: Sum >= 1. Actual: Datapoints == [].
12. Same call as step 10 but with RuleName=<ok-rule>.
    Expected: Sum == 0 under both spellings (the shared DLQ must not move the healthy rule's counters).
13. lambda AddPermission granting events.amazonaws.com lambda:InvokeFunction on <fn> with SourceArn <bad-rule ARN>; PutEvents a fresh nonce.
    Expected: the function runs and writes exactly that new nonce to the `result` queue, the previously dead-lettered event is NOT replayed, and nothing further reaches the `dlq`.
    Actual: not reached; the whole check exhausts the 60 s budget with no verdict.

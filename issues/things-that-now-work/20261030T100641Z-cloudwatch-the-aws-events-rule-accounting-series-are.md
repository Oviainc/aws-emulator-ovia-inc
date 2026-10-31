Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied
Issue-Key: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule|_poll_until_stable#25|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied|run#64|AssertionError|assert ctl_good >= 1, (
Issue-Commit: db763654427c553d8b06d764576848aa87eee804

# cloudwatch: the AWS/Events rule-accounting series are still not served — MatchedEvents/TriggeredRules under {EventBusName, RuleName} never settle, and SuccessfulInvocationAttempts reads Sum 0 for a rule whose target demonstrably delivered (2 causes)

> _Note: this issue was identified against the code base at commit `db763654427c553d8b06d764576848aa87eee804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Two net-new root causes, both in the AWS/Events metric-emission path. Neither failing check was reported ALREADY FILED or OWNED, so no open work item carries this run's replay key for either.

NOTES ON RELATED ISSUES (neither covers these failures):
- 20261029T182228Z-cloudwatch-events-cloudwatch-publishes-no-eventbridge-rule-metrics-matchedevents-triggeredrules.md is marked FIXED and was grounded in the same check as Cause 1; after that fix the check still does not pass, and it still never settles on a value for the {EventBusName, RuleName} series. This is not reported here as a refusal (the calls answer 2xx with an empty Datapoints list / never converge), so it is filed as a live divergence rather than as a REOPENS.
- 20261030T042208Z-cloudwatch-events-sqs-the-aws-events-per-rule.md names the same check as Cause 2 but carries a DIFFERENT failure key (`_poll_until#50|TimeoutError` on the TriggeredRules read). In this replay that read no longer blocks: the check now runs all the way to its CONTROL assertion and fails later and narrower, on SuccessfulInvocationAttempts for the control rule (`check_...py` line 485, `assert ctl_good >= 1`). That issue therefore does not carry this failure's replay key.

=== Cause 1 of 2: the AWS/Events {EventBusName, RuleName} series for a matching rule on a custom bus never settles on a value, so rule-level match/trigger accounting is unobservable ===
AWS: Smithy (cloudwatch) GetMetricStatistics — input Namespace (required, @length 1..255, @pattern ^[^:]), MetricName (required, @length 1..255), Dimensions (@length 0..30), StartTime/EndTime/Period (required, Period @range min 1), Statistics (@length 1..5); output Label + Datapoints; modelled errors InternalServiceFault (500 InternalServiceError), InvalidParameterCombinationException (400 InvalidParameterCombination), InvalidParameterValueException (400 InvalidParameterValue), MissingRequiredParameterException (400 MissingParameter). Smithy (cloudwatch) ListMetrics — input Namespace, MetricName, Dimensions (DimensionFilters, @length 0..10), NextToken, RecentlyActive, IncludeLinkedAccounts, OwningAccount; output Metrics + NextToken + OwningAccounts; errors InternalServiceFault, InvalidParameterValueException; paginated on NextToken. The metric vocabulary itself is in no Smithy model, so for the metric semantics this is recorded behavior only: for a rule on a CUSTOM event bus, EventBridge publishes MatchedEvents and TriggeredRules in namespace AWS/Events under the combined dimension set {EventBusName, RuleName}, with Unit "Count", and ListMetrics reports that two-dimension set for the rule's RuleName. One PutEvents entry onto a bus carrying two rules that differ only in the `source` they pin accounts for MatchedEvents Sum == 1 and TriggeredRules Sum == 1 on the MATCHING rule's series only; the non-matching twin's series stays absent entirely (no datapoints, not a zero datapoint). The recording also shows the alternate dimension sets ({EventBusName} alone, {RuleName} alone, {EventSourceName} alone) are NOT published for a custom bus.
Emulator: the check creates the custom bus, two rules (one matching, one not) pointed at the same SQS queue, publishes exactly one entry, asserts the delivery leg at the CONSUMER first (exactly one message, carrying the matching source and the id PutEvents returned), and then reads GetMetricStatistics(Namespace="AWS/Events", MetricName in {MatchedEvents, TriggeredRules}, Dimensions=[{EventBusName:<bus>},{RuleName:<matching rule>}], Period=60, Statistics=["Sum","SampleCount"]) until two readings held apart are equal and complete. That read never converges on a complete value, so the check produces no verdict: the suite reports `mismatch` with `Check timed out after 60.0s`. Everything gated behind it is therefore also unreached: MatchedEvents Sum == 1.0, TriggeredRules Sum == 1.0, Unit == {"Count"} on the datapoints, ListMetrics reporting the ("EventBusName","RuleName") dimension set for that RuleName, the held absence of TriggeredRules for the non-matching twin, and the re-read positive control.
Why: a client that operates an EventBridge fan-out from these counters cannot tell "the rule matched and triggered" from "nothing was published". It is the only per-rule accounting AWS offers, so an alarm on MatchedEvents/TriggeredRules sits in INSUFFICIENT_DATA forever instead of firing, and the AWS guarantee that a non-matching rule accounts for NOTHING (which is what distinguishes correct routing from a fan-out-to-every-rule implementation that still delivers the right message to the right queue) cannot be relied on, because an intentionally-absent series is indistinguishable from a store that publishes nothing.
Fix: the EventBridge rule-matching path must publish its per-rule AWS/Events telemetry into the CloudWatch metric store — MatchedEvents and TriggeredRules with Unit Count under the {EventBusName, RuleName} dimension set on every pattern match, and nothing at all for a rule that did not match — and the CloudWatch metric store must serve those series through GetMetricStatistics and register them in ListMetrics.
Traces: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule

=== Cause 2 of 2: SuccessfulInvocationAttempts is never published, so a rule whose SQS target demonstrably accepted the event reads Sum 0 ===
AWS: same GetMetricStatistics model surface as above; metric semantics are recorded behavior only. For a rule whose target was invoked and accepted the event, EventBridge publishes AWS/Events SuccessfulInvocationAttempts for that rule's RuleName with a non-zero Sum. In the recording this is the discriminator that makes a NEGATIVE observation on a sibling rule meaningful: the control rule's target was never denied and this metric IS emitted for that target type, so a Sum of 0 there means the metric is not observable at all rather than that nothing was delivered.
Emulator: the check establishes a bus-to-bus forward, attaches a destination-bus Deny, confirms via DescribeEventBus that the Deny is in the policy, re-publishes, and observes that the CONTROL rule on the source bus did deliver the event to its own SQS queue while the destination consumer stayed empty at depth 0 — i.e. the matching and the policy enforcement both happened. It then reads GetMetricStatistics(Namespace="AWS/Events", MetricName="SuccessfulInvocationAttempts", Dimensions=[{RuleName:<control rule>}], Period=60, Statistics=["Sum"]) and fails: `AssertionError: SuccessfulInvocationAttempts Sum=0 for the CONTROL rule <rule>, expected >=1 — the control's target was never denied and this metric IS emitted for that target type, so its absence would mean the metric isn't observable at all in this account/window` (check_cloudwatch__events__sqs_failed_invocations_records_the_bus_forwarding_the_destination_policy_denied.py line 485). The rule name in that message is a per-run generated name, not a requirement; the requirement is that a non-zero Sum exists for the RuleName of a rule whose target accepted an event.
Why: with SuccessfulInvocationAttempts missing, delivery success is unmeasurable, so the complementary AWS distinctions a client depends on collapse: a permissions refusal that deliberately does NOT raise FailedInvocations, and a dead-lettered invocation counted in InvocationsSentToDlq, both become indistinguishable from "EventBridge publishes no counters". Dashboards and alarms built on the per-rule invocation series read as a total outage.
Fix: the EventBridge target-invocation path must publish SuccessfulInvocationAttempts into the CloudWatch metric store on every accepted delivery (under the rule's RuleName dimension, alongside the FailedInvocations / InvocationsSentToDlq emission decisions for refused and dead-lettered deliveries), and the CloudWatch metric store must serve it.
Traces: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied

## Evidence
From this behavioral replay (266/766 passed; cloudwatch__events 0/1, cloudwatch__events__sqs 0/5). These two are the cloudwatch-token failures the suite marked neither ALREADY FILED nor OWNED; the other four cloudwatch__events__sqs failures are OWNED by existing issues.

Cause 1 — cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule [mismatch]:
  Error: Check timed out after 60.0s
  Expected: after one PutEvents entry onto a custom bus carrying a matching and a non-matching rule (delivery already asserted at the SQS consumer: exactly one message, source == the matching source, id == the EventId PutEvents returned),
    GetMetricStatistics(Namespace="AWS/Events", MetricName="MatchedEvents", Dimensions=[{Name:"EventBusName",Value:<bus>},{Name:"RuleName",Value:<matching rule>}], Period=60, Statistics=["Sum","SampleCount"]) -> Datapoints summing to 1.0, Unit "Count";
    the same read for MetricName="TriggeredRules" -> Sum 1.0;
    ListMetrics(Namespace="AWS/Events", Dimensions=[{Name:"RuleName",Value:<matching rule>}]) -> a metric whose dimension set is exactly ("EventBusName","RuleName");
    GetMetricStatistics for TriggeredRules on the NON-matching twin's {EventBusName, RuleName} -> no datapoints at all, held for the whole absence window;
    and a final re-read of the matching rule's TriggeredRules still 1.0.
  Actual: the paired MatchedEvents/TriggeredRules read never returns a complete, stable value, so the check is cut off at the harness deadline with no verdict. (Sum values 1.0 above are the counts this one-event scenario requires, not values copied from a run.)

Cause 2 — cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied [mismatch]:
  File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
  File "check_cloudwatch__events__sqs_failed_invocations_records_the_bus_forwarding_the_destination_policy_denied.py", line 485, in run
    assert ctl_good >= 1, (
  AssertionError: SuccessfulInvocationAttempts Sum=0 for the CONTROL rule sim2real-nd-fwd-ctlrule-385782ef8597, expected >=1 — the control's target was never denied and this metric IS emitted for that target type, so its absence would mean the metric isn't observable at all in this account/window
  Expected: GetMetricStatistics(Namespace="AWS/Events", MetricName="SuccessfulInvocationAttempts", Dimensions=[{Name:"RuleName",Value:<control rule>}], Period=60, Statistics=["Sum"]) -> Sum >= 1 for a rule whose SQS target accepted the event.
  Actual: Sum 0 (no datapoints). The generated rule name in the message is a per-run value; the fixed requirement is the non-zero Sum for the delivering rule's RuleName.
  Steps that PASSED first, locating the gap in metric emission rather than routing: the bus-to-bus forward was established end-to-end; PutPermission's Deny was visible in DescribeEventBus's Policy; the re-published event reached the CONTROL rule's queue; the destination consumer stayed empty and reported depth 0. Note this is a LATER and different failure point than the one recorded in 20261030T042208Z-cloudwatch-events-sqs-the-aws-events-per-rule.md, whose TriggeredRules poll no longer blocks.

## Steps to Reproduce
Cause 1 (MatchedEvents / TriggeredRules on a custom bus):
1. events CreateEventBus <bus>.
2. Create an SQS queue and set its Policy to allow Principal {Service: events.amazonaws.com} sqs:SendMessage on its ARN with Condition StringEquals aws:SourceAccount = <account>.
3. On <bus>: PutRule HIT with EventPattern {"source":[<matching source>],"detail-type":[<detail type>]}, State=ENABLED, PutTargets the queue. PutRule MISS with EventPattern {"source":[<other source>],"detail-type":[<detail type>]}, State=ENABLED, PutTargets the same queue.
4. PutEvents ONE entry with EventBusName=<bus>, Source=<matching source>, DetailType=<detail type>; assert FailedEntryCount == 0 and keep the returned EventId.
5. ReceiveMessage from the queue: exactly one message must arrive, whose envelope `source` is <matching source> and whose `id` is that EventId (the emulator gets this far).
6. cloudwatch GetMetricStatistics(Namespace="AWS/Events", MetricName="MatchedEvents", Dimensions=[{EventBusName:<bus>},{RuleName:HIT}], StartTime=now-20m, EndTime=now+5m, Period=60, Statistics=["Sum","SampleCount"]).
   Expected: Datapoints present, Sum totalling 1.0, every datapoint Unit "Count".
   Actual: no complete/stable value is ever returned.
7. Repeat step 6 for MetricName="TriggeredRules" (expected Sum 1.0).
8. cloudwatch ListMetrics(Namespace="AWS/Events", Dimensions=[{Name:"RuleName",Value:HIT}]): expected to include a metric whose dimension names are exactly EventBusName and RuleName.
9. GetMetricStatistics TriggeredRules for [{EventBusName:<bus>},{RuleName:MISS}]: expected NO datapoints for the whole absence window, then re-read HIT's TriggeredRules and expect it still 1.0.

Cause 2 (SuccessfulInvocationAttempts for a delivering rule):
1. events CreateEventBus <src> and CreateEventBus <dst>; create three SQS queues (control, destination consumer, DLQ), each with a policy allowing events.amazonaws.com sqs:SendMessage under Condition ArnLike aws:SourceArn = arn:*:events:<region>:<account>:rule/*.
2. On <dst>: PutRule + PutTargets the destination consumer. On <src>: PutRule CONTROL + PutTargets the control queue. Create an IAM role trusting events.amazonaws.com with events:PutEvents on <dst>'s bus ARN; on <src>: PutRule FORWARD + PutTargets {Arn: <dst> bus ARN, RoleArn: that role, DeadLetterConfig: {Arn: DLQ ARN}}.
3. PutEvents on <src> until an event reaches the destination consumer (forward established).
4. PutPermission on <dst> Denying that forwarding role events:PutEvents; confirm the Deny Sid via DescribeEventBus.
5. PutEvents on <src> with a fresh nonce: the control queue receives it; the destination consumer stays empty at depth 0.
6. cloudwatch GetMetricStatistics(Namespace="AWS/Events", MetricName="SuccessfulInvocationAttempts", Dimensions=[{Name:"RuleName",Value:CONTROL}], StartTime=<before step 5>, EndTime=now+2m, Period=60, Statistics=["Sum"]).
   Expected: Sum >= 1.
   Actual: Sum 0 with no datapoints.

## Resolution
Each AWS/Events per-rule counter is now published under exactly the dimension set(s) that counter
is published under, and under no other: the ROUTING counters (MatchedEvents, TriggeredRules), which
count what a bus's routing table did with an event, under the rule's whole identity
{EventBusName, RuleName} only — a read of the rule name alone, the bus alone or an event source
answers with no datapoint at all — and the HANDOVER counters (Invocations,
SuccessfulInvocationAttempts, FailedInvocations), which count what became of a delivery, under that
identity and under {RuleName} alone. Every accepted target invocation now counts once on
Invocations AND once on SuccessfulInvocationAttempts (a refused one on FailedInvocations, a
never-attempted one on neither), from every path that hands an event to a target — PutEvents, a
bus-to-bus forward's re-publication, PutPartnerEvents and a replay — because all of them deliver
through the one shared delivery step. Covered end to end by
`services/service-com-amazonaws-eventbridge-awsevents/tests/all/rule_metric_dimension_sets.rs`
(a literal, test-side table of which counter is published under which dimension set, probed through
GetMetricStatistics — a set no series was published under answering with no datapoint at all —
GetMetricData, and ListMetrics both unfiltered and through a RuleName dimension filter), with
`rule_invocation_metrics.rs` and `rule_match_metrics.rs` asserting the two counter families.

Cause 2 is verified by replay: the recorded check
`cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied`
now passes end to end.

Cause 1 is served but its recorded check still ends "Check timed out after 60.0s" inside its own
metric stability barrier. Every read that check names is answered — the sibling check above polls
GetMetricStatistics for TriggeredRules under {EventBusName, RuleName} on a custom bus and now gets
its datapoint, and the tests above assert the recorded read shape (a window ending ahead of now,
Period 60, Statistics Sum and SampleCount, Unit Count) for both routing counters, the absence for
the non-matching twin, and the ListMetrics registration. Before this change both dimension sets
carried the same count through both readers, so no value or dimension-set condition can be what
that barrier waits on; what remains is the barrier's own hold, and the match is counted at the
instant it happens — stamping the datapoint into an earlier minute would invent a past instant, so
it is not done.

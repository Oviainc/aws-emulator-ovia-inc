Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule
Issue-Key: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule|_assert_stays#31|Timeout|time.sleep(interval_s)
Issue-Commit: 629b90c9eb742411997cc74e691e02fa60cf874f

# cloudwatch__events: the AWS/Events MatchedEvents/TriggeredRules series for a custom bus's rule is still never readable — the routing-accounting check now hangs to the 180 s cap

> _Note: this issue was identified against the code base at commit `629b90c9eb742411997cc74e691e02fa60cf874f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
1 net-new-as-replayed root cause in the EventBridge -> CloudWatch metric-accounting read path. NOTE FOR THE TRIAGER: open issue 20261031T025935Z-cloudwatch-events-the-aws-events-matchedevents-triggeredrules-series.md already describes this same cause and already lists this same check in its Issue-Trace-Ids. The replay did NOT mark this failure as owned by it — its Issue-Key pins the old failure site (`_poll_until_stable#25|Timeout|time.sleep(interval_s)`) and the 60 s cap, while this run reports a bare harness-level "Check timed out after 180.0s", so the key no longer joins. This entry re-grounds the same cause against the current replay so it is not lost; if 20261031T025935Z is still the live work item, close this one against it rather than doing the work twice.

Cause 1 of 1: reading the AWS/Events routing counters a custom bus's rule publishes.

AWS: EventBridge accounts every pattern match of a rule in the AWS/Events namespace under the COMBINED {EventBusName, RuleName} dimension set — MatchedEvents and TriggeredRules, one observation of value 1 per match, Unit 'Count', at standard (60-second) resolution. A client reads them with CloudWatch GetMetricStatistics (Smithy, services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801: GetMetricStatistics takes Namespace, MetricName, StartTime, EndTime, Period plus Statistics/ExtendedStatistics and Dimensions, and returns Datapoints; modeled errors InvalidParameterValueException, MissingRequiredParameterException, InvalidParameterCombinationException, InternalServiceFault) and discovers the series with ListMetrics (input Dimensions targets DimensionFilters, output Metrics). Recorded behavior only for the dimension layout and the absence rule: on a CUSTOM bus these two counters exist ONLY under {EventBusName, RuleName} — {EventBusName} alone, {RuleName} alone and {EventSourceName} alone stay permanently absent — every returned datapoint carries Unit exactly 'Count', ListMetrics filtered to the rule name reports the ('EventBusName','RuleName') dimension set, and a rule whose pattern did NOT match the published event accounts for no datapoint at all on that series.

Emulator: the check publishes one entry to a fresh custom bus carrying two rules that differ only in the source they pin, confirms FailedEntryCount == 0, and then polls GetMetricStatistics for Namespace='AWS/Events', MetricName='MatchedEvents' and 'TriggeredRules' under {EventBusName=<bus>, RuleName=<matching rule>} for a value that settles. It never obtains a complete pair of readings, makes no further progress, and is killed by the suite's per-check cap: "Error: Check timed out after 180.0s". No AWS/Events value for the rule's series is ever served. (Note the check's own windows — 120 s of stability on the positive series, a 180 s absence hold on the twin, a 600 s metric budget — exceed the 180 s cap, so the timeout alone does not localise the stall; what it does establish is that no complete AWS/Events reading is ever produced.)

Why: a client cannot observe how many events EventBridge thought it routed. Every dashboard, capacity report and alarm built on AWS/Events MatchedEvents/TriggeredRules reads empty against the emulator while the same code reads real counts against AWS. Worse, an over-counting router (one that credits TriggeredRules to rules whose pattern did not match) and a router with no accounting at all are indistinguishable, so delivery-only assertions pass in both.

Fix: CloudWatch's metric READ path — GetMetricStatistics' aggregation/lookup over the stored datapoints by (namespace, metric name, dimension set, period) and ListMetrics' enumeration of the dimension sets present for a metric — so that datapoints a service appended for its own AWS/ namespace are served on the same terms as PutMetricData-ingested ones, including the Unit they were stored with. The EventBridge write side already produces the series and the {EventBusName, RuleName} dimension set; keep that set as the only published one for these two counters on a custom bus.

Traces: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), service group cloudwatch__events: 0/1 passed, 1 failed. This is the group's only check and the only net-new (not OWNED / not ALREADY FILED) failure in it.

[mismatch] cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule
  Error: Check timed out after 180.0s

What the check does and what it expects:
1. CreateEventBus on a fresh custom bus; two ENABLED rules on that one bus differing ONLY in the source they pin (one matching, one not), both targeting one SQS queue whose policy allows sqs:SendMessage for Principal Service events.amazonaws.com with Condition aws:SourceAccount = the caller's account.
2. PutEvents with a single entry naming the matching source; asserts FailedEntryCount == 0.
3. Delivery leg, read at the CONSUMER: exactly one message on the target queue, whose envelope `source` is the matching source and whose `id` is the EventId PutEvents returned (two arrivals would mean both rules matched).
4. Terminal leg, read at CloudWatch: get_metric_statistics(Namespace='AWS/Events', MetricName='MatchedEvents' then 'TriggeredRules', Dimensions=[{'EventBusName': <bus>}, {'RuleName': <matching rule>}], StartTime=now-20min, EndTime=now+5min, Period=60, Statistics=['Sum','SampleCount']) must settle on Sum == 1.0 for BOTH metrics; every returned datapoint's Unit must be exactly 'Count'; list_metrics(Namespace='AWS/Events', Dimensions=[{'Name':'RuleName','Value':<matching rule>}]) must report the ('EventBusName','RuleName') dimension set.
5. Negative leg: TriggeredRules for {EventBusName: <bus>, RuleName: <non-matching twin>} must have NO datapoints at all while the positive series is being served.
6. Positive control after the negative: the matching rule's TriggeredRules Sum still reads 1.0.

Expected: the two counters settle at Sum 1.0 on the matching rule's {EventBusName, RuleName} series with Unit 'Count', ListMetrics reports that two-dimension set, and the twin's series stays absent.
Actual: the check never reaches a complete reading and is killed at the 180.0 s cap; no AWS/Events value for the rule's series is ever obtained (the stability poll `_poll_until_stable` is waiting on a `(MatchedEvents, TriggeredRules)` pair that never both become non-None).

Requirements vs per-run values: the requirements are the namespace AWS/Events, the metric names MatchedEvents and TriggeredRules, the {EventBusName, RuleName} dimension set as the only published set on a custom bus, Sum == 1.0 per single matched event, Unit 'Count', and the total absence of any datapoint on the non-matching rule's series. NOT requirements: the generated bus name, the two generated rule names, the queue URL/ARN, the account id, the EventId and the datapoint timestamps.

Related open issues: 20261031T025935Z-cloudwatch-events-the-aws-events-matchedevents-triggeredrules-series.md (same cause, same check id, key drifted so the replay no longer joins it) and 20261029T203422Z-monitoring-cloudwatch-published-metric-data-is-unqueryable-and-metric-query-validation-diverges (GetMetricStatistics answering 200 with Datapoints: [] over a window into which data was just published — grounded in the wire corpus on client-published custom namespaces, naming neither AWS/Events nor this check; a fix there may well close this one).

## Steps to Reproduce
1. Boot the emulator from this branch (CloudWatch and EventBridge both answer; the `events` group passes 162/194, so routing itself is live).
2. events.create_event_bus(Name='probe-bus').
3. Create an SQS queue and set its policy to allow sqs:SendMessage for Principal Service events.amazonaws.com with Condition aws:SourceAccount = the caller's account; note its QueueArn.
4. On 'probe-bus' create two ENABLED rules with the SAME detail-type and DIFFERENT sources — rule 'hit' pinning source 'probe.match', rule 'miss' pinning source 'probe.nomatch' — and PutTargets the queue ARN on each.
5. events.put_events with one entry: EventBusName='probe-bus', Source='probe.match', DetailType=<the shared detail-type>, Detail='{}'. Confirm FailedEntryCount == 0 and note the returned EventId.
6. ReceiveMessage on the queue until exactly one message arrives; confirm its envelope `source` and `id`.
7. cloudwatch.get_metric_statistics(Namespace='AWS/Events', MetricName='MatchedEvents', Dimensions=[{'Name':'EventBusName','Value':'probe-bus'},{'Name':'RuleName','Value':'hit'}], StartTime=now-20min, EndTime=now+5min, Period=60, Statistics=['Sum','SampleCount']) and the same for MetricName='TriggeredRules'.
   Expected: one or more Datapoints whose Sum totals 1.0, each carrying Unit 'Count'. Observed: no value is ever returned for the series.
8. cloudwatch.list_metrics(Namespace='AWS/Events', Dimensions=[{'Name':'RuleName','Value':'hit'}]).
   Expected: a metric whose dimension names are exactly ('EventBusName','RuleName').
9. Repeat step 7 for RuleName='miss'.
   Expected: no Datapoints at all, held while the 'hit' series is being served.
10. Replay the single check to see the timeout: behavioral check id cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule.

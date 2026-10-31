Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest|run#23|AssertionError|raise AssertionError(
Issue-Key: logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: 92bcdac01f88c8b4612f5537a2ed608090c91758

# logs: subscription-filter delivery now reaches the destination but ignores the log group's transformer (applyOnTransformedLogs delivers the RAW line), and neither AWS/Logs ForwardedLogEvents nor AWS/Logs DeliveryErrors is ever published (3 causes)

> _Note: this issue was identified against the code base at commit `92bcdac01f88c8b4612f5537a2ed608090c91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Three distinct CloudWatch Logs obligations around subscription-filter delivery diverge. These are NET-NEW failure modes, not the ones the adjacent open issue carries: 20261026T072730Z-logs-cloudwatch-logs-subscription-filter-delivery-produces-nothing.md describes a state in which NOTHING at all arrived at the destination (all four of its checks reported only `Check timed out after 60.0s` while waiting on the first delivery poll). Delivery to the Kinesis destination now demonstrably works — cause 1 below shows a record actually arriving and being read back by the consumer — so the failures have moved past that point and the replay keys that issue carries no longer match. The remaining open issues 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md and 20261024T150703Z-cloudwatch-logs-puttransformer-accepts-a-transformer-with-no.md are also adjacent (over-permissive PutDeliverySource/PutSubscriptionFilter validation, and PutTransformer validation respectively) but neither covers the transform-on-delivery or metric-emission behavior below.

Cause 1 of 3: a subscription filter with applyOnTransformedLogs=true delivers the UNTRANSFORMED raw log line, so the log group's transformer is not applied on the delivery path.
AWS: Smithy models PutTransformer{logGroupIdentifier @length(1..2048), transformerConfig: Processors @length(1..20)} and the Processor union member `parseVPC` -> ParseVPC ("parse Amazon VPC vended logs, extract fields, and convert them into a JSON format. This processor always processes the entire log event message"), and PutSubscriptionFilter.applyOnTransformedLogs (ApplyOnTransformedLogs, boolean, @default false). Recorded behavior for the delivery contract: with transformerConfig [{"parseVPC":{}}] on a log group and a subscription filter carrying applyOnTransformedLogs=true, every delivered record's `message` is the TRANSFORMED event — a camelCase JSON object in which the numeric flow-log fields (version, srcPort, dstPort, protocol, packets, bytes, start, end) are JSON numbers and the address/identifier fields (accountId, interfaceId, srcAddr, dstAddr) plus action/logStatus are JSON strings — and on a NODATA line the fields whose raw value is the '-' placeholder are DROPPED entirely rather than emitted as "-" or null. The stored event stays byte-identical on the GetLogEvents read path.
Emulator: the delivered record's `message` is the raw, unparsed flow-log text line. The check fails with `AssertionError: a subscription with applyOnTransformedLogs=true must deliver the transformed JSON event, got non-JSON message '2 123456789012 eni-0a1b2c3d4e5f60718 10.0.1.5 10.0.2.9 443 49152 6 10 840 1700000000 1700000060 ACCEPT OK' (Extra data: line 1 column 3 (char 2))`. In that message the fixed part of the assertion is the sentence "a subscription with applyOnTransformedLogs=true must deliver the transformed JSON event, got non-JSON message"; the quoted line is the request's own payload (the raw message the check wrote with PutLogEvents), and the parenthesised text is the JSON decoder's position report — both are per-run values, not part of the requirement. The requirement is only that the delivered message parses as the transformed JSON object described above.
Why: the transform happens inside the service between the write and the asynchronous delivery, so the applyOnTransformedLogs delivery is the ONLY path on which a client can observe what its processor chain produced. A consumer that configured parseVPC and applyOnTransformedLogs=true writes a JSON parser against the delivered message; against the emulator that parser throws on every record, and the whole vended-processor contract (camelCase naming, numeric typing of ports/bytes/packets, dropping of '-' placeholders) ships ungraded.
Fix: the applyOnTransformedLogs branch of the subscription-filter delivery path must run the log group's stored transformer chain over the event before the envelope is serialized, and the vended parseVPC processor must be implemented with the field names and JSON types above. This belongs in the PutSubscriptionFilter delivery component and the PutTransformer processor chain of the CloudWatch Logs crate; see Traces for the files.
Traces: logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest (assertion at check line 103). Components: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs and services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_transformer.rs.

Cause 2 of 3: AWS/Logs ForwardedLogEvents is never published for a log-group-level subscription filter.
AWS: recorded behavior only — the metric is not part of the Smithy model (PutSubscriptionFilter returns Unit and models no telemetry). CloudWatch Logs publishes the AWS/Logs metric `ForwardedLogEvents` for a log-group-level subscription filter under exactly the dimension names {LogGroupName, DestinationType, FilterName} (PolicyLevel is a dimension of ACCOUNT-level subscription filters only), discoverable through cloudwatch:ListMetrics, with the FilterName dimension value equal to the filterName of the request. Its Sum over the window equals the number of events the filterPattern MATCHED, not the number written, and each datapoint carries Unit "None" per the AWS/Logs metric table.
Emulator: the metric never becomes discoverable through ListMetrics and no datapoint is readable, so the check never advances past the discovery poll and reports `Check timed out after 60.0s` (the harness cap; the check's own poll budget is 600s for discovery and 420s for the datapoint). The delivery leg before it succeeds — the consumer did receive exactly the matching records — so the absent telemetry, not an absent delivery, is what is unobservable here. Hypothesis: the subscription-delivery path has no CloudWatch metric sink at all, which is also why cause 3 produces nothing.
Why: ForwardedLogEvents is the only way a client can confirm its filter pattern is selecting the events it intended without reading the destination, and it is what distinguishes "the pattern matched 3 of 5" from "the pattern was ignored and all 5 were forwarded". With no metric, every dashboard and alarm built on AWS/Logs ForwardedLogEvents sits permanently in INSUFFICIENT_DATA against the emulator.
Fix: the metric-emission path of the subscription-filter delivery component — it must publish AWS/Logs ForwardedLogEvents, Unit None, incremented by the count of events that MATCHED the filter pattern, under the {LogGroupName, DestinationType, FilterName} dimension set, into the same store cloudwatch:ListMetrics and cloudwatch:GetMetricStatistics read.
Traces: logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events. Components: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs plus the CloudWatch metric sink the logs crate writes to.

Cause 3 of 3: AWS/Logs DeliveryErrors is never published when the destination cannot accept the delivery.
AWS: recorded behavior only (no modeled telemetry on PutSubscriptionFilter). With the subscription destination deleted, a matching log event makes CloudWatch Logs publish AWS/Logs `DeliveryErrors` with Sum >= 1 under the SAME dimension values as ForwardedLogEvents for that filter, Unit "None" (a ResourceNotFoundException from the destination is a non-retryable delivery error). The log event is NOT lost — it stays readable in the log group — and the event written while the destination was gone is never re-delivered once the destination returns: the filter's destination is marked unhealthy and only re-validated on the next PutSubscriptionFilter call. A destination recreated under the same name presents the identical ARN, so reissuing the identical PutSubscriptionFilter restores delivery.
Emulator: no DeliveryErrors datapoint is ever published; the check never advances past that poll and reports `Check timed out after 60.0s` (harness cap; its own DeliveryErrors poll budget is 600s).
Why: DeliveryErrors is the sole signal separating "the target refused the delivery" from "CloudWatch Logs never attempted it". Without it a client cannot alarm on broken subscription plumbing at all — a silently dropping destination and a correctly working one look identical.
Fix: the delivery-failure path of the subscription-filter delivery component must classify a non-retryable destination failure, mark the filter's destination unhealthy until the next PutSubscriptionFilter, keep the event stored, and publish AWS/Logs DeliveryErrors (Unit None) under the filter's dimension values.
Traces: logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint. Components: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs plus the CloudWatch metric sink.

## Evidence
Cause 1 — logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest, verdict `mismatch`:
  Error: AssertionError: a subscription with applyOnTransformedLogs=true must deliver the transformed JSON event, got non-JSON message '2 123456789012 eni-0a1b2c3d4e5f60718 10.0.1.5 10.0.2.9 443 49152 6 10 840 1700000000 1700000060 ACCEPT OK' (Extra data: line 1 column 3 (char 2))
  Expected delivered message for the full line: {"version": 2, "accountId": "123456789012", "interfaceId": "eni-0a1b2c3d4e5f60718", "srcAddr": "10.0.1.5", "dstAddr": "10.0.2.9", "srcPort": 443, "dstPort": 49152, "protocol": 6, "packets": 10, "bytes": 840, "start": 1700000000, "end": 1700000060, "action": "ACCEPT", "logStatus": "OK"} — version/srcPort/dstPort/protocol/packets/bytes/start/end as JSON NUMBERS.
  Expected delivered message for the NODATA line '2 123456789012 eni-… - - - - - - - 1700000000 1700000060 - NODATA': {"version": 2, "accountId": "123456789012", "interfaceId": "eni-0a1b2c3d4e5f60718", "start": 1700000000, "end": 1700000060, "logStatus": "NODATA"} — every '-' field ABSENT, not "-" and not null.
  Actual: the raw text line is delivered verbatim; json.loads fails on it. (The specific eni id, addresses and epoch seconds above are this recording's payload values, not part of the requirement; the requirement is the field set, the camelCase names and the JSON types.)

Cause 2 — logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events, verdict `mismatch`:
  Error: Check timed out after 60.0s
  Setup: one log group + one stream, a Kinesis destination stream, a CWL delivery role, PutSubscriptionFilter(filterPattern='SIM2REALMATCH'); PutLogEvents of 3 matching and 2 non-matching in-window events, rejectedLogEventsInfo absent.
  Expected: the 3 matching events arrive at the consumer (this leg passes); then cloudwatch:ListMetrics discovers AWS/Logs ForwardedLogEvents for the group with dimension names exactly {LogGroupName, DestinationType, FilterName} and FilterName == the created filterName; GetMetricStatistics Sum == 3 (NOT 5 — the pattern is applied before the count) and every datapoint Unit == 'None'.
  Actual: the metric is never discoverable through ListMetrics, so no dimension set and no datapoint exists and the check is still polling when the 60 s harness cap fires.

Cause 3 — logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint, verdict `mismatch`:
  Error: Check timed out after 60.0s
  Setup: positive-control delivery through the filter first succeeds and the ForwardedLogEvents dimension triple is read off it; the Kinesis destination is then deleted (confirmed by DescribeStream not found) and one more matching event is written.
  Expected: AWS/Logs DeliveryErrors Sum >= 1 under the same dimension values as ForwardedLogEvents, every datapoint Unit == 'None'; the event written during the outage still readable from the log group; after recreating the stream (identical ARN) delivery resumes only once the identical PutSubscriptionFilter is reissued, and the outage event is never re-delivered.
  Actual: no DeliveryErrors datapoint is ever published; the check is still polling at the 60 s harness cap.

Not filed as part of this issue: the 60 s harness cap itself is the runner's bound, not an emulator response — it is quoted only because it is the verdict the suite reports when the awaited emulator effect never occurs.

## Steps to Reproduce
Cause 1 (transform on delivery):
1. logs.create_log_group / logs.create_log_stream on a fresh group.
2. logs.put_transformer(logGroupIdentifier=<group>, transformerConfig=[{"parseVPC": {}}]).
3. Create a Kinesis data stream and wait for ACTIVE; create an IAM role CloudWatch Logs may assume to write it.
4. logs.put_subscription_filter(logGroupName=<group>, filterName=<f>, filterPattern='', destinationArn=<stream arn>, roleArn=<role arn>, applyOnTransformedLogs=True).
5. logs.put_log_events with a v2 VPC flow-log line and a NODATA line whose unavailable fields are '-'.
6. Read the destination shard and gunzip the envelope: expect each record's `message` to parse as the camelCase JSON object (numeric fields as JSON numbers, '-' fields dropped). Observe the raw text line delivered instead, so json.loads raises.
7. Confirm GetLogEvents still returns the raw line byte-identical.

Cause 2 (ForwardedLogEvents):
1. Fresh log group + stream, Kinesis destination stream (ACTIVE), CWL delivery role.
2. logs.put_subscription_filter(..., filterPattern='SIM2REALMATCH').
3. logs.put_log_events with 3 messages containing SIM2REALMATCH and 2 that do not, all in-window.
4. Drain the destination shard: the 3 matching events arrive.
5. cloudwatch.list_metrics(Namespace='AWS/Logs', MetricName='ForwardedLogEvents') filtered to the group: expect one metric with dimension names exactly {LogGroupName, DestinationType, FilterName} and FilterName == <f>. Observe nothing is ever returned.
6. cloudwatch.get_metric_statistics on that dimension set: expect Sum == 3 and Unit 'None'.

Cause 3 (DeliveryErrors):
1. Repeat steps 1-2 of cause 2 and write one matching event; confirm it is delivered (positive control) and read the ForwardedLogEvents dimension values.
2. kinesis.delete_stream(StreamName=<dest>, EnforceConsumerDeletion=True) and poll DescribeStream until not found.
3. logs.put_log_events with one more matching event.
4. cloudwatch.get_metric_statistics for AWS/Logs DeliveryErrors on the same dimension values: expect Sum >= 1, Unit 'None'. Observe no datapoint ever appears.
5. Confirm the outage event is still readable in the log group.
6. Recreate the stream under the same name (identical ARN), confirm delivery does NOT resume on its own, reissue the identical PutSubscriptionFilter, write a new matching event and confirm it is delivered while the outage event is not re-delivered.

Or replay the three check ids in trace_ids.

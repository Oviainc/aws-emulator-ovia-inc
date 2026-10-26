Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: d7ab0aa2df0341b9bd19f61962ae23a79b6d6bd1

# logs: CloudWatch Logs subscription-filter delivery produces nothing observable — no record envelope at the destination, no AWS/Logs ForwardedLogEvents, no AWS/Logs DeliveryErrors, and applyOnTransformedLogs delivers no transformed parseVPC event (4 causes)

> _Note: this issue was identified against the code base at commit `d7ab0aa2df0341b9bd19f61962ae23a79b6d6bd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Four distinct CloudWatch Logs claims about subscription-filter delivery and its telemetry are all ungraded because the emulator never produces the awaited effect; every one of these checks reports exactly `Check timed out after 60.0s`. These are net-new causes: the open issues 20261025T100129Z-logs-cloudwatch-logs-subscription-destination-plumbing-a-logical-destination.md, 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md and 20261024T150703Z-logs-cloudwatch-logs-metric-filters-emit-no-cloudwatch-metric.md are adjacent but carry none of the four replay keys below.

Shared context — Hypothesis: all four checks use a Kinesis data stream as the subscription destination, and `kinesis` is reported as NOT bootstrapped by get_bootstrap_status (pending: service-com-amazonaws-kinesis-kinesis-20131202). The destination side may therefore be the proximate blocker for the consumer-side assertions; the metric-side causes (2 and 3) are nevertheless CloudWatch Logs' own obligations and are not satisfied by a working destination alone. A fixer must stand the destination up and then re-replay the four ids.

Cause 1 of 4: the delivered record envelope.
AWS: Smithy models PutSubscriptionFilter{logGroupName, filterName, filterPattern, destinationArn, roleArn, distribution, applyOnTransformedLogs} returning Unit, and the destination contract is recorded behavior only: CloudWatch Logs delivers a GZIP-compressed (magic 0x1f 0x8b) JSON envelope carrying `owner` (the 12-digit account id owning the log group), `logGroup`, `logStream` (the single stream the batch was written to), `messageType`, `subscriptionFilters` (exactly the one filterName for a group with one filter) and `logEvents`, where each element has a distinct non-empty `id`, the exact millisecond `timestamp` passed to PutLogEvents and a byte-identical `message`. Only events the filterPattern matched appear; a non-matching event in the same batch never does.
Emulator: `Check timed out after 60.0s` — no record ever arrives at the destination, so neither the envelope's shape nor the pattern filtering is observable.
Why: a client consuming log events through a subscription filter receives nothing and cannot distinguish "filter not matching" from "delivery not implemented"; the envelope fields (owner/logGroup/logStream/subscriptionFilters) are what downstream consumers key on.
Fix: the PutSubscriptionFilter handler's delivery path and the envelope serializer for subscription records (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs and the log-event delivery component in that crate's state).
Traces: logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events.

Cause 2 of 4: AWS/Logs ForwardedLogEvents is never published.
AWS: recorded behavior only (the metric is not in the Smithy model): CloudWatch Logs publishes the AWS/Logs metric `ForwardedLogEvents` for a log-group-level subscription filter under exactly the dimension names {LogGroupName, DestinationType, FilterName}, discoverable through cloudwatch:ListMetrics, and its Sum over the window equals the number of events the filterPattern MATCHED — not the number written. PolicyLevel is a dimension of account-level subscription filters only.
Emulator: `Check timed out after 60.0s` — the metric never becomes discoverable through ListMetrics and no datapoint is readable, so the count can never be compared.
Why: ForwardedLogEvents is the only way a client can confirm that its filter pattern is selecting the events it intended without reading the destination; with no metric, dashboards and alarms built on it are permanently in INSUFFICIENT_DATA.
Fix: the metric-emission path of the subscription-filter delivery component — it must publish AWS/Logs ForwardedLogEvents with the {LogGroupName, DestinationType, FilterName} dimension set on each delivery (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs plus the CloudWatch metric sink).
Traces: logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events.

Cause 3 of 4: AWS/Logs DeliveryErrors is never published when a destination cannot accept the delivery.
AWS: recorded behavior only (blessed against real AWS 2026-09-17, us-west-2): with the destination deleted, a matching log event causes CloudWatch Logs to publish AWS/Logs `DeliveryErrors` with Sum >= 1 under the SAME dimension values as ForwardedLogEvents for that filter, with Unit "None" per the AWS/Logs metric table (ResourceNotFoundException is documented as a non-retryable delivery error). The log event itself is never lost — it stays readable in the log group — and the event written while the destination was gone is not re-delivered after the destination returns; the filter's destination is only re-validated on the next PutSubscriptionFilter call.
Emulator: `Check timed out after 60.0s` — no DeliveryErrors datapoint is ever published, so a destination that refuses a delivery is indistinguishable from a destination that was never attempted.
Why: DeliveryErrors is the sole signal that separates "the target refused the delivery" from "CloudWatch Logs never tried". Without it a client cannot alarm on broken subscription plumbing at all.
Fix: the delivery-failure path of the subscription-filter delivery component — classify a non-retryable destination failure, mark the destination unhealthy until the next PutSubscriptionFilter, and publish AWS/Logs DeliveryErrors (Unit None) under the filter's dimension values (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs plus the CloudWatch metric sink).
Traces: logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint.

Cause 4 of 4: a subscription filter with applyOnTransformedLogs=true delivers no transformed event, so the vended parseVPC processor's output is unobservable.
AWS: Smithy models PutTransformer{logGroupIdentifier, transformerConfig: Processors @length(1..20)} and PutSubscriptionFilter.applyOnTransformedLogs (default false). Recorded behavior: with transformerConfig [{"parseVPC":{}}] on a group and a subscription filter with applyOnTransformedLogs=true, the delivered envelope's `message` is the TRANSFORMED event — camelCase JSON in which numeric flow-log fields (version, srcPort, dstPort, protocol, packets, bytes, start, end) are JSON NUMBERS and address/id fields (accountId, interfaceId, srcAddr, dstAddr) are strings — and a NODATA line's '-' placeholders are DROPPED rather than emitted as "-" or null. The stored raw event remains byte-identical on the GetLogEvents/FilterLogEvents path.
Emulator: `Check timed out after 60.0s` — no transformed event is ever delivered, so neither the camelCase naming, nor the JSON typing, nor the dropping of '-' placeholders is graded.
Why: the transform happens inside the service between a write and an asynchronous delivery, so applyOnTransformedLogs delivery is the only path on which a client can see what its processor chain actually produced; an emulator that silently delivers nothing lets a wrong processor contract (snake_case names, string-typed ports, retained '-') ship undetected.
Fix: the PutTransformer processor chain (the vended parseVPC processor) and the applyOnTransformedLogs branch of the subscription-filter delivery path (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_transformer.rs and .../put_subscription_filter.rs).
Traces: logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest.

## Evidence
All four checks fail with the identical headline and no error code or status to quote, because the awaited effect never occurs:

- logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events — Error: Check timed out after 60.0s. Expected: a GZIP envelope at the destination with owner=<caller account>, logGroup=<group>, logStream=<the stream written>, subscriptionFilters=[<filterName>], and only the SIM2REALMATCH events, each with a distinct id and its exact timestamp/message. Actual: nothing delivered.
- logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events — Error: Check timed out after 60.0s. Expected: AWS/Logs ForwardedLogEvents discoverable via ListMetrics with dimension names exactly {LogGroupName, DestinationType, FilterName} and Sum == 3 for a batch of 3 matching + 2 non-matching events. Actual: metric never discoverable, no datapoint.
- logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint — Error: Check timed out after 60.0s. Expected: after deleting the destination, AWS/Logs DeliveryErrors Sum >= 1 with Unit "None" under the same dimension values as ForwardedLogEvents, the event still readable in the group, and delivery resuming only after the identical PutSubscriptionFilter is reissued. Actual: no datapoint ever published.
- logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest — Error: Check timed out after 60.0s. Expected (for raw '2 123456789012 eni-… 10.0.1.5 10.0.2.9 443 49152 6 10 840 … ACCEPT OK' and a NODATA line): delivered messages {version:2(number), accountId:"…"(string), interfaceId:"…", srcAddr/dstAddr strings, srcPort/dstPort/protocol/packets/bytes/start/end numbers, action/logStatus strings} and, for NODATA, only {version, accountId, interfaceId, start, end, logStatus} with the '-' fields absent. Actual: no transformed event delivered.

Hypothesis: the destination exercised by all four is a Kinesis data stream, and kinesis is listed as pending bootstrap, so the consumer-side legs may be blocked there; causes 2 and 3 are CloudWatch Logs' own metric obligations regardless.

## Steps to Reproduce
1. logs.create_log_group + create_log_stream; create a Kinesis destination stream and an IAM role CloudWatch Logs may assume to write it.
2. logs.put_subscription_filter(logGroupName=<group>, filterName=<f>, filterPattern='SIM2REALMATCH', destinationArn=<stream arn>, roleArn=<role arn>).
3. put_log_events with 3 messages containing SIM2REALMATCH and 2 that do not.
4. Read the destination shard: expect exactly the 3 matching events inside one GZIP JSON envelope carrying owner/logGroup/logStream/subscriptionFilters/logEvents. Observe nothing arrives.
5. cloudwatch.list_metrics / get_metric_statistics for AWS/Logs ForwardedLogEvents with dimensions {LogGroupName, DestinationType, FilterName}: expect Sum == 3. Observe the metric is never discoverable.
6. Delete the destination stream, write one more matching event, and poll AWS/Logs DeliveryErrors for Sum >= 1 with Unit 'None'. Observe no datapoint; also confirm the event is still readable in the log group.
7. On a fresh group, logs.put_transformer(logGroupIdentifier=<group>, transformerConfig=[{'parseVPC': {}}]), then put_subscription_filter(..., applyOnTransformedLogs=True) and write a v2 flow-log line plus a NODATA line. Expect the transformed camelCase JSON (numeric fields as JSON numbers, '-' fields dropped) as the delivered message and the raw line unchanged on GetLogEvents. Observe nothing delivered.
8. Or replay the four check ids in trace_ids.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-cleared-metrics-config-freezes-event-counts
Issue-Key: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-cleared-metrics-config-freezes-event-counts|_poll#13|Timeout|time.sleep(5)
Issue-Commit: b5c349f78c27b93d7bc476c26ede97815cd374b3

# cloudwatch__lambda__sqs: an SQS event-source mapping with MetricsConfig EventCount publishes no per-mapping AWS/Lambda event-count series, so clearing MetricsConfig cannot be shown to freeze it (1 cause)

> _Note: this issue was identified against the code base at commit `b5c349f78c27b93d7bc476c26ede97815cd374b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventSourceMapping

## Description
Cause 1 of 1: a Lambda event-source mapping created with MetricsConfig={"Metrics":["EventCount"]} produces no AWS/Lambda per-mapping event-count datapoints at CloudWatch, so neither the counted phase nor the frozen phase that an emptied MetricsConfig is supposed to produce can be observed; the check hangs to the harness cap with no leg completed.

AWS: Smithy: EventSourceMappingMetricsConfig.Metrics is a list of EventSourceMappingMetric (@length 0..3) whose values are EventCount, ErrorCount and KafkaMetrics; the member documentation says "EventCount to receive metrics related to the number of events processed by your event source mapping" and points at the Event source mapping metrics reference. Accepted on CreateEventSourceMapping and on UpdateEventSourceMapping, and echoed back by GetEventSourceMapping. AWS behaviour, as this check records it: while the mapping is Enabled with EventCount selected, it publishes PolledEventCount, InvokedEventCount, FilteredOutEventCount and DeletedEventCount into the AWS/Lambda namespace dimensioned by EventSourceMappingUUID = the mapping's UUID, including an explicit 0-valued PolledEventCount datapoint for an idle minute (which is what makes a later empty window mean suppression rather than lag). UpdateEventSourceMapping with MetricsConfig={"Metrics": []} is accepted, read back empty, leaves the mapping Enabled and still draining its queue, and the per-mapping series falls silent from the second minute boundary after the update (recorded behaviour, measured twice: exactly one trailing zero in the next minute, then nothing), with the accumulated totals unchanged.

Emulator: the check hangs and the replay verdict is the bare "Check timed out after 60.0s" - no leg completed and no assertion reached. Its blocking barriers, in order, are: the queue draining to depth (0,0) after the ESM is Enabled (180 s budget), then Polled/Invoked/DeletedEventCount for the mapping's UUID each reaching the number of messages sent (480 s budget), then an idle-minute 0-valued PolledEventCount (240 s budget). None of them clears within the cap, so the emulator's SQS event-source mapping does not surface the per-mapping EventCount family at CloudWatch under the EventSourceMappingUUID dimension (and may not be polling the queue at all).

Why: per-mapping event counts are the only way a client can see what a poller did with its messages - how many were polled, filtered out by FilterCriteria, handed to the function and deleted - and MetricsConfig is the documented switch that turns them on and off. Against this emulator a dashboard or alarm built on PolledEventCount/InvokedEventCount/FilteredOutEventCount/DeletedEventCount under EventSourceMappingUUID reads empty forever, which is indistinguishable from a mapping that is doing no work, and the MetricsConfig switch has no observable effect in either direction.

Fix: the Lambda event-source-mapping poller/runtime in the lambda service must emit the EventCount family (PolledEventCount, InvokedEventCount, FilteredOutEventCount, DeletedEventCount) into the AWS/Lambda namespace dimensioned by EventSourceMappingUUID while the mapping's MetricsConfig selects EventCount, publish the idle-minute zeros, and stop publishing once MetricsConfig is updated to an empty Metrics list; the sink is the cloudwatch service's metric store, read back through GetMetricStatistics.

Related already-open issues, named so this is not closed as a duplicate of either: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (function invocation) and 20261025T020208Z-cloudwatch-dynamodb-lambda-lambda-listeventsourcemappings-answers-resourcenotfoundexception-function-not-found-for.md (ESM control-plane reads). Neither carries this check's replay key and neither covers the per-mapping metric emission or the MetricsConfig gate, so a fix confined to either leaves this check failing.

Traces: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-cleared-metrics-config-freezes-event-counts (mismatch, "Check timed out after 60.0s").

## Evidence
Expected (AWS): after create_event_source_mapping(FunctionName=<fn>, EventSourceArn=<queue arn>, BatchSize=10, MaximumBatchingWindowInSeconds=1, FilterCriteria=..., MetricsConfig={"Metrics":["EventCount"]}) reaches State="Enabled" and N messages are sent, cloudwatch.get_metric_statistics(Namespace="AWS/Lambda", MetricName="PolledEventCount"|"InvokedEventCount"|"DeletedEventCount", Dimensions=[{Name:"EventSourceMappingUUID",Value:<uuid>}], Period=60, Statistics=["Sum"]) sums to N for each, and PolledEventCount additionally shows a 0-valued datapoint for an idle minute. After update_event_source_mapping(UUID=<uuid>, MetricsConfig={"Metrics": []}) (accepted, read back empty, State stays "Enabled"), further messages are still drained but add no datapoint at or after the second minute boundary following the update, and the totals stay at N.

Actual (emulator): no AWS/Lambda datapoint is ever returned for any of PolledEventCount / InvokedEventCount / FilteredOutEventCount / DeletedEventCount under Dimensions=[{EventSourceMappingUUID:<uuid>}]; the counted-phase barrier never clears and the harness kills the check with "Check timed out after 60.0s" before any assertion in the file is evaluated.

## Steps to Reproduce
1. iam.create_role for lambda.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes; sqs.create_queue(Attributes={VisibilityTimeout:"60"}); lambda.create_function(Runtime="python3.12") returning len(event["Records"]).
2. lambda.create_event_source_mapping(FunctionName=<fn>, EventSourceArn=<queue arn>, BatchSize=10, MaximumBatchingWindowInSeconds=1, FilterCriteria={"Filters":[{"Pattern": "{\"body\":{\"kind\":[\"keep\"]}}"}]}, MetricsConfig={"Metrics":["EventCount"]}); wait for get_event_source_mapping State=="Enabled".
3. sqs.send_message_batch with 3 matching messages; wait for ApproximateNumberOfMessages and ApproximateNumberOfMessagesNotVisible to both read 0.
4. cloudwatch.get_metric_statistics(Namespace="AWS/Lambda", MetricName="PolledEventCount", Dimensions=[{Name:"EventSourceMappingUUID",Value:<uuid>}], Period=60, Statistics=["Sum"]). Expected: Sum 3 (likewise InvokedEventCount and DeletedEventCount). Observed: no datapoints.
5. lambda.update_event_source_mapping(UUID=<uuid>, MetricsConfig={"Metrics": []}); send 4 more matching messages; confirm they drain. Expected: no new datapoint at or after the second minute boundary after the update, totals still 3. Observed: unreachable - the series never existed.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-filter-event-counts-and-opt-out-twin
Issue-Key: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected|run#8|AssertionError|assert refusals == expected, (
Issue-Key: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-filter-event-counts-and-opt-out-twin|_poll#17|Timeout|time.sleep(5)
Issue-Commit: 551d2b0384fb46e58b4eda1c68177a3367972419

# cloudwatch__lambda__sqs: a duplicated MetricsConfig metric is refused with the wrong message, and SQS ESM filtered event counts still never materialise (2 causes)

> _Note: this issue was identified against the code base at commit `551d2b0384fb46e58b4eda1c68177a3367972419`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
REOPENS 20261029T182452Z-cloudwatch-lambda-sqs-sqs-event-source-mappings-filtered-event-counts-never.md: its fix left the call refused. That issue is marked fixed and claimed both of these checks; both still fail, and the MetricsConfig check now fails on the refusal the fix introduced rather than on an acceptance. This issue bundles 2 root causes. Both must be closed before submitting.

Cause 1 of 2: CreateEventSourceMapping's MetricsConfig validation reports a DUPLICATED metric with the invalid-value message instead of AWS's duplicate-metrics message.
AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice) models CreateEventSourceMapping.MetricsConfig as EventSourceMappingMetricsConfig with member Metrics -> EventSourceMappingMetricList, @length min 0 max 3, member target the EventSourceMappingMetric enum whose values are exactly [EventCount, ErrorCount, KafkaMetrics]. The list is NOT @uniqueItems, and the model documents ErrorCount and KafkaMetrics as Amazon MSK / self-managed Apache Kafka only — so both the source-type restriction and the duplicate check live in the service, not the model: recorded behavior only. The recording shows two distinct refusal classes, both HTTP 400 InvalidParameterValueException: (a) a metric that is not valid for the mapping's resolved event source is refused with the fixed text "Invalid EventSourceMapping metrics. Expected values: [" + the comma-separated list of metrics permitted for THAT event-source type + "]" — for an SQS source that list is EventCount, so the message is "Invalid EventSourceMapping metrics. Expected values: [EventCount]"; (b) a list that repeats a metric is refused with the fixed text "MetricsConfig contains duplicate metrics.", which has no variable part. The two classes are checked independently: a list that repeats an otherwise-permitted value is a duplicate error, not an invalid-value error.
Emulator: for MetricsConfig={"Metrics": ["EventCount", "EventCount"]} on an SQS-sourced mapping it answers InvalidParameterValueException with the message "Invalid EventSourceMapping metrics. Expected values: [EventCount]" — class (a)'s message for a list whose only member IS the permitted value. The two single-value negatives (["KafkaMetrics"], ["ErrorCount"]) now match AWS, so only the duplicate branch diverges. No mapping is created in either case, which is correct.
Why: a client that keys on the refusal text (or surfaces it to an operator) is told the metric name is unsupported for the source when the metric is in fact supported and the real fault is that it was listed twice, so the caller 'fixes' the wrong field and the call keeps failing. Error-text assertions written against AWS fail against the emulator.
Fix: CreateEventSourceMapping's / UpdateEventSourceMapping's MetricsConfig input validation in the lambda service — add the duplicate-detection branch ahead of (or beside) the event-source-type vocabulary check and give it its own message.
Traces: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected

Cause 2 of 2: an SQS event source mapping with FilterCriteria and MetricsConfig still publishes no ESM event counts, and the check cannot complete.
AWS: recorded behavior only (the lambda model describes FilterCriteria and MetricsConfig but not the emitted CloudWatch series). An SQS mapping that opts into ESM metrics publishes PolledEventCount, InvokedEventCount, FilteredOutEventCount and DeletedEventCount dimensioned on EventSourceMappingUUID alone (never FunctionName): FilteredOutEventCount equals the number of non-matching messages, DeletedEventCount equals ALL messages (on SQS a filtered message is deleted too), InvokedEventCount covers at least the matching ones, and PolledEventCount is published in unit Count. Every message, kept or filtered, leaves the queue. A twin mapping over the same message mix WITHOUT MetricsConfig publishes none of those metrics while its own function still runs.
Emulator: the check exhausts its budget — "Error: Check timed out after 60.0s" — so neither the opted-in mapping's counts nor the twin's emptiness is ever observable; the SQS ESM poller / ESM metric publisher produces nothing.
Why: a customer tuning an ESM filter cannot see how many events were filtered out versus delivered, and the MetricsConfig opt-in is indistinguishable from the opt-out; nothing can alarm on an SQS mapping.
Fix: the Lambda SQS event-source-mapping poller and its ESM metric publisher (honouring the MetricsConfig opt-in) plus the cloudwatch state those datapoints land in.
Traces: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-filter-event-counts-and-opt-out-twin

## Evidence
Cause 1 — cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected:
AssertionError: MetricsConfig refusals on an SQS mapping were [(['KafkaMetrics'], ('InvalidParameterValueException', 'Invalid EventSourceMapping metrics. Expected values: [EventCount]')), (['ErrorCount'], ('InvalidParameterValueException', 'Invalid EventSourceMapping metrics. Expected values: [EventCount]')), (['EventCount', 'EventCount'], ('InvalidParameterValueException', 'Invalid EventSourceMapping metrics. Expected values: [EventCount]'))], expected [(['KafkaMetrics'], ('InvalidParameterValueException', 'Invalid EventSourceMapping metrics. Expected values: [EventCount]')), (['ErrorCount'], ('InvalidParameterValueException', 'Invalid EventSourceMapping metrics. Expected values: [EventCount]')), (['EventCount', 'EventCount'], ('InvalidParameterValueException', 'MetricsConfig contains duplicate metrics.'))]
Expected for the duplicate input: HTTP 400, code InvalidParameterValueException, message 'MetricsConfig contains duplicate metrics.' (entirely fixed text, no request- or state-derived parts).
Actual: HTTP 400, code InvalidParameterValueException, message 'Invalid EventSourceMapping metrics. Expected values: [EventCount]' — whose fixed text is 'Invalid EventSourceMapping metrics. Expected values: [' ... ']' and whose variable part is the permitted-metric list for the mapping's event-source type.

Cause 2 — cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-filter-event-counts-and-opt-out-twin:
Error: Check timed out after 60.0s (the check polls CloudWatch for the mapping's PolledEventCount / InvokedEventCount / FilteredOutEventCount / DeletedEventCount datapoints under the EventSourceMappingUUID dimension; none ever arrive).

## Steps to Reproduce
Boot the emulator and replay the two check ids in trace_ids.

Cause 1: create an IAM role trusted by lambda.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes, an SQS queue and a python function. Call CreateEventSourceMapping(FunctionName=fn, EventSourceArn=<queue arn>, MetricsConfig={'Metrics': ['KafkaMetrics']}), then ['ErrorCount'], then ['EventCount','EventCount']. The first two must be refused 400 InvalidParameterValueException 'Invalid EventSourceMapping metrics. Expected values: [EventCount]'; the third must be refused 400 InvalidParameterValueException 'MetricsConfig contains duplicate metrics.'. After each refusal ListEventSourceMappings(FunctionName=fn) must be empty. Then CreateEventSourceMapping with MetricsConfig={'Metrics': ['EventCount']} must be accepted and echo it back.

Cause 2: on the same queue/function, create two SQS mappings with FilterCriteria={'Filters':[{'Pattern': '{"body": {"kind": ["keep"]}}'}]}, one with MetricsConfig={'Metrics':['EventCount']} and one without. Send a mix of matching and non-matching messages, then poll GetMetricStatistics in AWS/Lambda for PolledEventCount, InvokedEventCount, FilteredOutEventCount and DeletedEventCount with Dimensions=[{'Name':'EventSourceMappingUUID','Value':<uuid>}]. The opted-in mapping must produce the counts (FilteredOutEventCount = non-matching, DeletedEventCount = all) and the twin none.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-filter-event-counts-and-opt-out-twin cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected
Issue-Key: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-filter-event-counts-and-opt-out-twin|_poll#17|Timeout|time.sleep(5)
Issue-Key: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected|_create_error#7|AssertionError|raise AssertionError(
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# SQS event source mappings: filtered event counts never materialise and CreateEventSourceMapping accepts Kafka-only MetricsConfig on an SQS source

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
This issue bundles 2 net-new root causes. Both must be closed before submitting.

Cause 1 of 2: an SQS event source mapping with FilterCriteria never produces the filtered/unfiltered event counts.
AWS: recorded behavior only (the lambda model describes FilterCriteria and MetricsConfig, not the emitted series). An SQS ESM that opts into ESM metrics publishes the filtered event counts for the mapping, and the opt-out twin mapping publishes none — the contrast between the two is the assertion.
Emulator: the check exhausts its 60s budget; neither the opted-in mapping's counts nor the opt-out twin's emptiness can be observed, i.e. the ESM poller/metric path produces nothing.
Why: a customer tuning an ESM filter has no way to see how many events were filtered out versus delivered, and the MetricsConfig opt-in is indistinguishable from the opt-out.
Fix: the Lambda SQS event-source-mapping poller and its ESM metric publisher (honouring MetricsConfig opt-in).
Traces: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-filter-event-counts-and-opt-out-twin.

Cause 2 of 2: CreateEventSourceMapping accepts a Kafka-only MetricsConfig metric on an SQS mapping.
AWS: `MetricsConfig.Metrics` is validated against the mapping's event-source type; the Kafka-only vocabulary (`KafkaMetrics`) is refused on an SQS-sourced mapping with a client error, and no mapping is created.
Emulator: the call is ACCEPTED with HTTP 202 and the mapping is returned with `MetricsConfig: {'Metrics': ['KafkaMetrics']}`, `State: 'Creating'` and an EventSourceMappingArn — so an invalid mapping is persisted.
Why: the misconfiguration deploys cleanly against the emulator and is refused by the real account, and the mapping that exists in the emulator advertises a metric vocabulary that cannot apply to its source.
Fix: CreateEventSourceMapping's / UpdateEventSourceMapping's MetricsConfig validation against the resolved event-source type.
Traces: cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected.

## Evidence
Cause 1 — cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-filter-event-counts-and-opt-out-twin: `Error: Check timed out after 60.0s`.

Cause 2 — cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected:
  AssertionError: MetricsConfig ['KafkaMetrics'] was ACCEPTED on an SQS mapping, real AWS rejects it: {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'UUID': ..., 'EventSourceArn': 'arn:aws:sqs:us-west-2:000000000000:<queue>', 'MetricsConfig': {'Metrics': ['KafkaMetrics']}, 'State': 'Creating', 'EventSourceMappingArn': ...}

## Steps to Reproduce
Replay the two check ids. The first creates a queue, a function, and two SQS event source mappings with FilterCriteria (one opting into ESM metrics, one not), sends matching and non-matching messages, and polls CloudWatch for the filtered event counts. The second calls CreateEventSourceMapping on an SQS queue with MetricsConfig={'Metrics': ['KafkaMetrics']}.

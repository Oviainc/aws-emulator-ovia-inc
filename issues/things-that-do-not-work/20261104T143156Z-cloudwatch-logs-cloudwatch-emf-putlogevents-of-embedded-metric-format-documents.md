Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.emf-log-to-metric-extraction-exactness
Issue-Key: cloudwatch__logs.emf-log-to-metric-extraction-exactness|_assert_stays#69|Timeout|time.sleep(interval_s)
Issue-Commit: 732d86be3641565fdf04b49c6a1d1231e9cf6723

# CloudWatch EMF: PutLogEvents of embedded-metric-format documents materialises no CloudWatch metric, so the extracted series never appear

> _Note: this issue was identified against the code base at commit `732d86be3641565fdf04b49c6a1d1231e9cf6723`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLogEvents

## Description
One net-new root cause, grounded in the single failing check listed under `### cloudwatch__logs` with no ALREADY FILED / OWNED marker. Cause count: 1 — it must be closed before this issue is submitted.

RELATED OPEN ISSUE (it does not carry this check id and is a different extraction path): 20261024T150703Z-logs-cloudwatch-logs-metric-filters-emit-no-cloudwatch-metric.md covers metric FILTERS (PutMetricFilter) emitting no metric. The cause below needs no metric filter at all: it is the service-side embedded-metric-format (EMF) extractor that reads the `_aws` envelope of an ingested log event.

=== Cause 1 of 1: the EMF extractor does not run on ingest ===
AWS: recorded behavior — a log event whose message is an EMF document ({"_aws":{"Timestamp":<ms>,"CloudWatchMetrics":[{"Namespace":N,"Dimensions":[[...],[...]],"Metrics":[{"Name":M,"Unit":U,"StorageResolution":R}]}]}, plus the sibling properties that supply each metric value and each declared dimension value) is accepted by PutLogEvents and, within seconds, materialises the declared metrics in CloudWatch under namespace N: one independent series per declared dimension SET, Sum/SampleCount/Minimum/Maximum aggregated over the documents in the period, the declared Unit carried through, and the series discoverable through ListMetrics. Undeclared high-cardinality properties stay in Logs and must NOT become dimensions; a declared metric with no sibling value, and a declared dimension that is not a property of the document, must yield NO metric. The model says nothing about EMF (no Smithy member expresses it), so the recording is the contract; PutLogEvents itself returns 200 with no rejectedLogEventsInfo in both AWS and the emulator.
Emulator: PutLogEvents accepts the EMF documents (no rejectedLogEventsInfo), but neither cloudwatch GetMetricStatistics(Namespace=N, MetricName='OrderCount'/'OrderLatency', Dimensions=[...]) nor cloudwatch ListMetrics(Namespace=N) ever returns anything: the probe stays empty for the whole budget, so the check cannot even begin its stability window and is killed at the harness's 60s cap. No metric is created at all — correct or otherwise.
Why: EMF is the standard way agentless workloads (Lambda powertools, the CloudWatch agent, ECS/EKS sidecars) publish metrics — they never call PutMetricData. Against the emulator every such metric silently does not exist: dashboards are empty, alarms built on those metrics stay in INSUFFICIENT_DATA forever, and the error is indistinguishable from 'no traffic yet' because the log events themselves were accepted.
Fix: an EMF extraction step on the PutLogEvents ingest path in the logs service crate (parse the `_aws` envelope, resolve each declared metric value and each declared dimension set from the document's sibling properties, drop a declaration whose value or dimension property is absent) that publishes the resulting datapoints into the cloudwatch service's metric store, carrying the declared Unit and StorageResolution, so GetMetricStatistics and ListMetrics see them.
Traces: cloudwatch__logs.emf-log-to-metric-extraction-exactness.

## Evidence
[mismatch] cloudwatch__logs.emf-log-to-metric-extraction-exactness
  Error: Check timed out after 60.0s

The check creates a log group + stream, PutLogEvents 12 EMF documents inside one whole minute (each declaring OrderLatency Milliseconds StorageResolution=1 and OrderCount Count over the dimension sets [["Service","Operation"],["Service"]]) plus two malformed-declaration arms, and asserts the write succeeded ('rejectedLogEventsInfo' absent) BEFORE looking at any metric — so every later absence is attributable to extraction, not to a dropped log event. It then polls cloudwatch GetMetricStatistics(Namespace=<group>, MetricName='OrderCount', Dimensions=[Service=checkout, Operation=PlaceOrder], Period=60, Statistics=[Sum,SampleCount,Minimum,Maximum]) together with ListMetrics(Namespace=<group>) and requires two EQUAL non-empty readings 45s apart. Its _snapshot() returns None while either reading is empty, which resets the anchor — so the poll can never 'stabilise on absence' and runs until the harness kills it. Expected on AWS: the first datapoint is readable ~10s after PutLogEvents returns, OrderCount under [Service, Operation] = exactly one Period=60 bucket with Sum == SampleCount == 12, Unit 'Count'; OrderLatency Sum 364.0, Minimum 11.0, Maximum 53.0; a second independent series under [Service] alone; requestId/customerId NOT present as dimensions; and no series for the declared-but-valueless metric or the declared-but-absent dimension. Observed: the namespace stays empty for the entire run.

## Steps to Reproduce
1. logs.create_log_group(logGroupName=G); logs.create_log_stream(logGroupName=G, logStreamName='emf').
2. logs.put_log_events(logGroupName=G, logStreamName='emf', logEvents=[{timestamp: t_i, message: json.dumps({"_aws":{"Timestamp":t_i,"CloudWatchMetrics":[{"Namespace":G,"Dimensions":[["Service","Operation"],["Service"]],"Metrics":[{"Name":"OrderLatency","Unit":"Milliseconds","StorageResolution":1},{"Name":"OrderCount","Unit":"Count"}]}]},"Service":"checkout","Operation":"PlaceOrder","OrderLatency":<latency_i>,"OrderCount":1,"requestId":<uuid>,"customerId":<uuid>})} for 12 distinct latencies at one-second offsets inside one minute]) -> 200, no rejectedLogEventsInfo.
3. cloudwatch.list_metrics(Namespace=G) -> OBSERVE Metrics == [] indefinitely; EXPECT OrderCount and OrderLatency, each under both {Service,Operation} and {Service}.
4. cloudwatch.get_metric_statistics(Namespace=G, MetricName='OrderCount', Dimensions=[{Service: checkout},{Operation: PlaceOrder}], Period=60, StartTime=<minute-60>, EndTime=now+5m, Statistics=['Sum','SampleCount','Minimum','Maximum']) -> OBSERVE Datapoints == []; EXPECT exactly one datapoint with Sum=12.0, SampleCount=12.0, Unit='Count' (and OrderLatency Sum=364.0, Minimum=11.0, Maximum=53.0).

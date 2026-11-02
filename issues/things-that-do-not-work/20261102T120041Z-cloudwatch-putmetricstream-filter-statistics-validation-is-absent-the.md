Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.mstrx-exclude-filter-of-metric-names-without-a-namespace-rejected cloudwatch.mstrx-statistics-configuration-with-no-additional-statistic-rejected cloudwatch.mstrx-out-of-range-and-misspelt-percentiles-rejected cloudwatch.rpcv2-cbor-describe-alarms-encodes-timestamps-as-epoch-tag cloudwatch.rpcv2-cbor-insight-rule-mute-rule-and-error-maps cloudwatch.alarm-on-a-series-never-published-stays-insufficient-data
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.mstrx-exclude-filter-of-metric-names-without-a-namespace-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudwatch.mstrx-statistics-configuration-with-no-additional-statistic-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudwatch.mstrx-out-of-range-and-misspelt-percentiles-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudwatch.rpcv2-cbor-describe-alarms-encodes-timestamps-as-epoch-tag|run#9|AssertionError|assert (status, headers.get("smithy-protocol")) == (200, "rpc-v2-cbor"), (
Issue-Key: cloudwatch.rpcv2-cbor-insight-rule-mute-rule-and-error-maps|cbor_call#9|AssertionError|assert headers.get("smithy-protocol") == "rpc-v2-cbor", (
Issue-Key: cloudwatch.alarm-on-a-series-never-published-stays-insufficient-data|_poll#12|Timeout|time.sleep(interval_s)
Issue-Commit: b8358f56f4f44e240ca755ea7919bfa62f43cb3d

# cloudwatch: PutMetricStream filter/statistics validation is absent, the rpc-v2-cbor protocol is not served, and metric alarms are never evaluated from published datapoints (5 causes)

> _Note: this issue was identified against the code base at commit `b8358f56f4f44e240ca755ea7919bfa62f43cb3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
Five independent root causes in the CloudWatch metric-stream write path, the CBOR wire protocol and the alarm evaluator. Each must be closed separately. Related open issues that do NOT carry these replay keys (so they do not cover these causes), to be merged with where they overlap: 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md (already reports that rpc-v2-cbor is not served, for different checks), 20261029T055514Z-cloudwatch-a-slidingwindow-default-metric-alarm-never-transitions.md and 20261028T224444Z-cloudwatch-7-net-new-root-causes-high-resolution.md (alarm evaluation), 20261102T112036Z-monitoring-cloudwatch-metric-stream-operations-refuse-awsquery-and-emit.md.

Cause 1 of 5: PutMetricStream accepts an ExcludeFilters/IncludeFilters entry with MetricNames but no Namespace.
AWS: Smithy: PutMetricStream.ExcludeFilters targets MetricStreamFilters; MetricStreamFilter.Namespace is NOT marked required in the model, so this is a service rule the model cannot express. Recorded behavior: a filter entry naming MetricNames with no Namespace is refused HTTP 400 `InvalidParameterValue` with `Value null at 'ExcludeFilters.<i>.member.Namespace' failed to satisfy constraint: Member must not be null`; the fixed text is the frame, and `<i>` is the zero-based index of the offending filter in the request list. No stream is created (GetMetricStream then answers `ResourceNotFoundException` with an empty message).
Emulator: the call succeeds, so no error code exists and the stream is created.
Why: a client that omits the namespace gets a stream whose filter CloudWatch would never accept, and the configuration it reads back is not one real AWS can produce.
Fix: PutMetricStream's validation (filter-list checks before the write).
Traces: cloudwatch.mstrx-exclude-filter-of-metric-names-without-a-namespace-rejected.

Cause 2 of 5: PutMetricStream accepts an empty AdditionalStatistics list.
AWS: Smithy: PutMetricStream.StatisticsConfigurations targets MetricStreamStatisticsConfigurations; the non-empty requirement on AdditionalStatistics is a service rule. Recorded behavior: a statistics configuration whose AdditionalStatistics list is empty is refused HTTP 400 `InvalidParameterValue` with the message `AdditionalStatistics less than 1` (wholly fixed text) and nothing is created — unlike an empty MetricNames list on a filter, which AWS accepts and normalises away.
Emulator: the call succeeds (no error code) and the stream is created.
Why: the asymmetry between the two empty lists is part of the contract; the emulator erases it, so a caller cannot distinguish a configuration AWS stores from one it refuses.
Fix: PutMetricStream's validation.
Traces: cloudwatch.mstrx-statistics-configuration-with-no-additional-statistic-rejected.

Cause 3 of 5: PutMetricStream accepts unsupported AdditionalStatistics values.
AWS: recorded behavior only (the model types AdditionalStatistics as a plain string list): AWS accepts open-interval percentiles (`p95`, `p99`, `p99.9`) and refuses `p0`, `p100`, an upper-case `P99` and a standard statistic name such as `Average`, each with HTTP 400 `InvalidParameterValue` and the message `Unsupported statistic <value> for selected OutputFormat` — fixed text apart from `<value>`, the rejected statistic from the request. A refused put leaves the previously stored AdditionalStatistics untouched.
Emulator: every one of those values is accepted (no error code) and overwrites the stored configuration.
Why: the percentile grammar and the open interval (0, 100) are what a caller relies on to catch a bad statistic before the stream starts emitting; here any string is stored and the stream's configuration diverges from anything AWS would run.
Fix: PutMetricStream's validation (statistic grammar parse, per OutputFormat).
Traces: cloudwatch.mstrx-out-of-range-and-misspelt-percentiles-rejected.

Cause 4 of 5: the rpc-v2-cbor protocol is not served.
AWS: Smithy: the service GraniteServiceVersion20100801 declares protocols awsJson1_0, awsQuery AND rpcv2Cbor. A Smithy RPC v2 CBOR request is POSTed to `/service/GraniteServiceVersion20100801/operation/<OperationName>` with `smithy-protocol: rpc-v2-cbor`, `content-type: application/cbor` and NO `x-amz-target` header; the response is HTTP 200 carrying `smithy-protocol: rpc-v2-cbor` and a CBOR body (timestamps encoded as epoch-tagged items), and modeled errors come back in the CBOR error envelope.
Emulator: the request is answered HTTP 400 with `content-type: application/x-amz-json-1.0` and the awsJson error body `{"__type":"SerializationException","message":"missing required header `x-amz-target`"}` — the CBOR route is not recognised at all, so the request is parsed as awsJson1_0.
Why: an SDK configured for rpc-v2-cbor (the default for several newer SDK releases) cannot call CloudWatch at all, and no CBOR-encoded response — including the epoch-tag timestamp encoding and the CBOR error maps — is observable.
Fix: the service's protocol routing/serialization layer (the rpc-v2-cbor request router and serializer), not any single operation handler.
Traces: cloudwatch.rpcv2-cbor-describe-alarms-encodes-timestamps-as-epoch-tag, cloudwatch.rpcv2-cbor-insight-rule-mute-rule-and-error-maps.

Cause 5 of 5: a metric alarm is never evaluated server-side from published datapoints.
AWS: recorded behavior only (alarm evaluation is not expressible in the model): an alarm created by PutMetricAlarm with Period=60, EvaluationPeriods=1, DatapointsToAlarm=1 is evaluated by the service from the datapoints PutMetricData stored — within about 1-3 minutes it moves INSUFFICIENT_DATA -> ALARM/OK as the series crosses the threshold, each transition is recorded as a StateUpdate alarm-history item, and an alarm whose own dimensioned series never receives a datapoint stays INSUFFICIENT_DATA with no history at all.
Emulator: the control alarm fed matching datapoints never leaves its initial state, so the check never observes the first ALARM transition and is cut off at the harness ceiling (60 s) while still polling; no state transition and no StateUpdate history item ever appears.
Why: every scenario that reads an alarm's state after publishing data — dashboards, alarm actions, deployment gates — observes a state that never changes, so the absence of a transition can never be distinguished from a correctly-quiet alarm.
Fix: the alarm evaluator in the service's state (crate::state::alarm_evaluation, driven from the PutMetricData store) — it must evaluate single-metric alarms and record the transitions in alarm history.
Traces: cloudwatch.alarm-on-a-series-never-published-stays-insufficient-data.

## Evidence
Replayed against the emulator (suite service heading `### cloudwatch`):

1. mstrx-exclude-filter-of-metric-names-without-a-namespace-rejected — put_metric_stream(ExcludeFilters=[{'MetricNames': ['CPUUtilization','NetworkIn']}]) expected ('InvalidParameterValue', "Value null at 'ExcludeFilters.0.member.Namespace' failed to satisfy constraint: Member must not be null"); actual: 'the call succeeded; real AWS rejects it, so there is no error code'.
2. mstrx-statistics-configuration-with-no-additional-statistic-rejected — StatisticsConfigurations=[{IncludeMetrics:[...], AdditionalStatistics: []}] expected ('InvalidParameterValue', 'AdditionalStatistics less than 1'); actual: the call succeeded.
3. mstrx-out-of-range-and-misspelt-percentiles-rejected — AdditionalStatistics=['p0'] (and 'p100', 'P99', 'Average') expected ('InvalidParameterValue', 'Unsupported statistic p0 for selected OutputFormat'); actual: the call succeeded.
4. rpcv2-cbor-describe-alarms-encodes-timestamps-as-epoch-tag — POST {endpoint}/service/GraniteServiceVersion20100801/operation/PutMetricAlarm with smithy-protocol: rpc-v2-cbor expected (200, 'rpc-v2-cbor'); actual 400 with headers {'content-type': 'application/x-amz-json-1.0', 'content-length': '86'} and body ...'"SerializationException","message":"missing required header `x-amz-target`"}'. rpcv2-cbor-insight-rule-mute-rule-and-error-maps fails identically on the same route.
5. alarm-on-a-series-never-published-stays-insufficient-data — after put_metric_alarm + repeated put_metric_data on the alarm's exact dimensions, the check polls describe_alarms for the control alarm to reach ALARM (its own deadline 300 s, poll interval 15 s) and never sees it; the check was cut at the harness ceiling: 'Check timed out after 60.0s'. No StateUpdate item is produced for either alarm.

## Steps to Reproduce
1. Cause 1: cw.put_metric_stream(Name=N, FirehoseArn=F, RoleArn=R, OutputFormat='json', ExcludeFilters=[{'MetricNames': ['CPUUtilization','NetworkIn']}]) — expect 400 InvalidParameterValue, observe success; then cw.get_metric_stream(Name=N) and observe the stream exists.
2. Cause 2: same put with StatisticsConfigurations=[{'IncludeMetrics': [{'Namespace':'AWS/ECS','MetricName':'CPUUtilization'}], 'AdditionalStatistics': []}] — expect 400 InvalidParameterValue 'AdditionalStatistics less than 1'.
3. Cause 3: same put with AdditionalStatistics=['p0'] then ['p100'], ['P99'], ['Average'] — expect 400 InvalidParameterValue each time; also confirm a previously stored ['p99'] is left untouched.
4. Cause 4: POST {endpoint}/service/GraniteServiceVersion20100801/operation/PutMetricAlarm with headers smithy-protocol: rpc-v2-cbor, content-type: application/cbor, accept: application/cbor, a SigV4 signature over signing name 'monitoring', a definite-length CBOR body and NO x-amz-target — expect 200 with smithy-protocol: rpc-v2-cbor and a CBOR body, observe 400 SerializationException.
5. Cause 5: cw.put_metric_alarm(Namespace=NS, MetricName='HealthStatus', Dimensions=[{'Name':'ClusterIdentifier','Value':C}], Statistic='Average', Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=0.5, ComparisonOperator='GreaterThanThreshold'); then cw.put_metric_data on exactly those dimensions with Value=1.0 and poll cw.describe_alarms — the StateValue never leaves its initial value and describe_alarm_history(HistoryItemType='StateUpdate') stays empty.

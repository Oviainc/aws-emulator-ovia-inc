Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.cw-get-metric-data-math-expression-is-computed-server-side cloudwatch.metric-math-multi-metric-fill-rate-query cloudwatch.get-metric-data-dynamic-label-is-resolved-server-side cloudwatch.alarm-evaluation-engine-metric-math-alarm-evaluates-the-computed-series cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs cloudwatch.composite-alarm-server-side-evaluation cloudwatch.composite-alarm-and-rule-requires-all-children cloudwatch.composite-alarm-boolean-and-logic cloudwatch.composite-alarm-actions-suppressed-by-alarm cloudwatch.alarm-state-transitioned-vs-updated-timestamp cloudwatch.unit-scoped-metric-query-isolation cloudwatch.metric-data-high-resolution-period cloudwatch.percentile-availability-depends-on-the-publication-mode cloudwatch.values-counts-histogram-query-aggregation cloudwatch.metric-data-timestamp-ordering cloudwatch.metric-stream-start-stop-timing cloudwatch.mstrx-one-name-in-two-regions-is-two-independent-streams cloudwatch.get-metric-stream-invalid-name-pattern-rejected cloudwatch.get-dataset-nonexistent-not-found cloudwatch.get-dataset-not-found-for-unknown-dataset cloudwatch.disassociate-dataset-kms-key-nonexistent-dataset-not-found cloudwatch.put-metric-alarm-empty-vs-absent-dimensions-distinguished
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.cw-get-metric-data-math-expression-is-computed-server-side|run#19|AssertionError|assert by_id["e4"]["Values"] == [4.0], (
Issue-Key: cloudwatch.metric-math-multi-metric-fill-rate-query|run#19|AssertionError|assert e_sum["Values"] == [11.0, 20.0, 43.0], (
Issue-Key: cloudwatch.get-metric-data-dynamic-label-is-resolved-server-side|run#14|AssertionError|assert surviving == [], (
Issue-Key: cloudwatch.alarm-evaluation-engine-metric-math-alarm-evaluates-the-computed-series|run#13|AssertionError|assert set(dp) == {"timestamp", "value"}, f"expression evaluatedDatapoint fields {sorted(dp)!r}"
Issue-Key: cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs|_assert_evaluated_value#20|AssertionError|assert len(window) == 1, (
Issue-Key: cloudwatch.composite-alarm-server-side-evaluation|run#16|AssertionError|assert initial == "OK", (
Issue-Key: cloudwatch.composite-alarm-and-rule-requires-all-children|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.composite-alarm-boolean-and-logic|_poll_composite_state#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.composite-alarm-actions-suppressed-by-alarm|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.alarm-state-transitioned-vs-updated-timestamp|run#31|AssertionError|assert self_transitions == [], (
Issue-Key: cloudwatch.unit-scoped-metric-query-isolation|run#32|AssertionError|assert unscoped_gmd["Values"] == [7.0], (
Issue-Key: cloudwatch.metric-data-high-resolution-period|run#12|AssertionError|assert std_datapoints == [], (
Issue-Key: cloudwatch.percentile-availability-depends-on-the-publication-mode|run#27|AssertionError|assert spread_pctl == [], (
Issue-Key: cloudwatch.values-counts-histogram-query-aggregation|run#24|AssertionError|assert message == (
Issue-Key: cloudwatch.metric-data-timestamp-ordering|run#20|AssertionError|assert error == ("ValidationError", (
Issue-Key: cloudwatch.metric-stream-start-stop-timing|run#15|AssertionError|assert t_stopped > t_created, (
Issue-Key: cloudwatch.mstrx-one-name-in-two-regions-is-two-independent-streams|run#12|AssertionError|assert doc_home.get("ExcludeFilters") == [{"Namespace": "AWS/EC2", "MetricNames": ["NetworkIn"]}] \
Issue-Key: cloudwatch.get-metric-stream-invalid-name-pattern-rejected|run#8|AssertionError|assert invalid_code == "InvalidParameterValue", (
Issue-Key: cloudwatch.get-dataset-nonexistent-not-found|run#6|AssertionError|assert code == "AccessDenied", f"expected AccessDenied, got {code!r}"
Issue-Key: cloudwatch.get-dataset-not-found-for-unknown-dataset|run#9|AssertionError|assert code == "AccessDenied", f"expected AccessDenied, got {code!r}"
Issue-Key: cloudwatch.disassociate-dataset-kms-key-nonexistent-dataset-not-found|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudwatch.put-metric-alarm-empty-vs-absent-dimensions-distinguished|run#17|AssertionError|assert has_dims_empty == has_dims_absent, (has_dims_empty, has_dims_absent)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# CloudWatch metric/alarm data plane: 9 net-new root causes (metric-math fill, dynamic labels, alarm evaluation, composite evaluation, unit identity, metric-stream state, dataset errors)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
This issue bundles 9 net-new root causes in the CloudWatch metric read path, alarm evaluation engine and metric-stream/dataset state. Every cause is independent; all 9 must be closed before submitting.

Cause 1 of 9: metric math drops a bucket when one operand has no datapoint, instead of treating the absent operand as 0.
AWS: recorded behavior only (metric math fill semantics are not expressed in the Smithy model; MetricDataQuery.Expression is an opaque string). For an arithmetic expression over two metrics, CloudWatch returns a value in every bucket in which ANY operand has data, counting the operand with no datapoint as 0 — `m1 + m2` where m2 is absent at t1 yields m1's value at t1, and `m1 + mghost` over a never-written mghost yields m1's own series.
Emulator: the bucket is omitted entirely — `m1 + mghost` returned `[]` where `[4.0]` is required, and `m1+m2` returned `[11.0, 43.0]` where `[11.0, 20.0, 43.0]` is required.
Why: any dashboard or alarm built on a summed/rate expression silently loses the periods in which one series is idle, so a rate denominator disappears and the alarm never breaches.
Fix: the metric-math evaluator in the GetMetricData handler (operand alignment / fill of absent operands).
Traces: cloudwatch.cw-get-metric-data-math-expression-is-computed-server-side, cloudwatch.metric-math-multi-metric-fill-rate-query.

Cause 2 of 9: dynamic labels are echoed verbatim instead of being resolved server-side.
AWS: recorded behavior only (Label is modelled as a plain string). CloudWatch substitutes the dynamic-label placeholders from the data it is returning, so the response Label contains resolved numbers/names, never the `${...}` tokens.
Emulator: the returned Label is the request's string verbatim — `min ${MIN} max ${MAX} sum ${SUM} n ${DATAPOINT_COUNT} of ${LABEL}` — with every placeholder unresolved.
Why: dashboards and alerts that embed dynamic labels render literal `${MIN}` text to the user.
Fix: the GetMetricData serializer must resolve dynamic-label placeholders against each result's computed statistics.
Traces: cloudwatch.get-metric-data-dynamic-label-is-resolved-server-side.

Cause 3 of 9: a metric-math alarm evaluates nothing, and its evaluated-datapoint records carry a field AWS does not emit.
AWS: recorded behavior only (StateReasonData is an opaque JSON string, @length 0–4000). An alarm whose Metrics array computes an expression evaluates the COMPUTED series: with EvaluationPeriods=1 exactly one entry appears in StateReasonData's evaluatedDatapoints, and each expression entry carries exactly the keys `timestamp` and `value` — no `datapoint` key.
Emulator: the OK verdict evaluated 0 datapoints (`[]`) with EvaluationPeriods=1, and the expression entries carry the extra key `datapoint` alongside `timestamp` and `value`.
Why: a metric-math alarm never reflects its expression, so a derived-rate alarm stays flat regardless of the data; and a consumer parsing StateReasonData sees a schema AWS never produces.
Fix: the alarm evaluation engine (expression series selection) and its StateReasonData writer.
Traces: cloudwatch.alarm-evaluation-engine-metric-math-alarm-evaluates-the-computed-series, cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs.

Cause 4 of 9: composite alarms are never evaluated server-side from their children.
AWS: recorded behavior only (PutCompositeAlarm models AlarmRule as a string). A composite alarm derives its state from its AlarmRule over the children's current states, with no SetAlarmState call on the composite itself: two OK children under an AND rule put the composite in OK, an unsatisfied rule keeps it OK and fires no action, and an ActionsSuppressor in ALARM suppresses the composite's actions.
Emulator: the composite stays `INSUFFICIENT_DATA` with two OK children, and the three sibling composite checks (AND-rule over all children, boolean AND logic, actions-suppressed-by-alarm) never observe any composite state transition and exhaust the 60s check budget — the awaited state change never arrives.
Why: composite alarms — the standard way to de-noise paging — never change state, so neither notification nor suppression can be exercised.
Fix: the composite-alarm evaluator (AlarmRule evaluation against child state, and the ActionsSuppressor gate) in the alarm state engine.
Traces: cloudwatch.composite-alarm-server-side-evaluation, cloudwatch.composite-alarm-and-rule-requires-all-children, cloudwatch.composite-alarm-boolean-and-logic, cloudwatch.composite-alarm-actions-suppressed-by-alarm.

Cause 5 of 9: alarm history records a StateUpdate for a state that did not change.
AWS: recorded behavior only (DescribeAlarmHistory's HistoryItemType enum includes StateUpdate/ConfigurationUpdate/Action). A SetAlarmState call that sets the alarm to the state it is already in is not a transition and writes no StateUpdate history item; StateUpdatedTimestamp therefore differs from the configuration-updated timestamp.
Emulator: the history contains self-transitions — `[('OK', 'OK'), ('ALARM', 'ALARM')]`.
Why: audit tooling and flap detection that count StateUpdate items over-count transitions that never happened.
Fix: the SetAlarmState/alarm-state transition writer must suppress the history item when old and new state are equal.
Traces: cloudwatch.alarm-state-transitioned-vs-updated-timestamp.

Cause 6 of 9: Unit and StorageResolution are not part of a datapoint's read identity, and distribution-less aggregates answer percentile queries.
AWS: recorded behavior only for the read-identity rules (Unit is a closed enum on the write path; GetMetricStatistics/GetMetricData expose Unit and Period). (a) A MetricStat that OMITS Unit resolves to exactly one unit series (the alphabetically first unit name present, stable across calls) and never the cross-unit sum. (b) A standard-resolution (60s) datum does not produce a sub-minute datapoint at Period=1 the way a StorageResolution=1 datum does. (c) ExtendedStatistics such as p90 over a statistic SET (Minimum/Maximum/Sum/SampleCount, no raw distribution) return NO datapoint — not an interpolation, not Sum/SampleCount, not Maximum. (d) PutMetricData with `Values` and `Counts` of equal length is accepted and aggregated.
Emulator: (a) an omitted Unit returned the cross-unit sum `[107.0]`; (b) the standard-resolution datum produced a Period=1 datapoint; (c) the p90 read returned an entry carrying only `Timestamp` and `Unit`, i.e. a present-but-valueless datapoint rather than none; (d) the histogram publish was refused with `The parameters MetricData.member.2.Values and MetricData.member.2.Counts must be the same length.`
Why: queries silently mix units, high-resolution reads fabricate sub-minute resolution for standard metrics, and percentile panels show a datapoint shell AWS would not return.
Fix: the metric store's datapoint key (namespace, name, dimensions, unit, storage resolution) and the GetMetricStatistics/GetMetricData aggregation path, plus PutMetricData's Values/Counts validation.
Traces: cloudwatch.unit-scoped-metric-query-isolation, cloudwatch.metric-data-high-resolution-period, cloudwatch.percentile-availability-depends-on-the-publication-mode, cloudwatch.values-counts-histogram-query-aggregation.

Cause 7 of 9: GetMetricData's ScanBy refusal is misclassified.
AWS: ScanBy is a closed enum (TimestampAscending/TimestampDescending); an out-of-enum value is refused as a ValidationError whose message does not quote the rejected value back in the constraint sentence AWS uses for this member.
Emulator: returns ('ValidationError', "1 validation error detected: Value 'NotARealScanByValue' at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampDescending, TimestampAscending]") — generic constraint text with the enum rendered in reverse declaration order.
Why: a client matching on AWS's wording for the ScanBy refusal does not recognise it.
Fix: GetMetricData's ScanBy validation and its message template (declaration-order enum rendering).
Traces: cloudwatch.metric-data-timestamp-ordering.

Cause 8 of 9: metric-stream state is not maintained — LastUpdateDate is frozen, streams are not region-scoped, and GetMetricStream misclassifies a bad name.
AWS: recorded behavior only for the timestamps. StopMetricStreams/StartMetricStreams advance `LastUpdateDate`; a metric stream is a regional resource, so the same stream NAME in two regions is two independent streams with independent filters and ARNs; GetMetricStream for a name violating the stream-name pattern answers `InvalidParameterValue`.
Emulator: LastUpdateDate did not advance after StopMetricStreams (identical before/after); a us-west-2 GetMetricStream returned the us-east-1 stream, ARN `arn:aws:cloudwatch:us-east-1:...:metric-stream/<name>` and the other region's IncludeFilters; and the bad-name GetMetricStream answered `ResourceNotFoundException`.
Why: a multi-region deployment's streams collide into one object, so a change in one region silently rewrites the other; and stop/start audit trails show no change.
Fix: the metric-stream state store must key on (region, account, name) and stamp LastUpdateDate on every state transition; GetMetricStream's error mapping.
Traces: cloudwatch.metric-stream-start-stop-timing, cloudwatch.mstrx-one-name-in-two-regions-is-two-independent-streams, cloudwatch.get-metric-stream-invalid-name-pattern-rejected.

Cause 9 of 9: the dataset/KMS operations answer the wrong error class and invented messages, and PutMetricAlarm fabricates an empty-vs-absent Dimensions distinction the Query protocol cannot express.
AWS: GetDataset for a dataset the caller cannot see answers `AccessDenied`, not a not-found; DisassociateDatasetKmsKey for a dataset with no CMK association answers the fixed text `No CMK association found for dataset default.` (fixed sentence with the dataset identifier from the request). Separately, PutMetricAlarm is served over the AWS Query protocol, where an empty list and an omitted list serialize to byte-identical bodies (no `Dimensions.member.N` keys either way) — so DescribeAlarms must report the two alarms with an IDENTICAL Dimensions representation.
Emulator: GetDataset answers `ResourceNotFoundException`; DisassociateDatasetKmsKey answers `the dataset default has no associated customer managed KMS key`; and DescribeAlarms emits the `Dimensions` key for the alarm created with `Dimensions=[]` but omits it for the one created with Dimensions absent (`(True, False)`), i.e. it tracks a "field was set" bit the wire never carried.
Why: the authorization/not-found distinction is inverted for datasets, and a client diffing DescribeAlarms output sees a spurious change between two requests that are identical on the wire.
Fix: the dataset handlers' error mapping and messages, and PutMetricAlarm's input decoding/DescribeAlarms serializer (normalise an absent list to the same representation as an empty one).
Traces: cloudwatch.get-dataset-nonexistent-not-found, cloudwatch.get-dataset-not-found-for-unknown-dataset, cloudwatch.disassociate-dataset-kms-key-nonexistent-dataset-not-found, cloudwatch.put-metric-alarm-empty-vs-absent-dimensions-distinguished.

## Evidence
Representative expected-vs-actual from the replay:

1) cloudwatch.cw-get-metric-data-math-expression-is-computed-server-side — 'm1 + mghost over a never-written mghost returned [], expected [4.0]'
   cloudwatch.metric-math-multi-metric-fill-rate-query — 'expected [11.0, 20.0, 43.0], got [11.0, 43.0]'
2) cloudwatch.get-metric-data-dynamic-label-is-resolved-server-side — Label returned as 'min ${MIN} max ${MAX} sum ${SUM} n ${DATAPOINT_COUNT} of ${LABEL}' with placeholders ['${MIN}','${MAX}','${SUM}','${DATAPOINT_COUNT}','${LABEL}'] unresolved
3) cloudwatch.alarm-evaluation-engine-metric-math-alarm-evaluates-the-computed-series — 'expression evaluatedDatapoint fields [\'datapoint\', \'timestamp\', \'value\']' (expected {timestamp, value})
   cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs — 'the OK verdict evaluated 0 datapoints []; EvaluationPeriods=1 means exactly one'
4) cloudwatch.composite-alarm-server-side-evaluation — 'expected the composite to server-evaluate to OK purely from two OK children (no SetAlarmState was ever called on the composite), got INSUFFICIENT_DATA'; cloudwatch.composite-alarm-and-rule-requires-all-children, cloudwatch.composite-alarm-boolean-and-logic, cloudwatch.composite-alarm-actions-suppressed-by-alarm — 'Check timed out after 60.0s' with no composite transition ever observed
5) cloudwatch.alarm-state-transitioned-vs-updated-timestamp — 'the alarm history must never record a self-transition ... got [(\'OK\', \'OK\'), (\'ALARM\', \'ALARM\')]'
6) cloudwatch.unit-scoped-metric-query-isolation — 'a MetricStat that omits Unit must resolve to exactly ONE unit series ... and never the 107.0 cross-unit sum; got [107.0]'
   cloudwatch.metric-data-high-resolution-period — 'a standard-resolution (60s) datum must not produce a sub-minute (Period=1) datapoint ... got [{Sum: 42.0, Unit: Count}]'
   cloudwatch.percentile-availability-depends-on-the-publication-mode — "ExtendedStatistics=['p90'] over a statistic set ... returned [{'Timestamp': ..., 'Unit': 'Count'}]; the documented answer is NO datapoint"
   cloudwatch.values-counts-histogram-query-aggregation — 'The parameters MetricData.member.2.Values and MetricData.member.2.Counts must be the same length.'
7) cloudwatch.metric-data-timestamp-ordering — got ('ValidationError', "1 validation error detected: Value 'NotARealScanByValue' at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampDescending, TimestampAscending]")
8) cloudwatch.metric-stream-start-stop-timing — 'expected LastUpdateDate to advance when the stream transitions to stopped, got 2026-10-29 18:02:50.573000+00:00 vs 2026-10-29 18:02:50.573000+00:00'
   cloudwatch.mstrx-one-name-in-two-regions-is-two-independent-streams — the us-west-2 read returned Arn 'arn:aws:cloudwatch:us-east-1:...:metric-stream/<name>' with the other region's IncludeFilters [{'Namespace': 'AWS/Lambda'}]
   cloudwatch.get-metric-stream-invalid-name-pattern-rejected — expected InvalidParameterValue, got 'ResourceNotFoundException'
9) cloudwatch.get-dataset-nonexistent-not-found / cloudwatch.get-dataset-not-found-for-unknown-dataset — expected AccessDenied, got 'ResourceNotFoundException'
   cloudwatch.disassociate-dataset-kms-key-nonexistent-dataset-not-found — expected 'No CMK association found for dataset default.', got 'the dataset default has no associated customer managed KMS key'
   cloudwatch.put-metric-alarm-empty-vs-absent-dimensions-distinguished — AssertionError: (True, False) — 'Dimensions' present on the empty-list alarm, absent on the omitted-list alarm

## Steps to Reproduce
Boot the emulator and replay the behavioral suite restricted to the trace_ids above. Each check publishes its own metrics/alarms into the emulator, then reads them back with GetMetricData / GetMetricStatistics / DescribeAlarms / DescribeAlarmHistory / GetMetricStream / GetDataset and asserts the returned series, state or error. The composite-alarm checks poll DescribeAlarms (AlarmTypes=['CompositeAlarm']) for the derived state and time out because it never changes.

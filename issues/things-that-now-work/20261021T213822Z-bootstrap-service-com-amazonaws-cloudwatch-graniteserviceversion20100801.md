Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CloudWatch emulator

_Filed: 2026-10-21T21:38:22Z_
_Operation: service-com-amazonaws-cloudwatch-graniteserviceversion20100801 | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 55 operations in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no CloudWatch client can do anything. Implement a FIRST, SMALL stateful slice: the metric ingest/read loop plus the metric-alarm lifecycle. Wire protocol is `awsJson1_0` (`POST /`, header `X-Amz-Target: GraniteServiceVersion20100801.<OperationName>`), sigv4 signing name `monitoring` (README.md / OPERATIONS.md "Wire protocol").

Happy path, in order (fields per OPERATIONS.md):

1. **PutMetricData** — input `Namespace` (required, len 1..255, pattern `^[^:]`), `MetricData` (list<MetricDatum>: `MetricName` required, `Dimensions`, `Timestamp`, `Value`, `StatisticValues`, `Values`/`Counts`, `Unit`, `StorageResolution`). Empty output. Must APPEND the datapoints into the per-(namespace, metric name, dimension set) time series, defaulting a missing `Timestamp` to now; must not discard `Unit` or `Dimensions`.
2. **ListMetrics** — input `Namespace`, `MetricName`, `Dimensions` (list<DimensionFilter>, 0..10), `NextToken`, `RecentlyActive`. Output `Metrics` (list<Metric>: `Namespace`, `MetricName`, `Dimensions`), `NextToken`. Must enumerate ONLY the metric keys actually created by prior PutMetricData calls, filtered by the supplied namespace/name/dimension filters.
3. **GetMetricStatistics** — input `Namespace`, `MetricName` (required), `Dimensions`, `StartTime`, `EndTime`, `Period` (required, >=1), `Statistics` (1..5 of Average|Maximum|Minimum|SampleCount|Sum), `Unit`. Output `Label` (= MetricName), `Datapoints` (list<Datapoint>: `Timestamp`, `SampleCount`, `Average`, `Sum`, `Minimum`, `Maximum`, `Unit`). Must bucket the stored datapoints into `Period`-second windows between `StartTime` (inclusive) and `EndTime` (exclusive), aggregate each window, and emit ONLY the requested `Statistics` members — computed from stored values, never canned. A window with no data must be omitted entirely.
4. **PutMetricAlarm** — input `AlarmName` (required), `MetricName`, `Namespace`, `Statistic`, `Dimensions`, `Period`, `EvaluationPeriods`, `DatapointsToAlarm`, `Threshold`, `ComparisonOperator`, `AlarmDescription`, `ActionsEnabled`, `AlarmActions`/`OKActions`/`InsufficientDataActions`, `TreatMissingData`, `Unit`, `Tags`. Empty output. Must create-or-REPLACE the alarm under its name, assign `AlarmArn` (`arn:aws:cloudwatch:<region>:<account>:alarm:<AlarmName>`), set `AlarmConfigurationUpdatedTimestamp`, and initialise `StateValue` to `INSUFFICIENT_DATA` with a `StateUpdatedTimestamp`.
5. **DescribeAlarms** — input `AlarmNames` (0..100), `AlarmNamePrefix`, `AlarmTypes`, `StateValue`, `MaxRecords` (1..100), `NextToken`. Output `MetricAlarms`, `CompositeAlarms`, `LogAlarms`, `NextToken`. Must return the alarms as stored (every field given to PutMetricAlarm round-trips), honour name/prefix/state filters, cap at `MaxRecords` and emit a `NextToken` when more remain (rejecting a malformed token with `InvalidNextToken`, HTTP 400).
6. **SetAlarmState** — input `AlarmName` (required), `StateValue` (required: ALARM|INSUFFICIENT_DATA|OK), `StateReason` (required), `StateReasonData`. Empty output; `ResourceNotFound` (HTTP 404) for an unknown alarm. Must mutate the stored alarm so a following DescribeAlarms shows the new `StateValue`/`StateReason`/`StateUpdatedTimestamp`.
7. **DeleteAlarms** — input `AlarmNames` (required, 0..100). Empty output; `ResourceNotFound` (HTTP 404) when any named alarm does not exist (and then nothing is deleted). After deletion DescribeAlarms must no longer return them.

NO canned, echoed or hardcoded responses: each of these must compute its reply from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"), and each must delete its `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line.

## State Model

In `src/state.rs` (`ComAmazonawsCloudwatchGraniteserviceversion20100801State`):

- `metrics: HashMap<MetricKey, Vec<StoredDatum>>` where
  - `MetricKey { namespace: String, metric_name: String, dimensions: BTreeMap<String,String> }` (dimension set normalised/sorted so the same set keys the same series regardless of request order),
  - `StoredDatum { timestamp: SystemTime, value: f64, sample_count: f64, sum: f64, min: f64, max: f64, unit: Option<StandardUnit>, storage_resolution: i32 }` — a plain `Value` becomes `sample_count=1, sum=min=max=value`; `StatisticValues` and `Values`/`Counts` are folded into the same fields so GetMetricStatistics has one aggregation path.
- `alarms: HashMap<String, MetricAlarmRecord>` keyed by alarm name, with
  `MetricAlarmRecord { alarm_name, alarm_arn, alarm_description, actions_enabled, ok_actions, alarm_actions, insufficient_data_actions, namespace, metric_name, dimensions, statistic, extended_statistic, period, unit, evaluation_periods, datapoints_to_alarm, threshold, comparison_operator, treat_missing_data, metrics, tags, state_value, state_reason, state_reason_data, state_updated_timestamp, alarm_configuration_updated_timestamp }`.
- `alarm_history: HashMap<String, Vec<AlarmHistoryItem>>` — append a `ConfigurationUpdate` item on PutMetricAlarm and a `StateUpdate` item on SetAlarmState so DescribeAlarmHistory can be layered on later without re-plumbing.

Everything outside this list (dashboards, metric streams, insight rules, anomaly detectors, alarm mute rules, OTel enrichment, resource metrics configurations, tagging, GetMetricData, PutCompositeAlarm/PutLogAlarm) stays at 501 and is filed as individual gap issues later.

## Evidence

All 55 files under `src/ops/` carry `//! SCAFFOLDED, NOT IMPLEMENTED:` at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501 (verified by grep over the whole ops directory; e.g. `put_metric_data.rs`, `list_metrics.rs`, `get_metric_statistics.rs`, `put_metric_alarm.rs`, `describe_alarms.rs`, `set_alarm_state.rs`, `delete_alarms.rs`). Operation contracts and field names are taken from the crate's generated `OPERATIONS.md` (PutMetricData lines 303-307, ListMetrics 235-240, GetMetricStatistics 184-188, PutMetricAlarm 297-301, DescribeAlarms 94-99, SetAlarmState 315-319, DeleteAlarms 45-49) and from the Smithy model shapes `MetricDatum`, `Datapoint` and `Metric` (smithy_query). The protocol/target-header facts are from `OPERATIONS.md` "Wire protocol". No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI CloudWatch client at it (endpoint prefix `monitoring`).
2. `PutMetricData` with Namespace="MyApp", MetricData=[{MetricName:"Latency", Dimensions:[{Name:"Service",Value:"api"}], Value:100, Unit:"Milliseconds", Timestamp:T0}, {MetricName:"Latency", Dimensions:[{Name:"Service",Value:"api"}], Value:300, Unit:"Milliseconds", Timestamp:T0+30s}] — expect 200 with an empty body; today it returns HTTP 501 NotImplemented.
3. `ListMetrics` with Namespace="MyApp" — expect exactly one Metric {Namespace:"MyApp", MetricName:"Latency", Dimensions:[{Service:api}]}.
4. `GetMetricStatistics` Namespace="MyApp", MetricName="Latency", Dimensions=[{Service:api}], StartTime=T0, EndTime=T0+300s, Period=300, Statistics=["Average","Sum","Maximum","SampleCount"] — expect Label="Latency" and one Datapoint with SampleCount=2, Sum=400, Average=200, Maximum=300, Unit="Milliseconds"; no Minimum member since it was not requested.
5. `PutMetricAlarm` AlarmName="latency-high", Namespace="MyApp", MetricName="Latency", Dimensions=[{Service:api}], Statistic="Average", Period=300, EvaluationPeriods=1, Threshold=250, ComparisonOperator="GreaterThanThreshold" — expect 200.
6. `DescribeAlarms` with AlarmNames=["latency-high"] — expect one MetricAlarm whose AlarmArn, Threshold=250, ComparisonOperator, Period, Statistic and Dimensions match what was sent and whose StateValue is INSUFFICIENT_DATA.
7. `SetAlarmState` AlarmName="latency-high", StateValue="ALARM", StateReason="manual" then `DescribeAlarms` again — expect StateValue=ALARM with StateReason="manual" and an updated StateUpdatedTimestamp.
8. `DescribeAlarms` with AlarmNamePrefix="latency" and MaxRecords=1 after creating a second alarm — expect 1 alarm plus a NextToken, and passing that token returns the second; a garbage NextToken must fail with InvalidNextToken (HTTP 400).
9. `DeleteAlarms` AlarmNames=["latency-high"] then `DescribeAlarms` — expect the alarm gone; deleting a non-existent name must fail with ResourceNotFound (HTTP 404).

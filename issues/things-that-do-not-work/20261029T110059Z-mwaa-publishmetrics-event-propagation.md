Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 1ff82dd4da31f037aed5d48a06cbab4eda93dc7c

# PublishMetrics stores datapoints in a private per-environment map instead of the CloudWatch store its model names as the destination

_Filed: 2026-10-29T11:00:59Z_
_Operation: PublishMetrics | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `1ff82dd4da31f037aed5d48a06cbab4eda93dc7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PublishMetrics` is documented as publishing environment health metrics *to Amazon CloudWatch*, and this repo emulates CloudWatch metrics (`put_metric_data.rs`, `list_metrics.rs`, `get_metric_statistics.rs`, `get_metric_data.rs` in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/` are all implemented, not scaffolds).

The MWAA handler instead appends every accepted datum to a map that lives on its own `EnvironmentRecord` (`EnvironmentRecord.metrics: EnvironmentMetrics`) and is read by NO operation of any emulated service: not by MWAA (its own handler doc says so) and not by CloudWatch's `ListMetrics` / `GetMetricStatistics` / `GetMetricData`, which read the CloudWatch state struct. A caller that publishes a datum and then reads the documented destination sees nothing, and the stored copy is additionally discarded when the environment's teardown retires the record — so the datapoints a successful 200 claims to have published are observable to nobody.

This is not a behaviour the tree never attempts: several services in this repo publish their own metrics by appending directly into the CloudWatch store through `cloudwatch::state::service_state(engine)` (EventBridge Pipes, Lambda, Bedrock Runtime), and this very crate already reaches into another service's state module the same way (`state.rs` uses `sts_state::` accessors for the caller identity). The already-open sibling issue `20261024T132525Z-mwaa-createenvironment-event-propagation` establishes the same class for this crate's other CloudWatch destination (log groups).

## Evidence

Model (destination is CloudWatch) — `smithy_query(query="operation",operation="PublishMetrics")` / the handler's own transcribed contract, `services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/publish_metrics.rs:69`:
    //! Internal only. Publishes environment health metrics to Amazon CloudWatch.
and `MetricDatum`'s doc: "Collects Apache Airflow metrics. To learn more about the metrics published to Amazon CloudWatch, see Amazon MWAA performance metrics in Amazon CloudWatch".

What the handler actually does — `services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/publish_metrics.rs:376-384`:
            stored
                .metrics
                .record_all(&batch)
                .map_err(|rejection| match rejection {
and the store it writes into, `services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:947-950`:
    /// The environment health metrics published for this environment (`PublishMetrics`), by
    /// series. On the record, so they are scoped to the one environment they describe and go
    /// when its teardown retires it.
    pub metrics: EnvironmentMetrics,

The handler doc admits the data reaches no reader — `publish_metrics.rs:19-20`:
    //! The model marks this operation internal, so nothing it accepts is reported by another
    //! operation of this service: what it answers is the modelled empty output at HTTP 200.

The pattern this crate skips, quoted from a sibling service that implements it — `services/service-com-amazonaws-pipes-pipes/src/state/metrics.rs:15-20`:
    //! The datapoints are appended to the CloudWatch store directly rather than sent through
    //! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own
    //! them ... It
    //! goes into the very store `ListMetrics` and `GetMetricStatistics` read, through
    //! CloudWatch's own bounded append, so one metric and its reader cannot disagree.
and its accessor, `services/service-com-amazonaws-pipes-pipes/src/state/metrics.rs:40-44`:
    pub fn cloudwatch_state<Eng: Send + Sync + 'static>(
        engine: &Eng,
    ) -> Option<::engine::ServiceState<CloudWatchState>> {
        ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(engine)
The same cross-crate reach already exists in MWAA for identity — `services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:22`:
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

Test coverage: `services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/published_metrics.rs` asserts the datapoints only by reaching into MWAA's own state (`.series_of(metric, &dimension_map(dimensions))`, line 212; `record.metrics.datapoint_count()`, line 221) — there is no assertion anywhere that a published metric is readable through CloudWatch, which is precisely the delivery that is missing.

## Steps to Reproduce

1. Mount the MWAA emulator and the CloudWatch emulator (`service-com-amazonaws-cloudwatch-graniteserviceversion20100801`) on the SAME shared engine, as `services/service-com-amazonaws-pipes-pipes/tests/all/pipe_metrics.rs:80` does for Pipes.
2. `PUT /environments/MyEnv` with the required members — the create answers 200 with the environment ARN.
3. `POST /metrics/environments/MyEnv` with `MetricData=[{"MetricName":"QueuedTasks","Timestamp":<now>,"Value":5,"Unit":"Count","Dimensions":[{"Name":"Function","Value":"Executor"}]}]` — the handler answers 200 and `EnvironmentRecord.metrics` now holds the datapoint.
4. Call CloudWatch `ListMetrics` (and `GetMetricStatistics` / `GetMetricData` for the MWAA metric `QueuedTasks` with that dimension) on the same engine: nothing is reported — no metric is listed and no datapoint is returned, although the operation's own doc says the metric was published to Amazon CloudWatch.
5. `DELETE /environments/MyEnv`, wait for the teardown to settle, and observe that the stored copy is gone with the record, so the published datapoints are unreachable for good.

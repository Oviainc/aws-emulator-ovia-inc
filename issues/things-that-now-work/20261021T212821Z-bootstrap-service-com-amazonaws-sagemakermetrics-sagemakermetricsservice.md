Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the SageMaker Metrics emulator

_Filed: 2026-10-21T21:28:21Z_
_Operation: service-com-amazonaws-sagemakermetrics-sagemakermetricsservice | Class: stub_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The SageMaker Metrics service (`SDK id: SageMaker Metrics`, restJson1, signing name `sagemaker`, endpoint prefix `metrics.sagemaker`) binds exactly two operations and BOTH are unmodified scaffolds that answer HTTP 501 and hold no state. The bootstrap slice is therefore the service's whole core loop: write metrics, then read them back.

## Happy path (implement in this order)

1. **BatchPutMetrics** — `PUT /BatchPutMetrics`. Input: `TrialComponentName` (string, 1..120, pattern `^[a-z0-9](-*[a-z0-9]){0,119}$`, required) and `MetricData` (list<RawMetricData>, 1..10, required). Each `RawMetricData` carries required `MetricName` (1..255), required `Timestamp`, required `Value` (double) and optional `Step` (integer, >= 0). Output: `Errors` (list<BatchPutMetricsError> with `Code` in {METRIC_LIMIT_EXCEEDED, INTERNAL_ERROR, VALIDATION_ERROR, CONFLICT_ERROR} and `MetricIndex`). On the happy path every datum is accepted and the response omits `Errors` (or returns an empty list); a datum that fails per-item validation (e.g. blank MetricName, negative Step) must be reported as a per-index `BatchPutMetricsError` with `VALIDATION_ERROR` while the remaining data points are still persisted — the batch must be partially successful, not all-or-nothing.

2. **BatchGetMetrics** — `POST /BatchGetMetrics`. Input: `MetricQueries` (list<MetricQuery>, 1..100, required). Each `MetricQuery` carries required `MetricName`, required `ResourceArn` (SageMaker resource ARN, pattern `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:[a-z\-].*/`), required `MetricStat` (Min|Max|Avg|Count|StdDev|Last), required `Period` (OneMinute|FiveMinute|OneHour|IterationNumber), required `XAxisType` (IterationNumber|Timestamp), plus optional `Start`/`End` (long). Output: `MetricQueryResults` (list<MetricQueryResult>, 1..100, positionally aligned with the input queries) where each result has required `Status` (Complete|Truncated|InternalError|ValidationError), required `XAxisValues` (list<long>), required `MetricValues` (list<double>) and optional `Message`.

BatchGetMetrics must compute each result FROM the data persisted by BatchPutMetrics:
- Resolve `ResourceArn` to a stored series: the trial-component name is the last ARN path segment (e.g. `arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component/my-trial-comp` -> `my-trial-comp`), matched against the `TrialComponentName` used on put.
- Select the data points for `MetricName`, filtered by `Start`/`End` when present (against timestamp for `XAxisType=Timestamp`, against `Step` for `IterationNumber`).
- Bucket the points by `Period` (OneMinute/FiveMinute/OneHour buckets on timestamp; IterationNumber = one bucket per `Step`) and reduce each bucket with `MetricStat` (Min, Max, Avg, Count, StdDev, Last) to produce one `MetricValues` entry per bucket, with the matching bucket key in `XAxisValues` (epoch seconds for Timestamp, step number for IterationNumber). `XAxisValues` and `MetricValues` must be the same length and ascending in x.
- A query for an unknown resource/metric returns `Status=Complete` with empty value lists (not an HTTP error); a query whose fields violate the model (bad ARN shape, Start > End) returns `Status=ValidationError` with a `Message`, and that per-query failure must not fail the sibling queries in the batch.

Every response must be derived from persisted state — no canned `MetricQueryResult`, no echoing of the request, no empty-success-with-nothing-stored. A BatchPutMetrics followed by BatchGetMetrics with the equivalent ARN must return exactly the values that were put, aggregated as requested, and the data must survive across requests (shared engine state via `crate::state::service_state(&*self.engine)`, never a fresh `InMemoryEngine`).

## State Model

```
SageMakerMetricsState {
    // trial component name (from BatchPutMetrics.TrialComponentName,
    // also the final path segment of MetricQuery.ResourceArn)
    //   -> that component's metrics
    components: HashMap<String, TrialComponentMetrics>,
}

TrialComponentMetrics {
    name: String,              // TrialComponentName as written
    // metric name -> time/step ordered series
    series: HashMap<String, Vec<MetricDatum>>,
}

MetricDatum {
    metric_name: String,       // RawMetricData.MetricName
    timestamp: SystemTime,     // RawMetricData.Timestamp
    step: Option<i32>,         // RawMetricData.Step (None when omitted)
    value: f64,                // RawMetricData.Value
}
```

Per-series insertion keeps the vector sorted by (timestamp, step) so BatchGetMetrics bucketing and `Last` are well defined, and a per-resource metric cap is what makes `METRIC_LIMIT_EXCEEDED` reachable.

There is no third operation to defer: this bootstrap slice IS the service, so done = a client can put metrics for a trial component and read them back aggregated by each `MetricStat`/`Period`/`XAxisType` combination.

## Evidence

OPERATIONS.md for service-com-amazonaws-sagemakermetrics-sagemakermetricsservice lists exactly 2 operations: `BatchGetMetrics` (POST /BatchGetMetrics, input MetricQueries 1..100 required, output MetricQueryResults 1..100) and `BatchPutMetrics` (PUT /BatchPutMetrics, input TrialComponentName required + MetricData 1..10 required, output Errors 1..10). Shape detail from the Smithy model (`smithy_query shape`): MetricQuery{MetricName, ResourceArn, MetricStat, Period, XAxisType required; Start/End optional longs}, MetricQueryResult{Status, XAxisValues, MetricValues required; Message optional}, RawMetricData{MetricName, Timestamp, Value required; Step optional, range min 0}, BatchPutMetricsError{Code, MetricIndex}; enums MetricStatistic=[Min,Max,Avg,Count,StdDev,Last], Period=[OneMinute,FiveMinute,OneHour,IterationNumber], XAxisType=[IterationNumber,Timestamp], MetricQueryResultStatus=[Complete,Truncated,InternalError,ValidationError], PutMetricsErrorCode=[METRIC_LIMIT_EXCEEDED,INTERNAL_ERROR,VALIDATION_ERROR,CONFLICT_ERROR]. Both handler files still carry the scaffold marker: `src/ops/batch_put_metrics.rs:4` and `src/ops/batch_get_metrics.rs:4` contain `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, so the service holds zero state today. No tests assert any behavior beyond the generated state_roundtrip example.

## Steps to Reproduce

1. Call BatchPutMetrics with TrialComponentName="my-trial-comp" and MetricData=[{MetricName:"loss", Timestamp:T0, Step:0, Value:1.0}, {MetricName:"loss", Timestamp:T0+1s, Step:1, Value:0.5}, {MetricName:"loss", Timestamp:T0+2s, Step:2, Value:0.25}] — expect HTTP 200 with no Errors entries (today: HTTP 501 NotImplemented).
2. Call BatchGetMetrics with one MetricQuery {MetricName:"loss", ResourceArn:"arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component/my-trial-comp", MetricStat:"Last", Period:"IterationNumber", XAxisType:"IterationNumber"} — expect one MetricQueryResult with Status=Complete, XAxisValues=[0,1,2] and MetricValues=[1.0,0.5,0.25] read back from stored state (today: HTTP 501).
3. Repeat the same BatchGetMetrics with MetricStat="Min" and Period="OneHour", XAxisType="Timestamp" — expect a single bucket whose MetricValues=[0.25] and one epoch-second XAxisValues entry, proving the stat/period aggregation is computed and not canned.
4. Call BatchGetMetrics for MetricName="accuracy" on the same ARN — expect Status=Complete with empty XAxisValues/MetricValues, not an HTTP error.
5. Call BatchGetMetrics with a malformed ResourceArn in query[0] and the valid query above as query[1] — expect query[0] Status=ValidationError with a Message and query[1] still Complete with the stored values (per-query failure isolation).
6. Call BatchPutMetrics again with a second data point batch for the same trial component, then BatchGetMetrics — expect the earlier and later points both present, proving state persists across requests.

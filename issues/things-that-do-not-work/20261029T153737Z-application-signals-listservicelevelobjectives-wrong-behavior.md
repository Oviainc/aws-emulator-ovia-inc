Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08

# MetricSourceType is derived from TotalRequestCountMetric alone, so a request-based SLO whose only queries sit in MonitoredRequestCountMetric reports no source type and is dropped from a CloudWatchMetric listing

_Filed: 2026-10-29T15:37:37Z_
_Operation: ListServiceLevelObjectives | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::derive_metric_source_type` is the one derivation of a stored SLO's `MetricSourceType`; `ListServiceLevelObjectives` both REPORTS it (`summary_of` -> `ServiceLevelObjectiveSummary.MetricSourceType`) and FILTERS on it (the `MetricSourceTypes` selector). For a request-based SLO it passes only `total_request_count_metric` as the "does this indicator name CloudWatch metric queries of its own" evidence, and ignores `monitored_request_count_metric`.

`RequestBasedServiceLevelIndicatorMetricConfig` leaves BOTH query members optional, so a create may define its indicator entirely through `MonitoredRequestCountMetric` (the good-count or bad-count queries) with no `KeyAttributes`, no `MetricSource`, no `DependencyConfig` and no `OperationName`. Such a record is stored with its good/bad-count queries intact, yet `metric_source_type` is reached with an empty query list, falls through every branch and answers `None`. The stored SLO therefore reports NO `MetricSourceType` at all, and a `ListServiceLevelObjectives` call filtering `MetricSourceTypes: ["CloudWatchMetric"]` silently omits it — while the exactly parallel period-based SLO (queries in `SliMetricConfig.MetricDataQueries`) is classified `CloudWatchMetric` and is reported.

Two things in this one module contradict that outcome. First, `metric_source_type`'s own doc states the fallback rule it is breaking: "an SLI that names no entity at all is reading plain CloudWatch metrics", and "Reporting `None` would drop a source kind the request did state". Second, the sibling derivation in the same file, `request_based_sli_from_config`, computes "does this config name its OWN metric" over BOTH query members — so one rule in this crate counts `MonitoredRequestCountMetric` as the config naming its own metric and the other does not, and the two can only disagree about the same stored record.

## Evidence

The derivation that drops the member — `services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:1313-1321`:
```
    if let Some(request_based_sli) = request_based_sli {
        let metric = &request_based_sli.request_based_sli_metric;
        return metric_source_type(
            metric.key_attributes.as_ref(),
            metric.operation_name.as_deref(),
            metric.dependency_config.as_ref(),
            metric.metric_source.as_ref(),
            &metric.total_request_count_metric,
        );
    }
```
`metric.monitored_request_count_metric` is never consulted.

The sibling rule in the SAME file counts it — `state.rs:1138-1148` (`request_based_sli_from_config`):
```
            metric_type: reported_metric_type(
                metric.metric_type.as_ref(),
                metric.key_attributes.as_ref(),
                metric.metric_name.as_deref(),
                metric.metric_source.as_ref(),
                metric
                    .total_request_count_metric
                    .as_ref()
                    .is_some_and(|queries| !queries.is_empty())
                    || metric.monitored_request_count_metric.is_some(),
            ),
```

The rule the dropped member defeats — `state.rs:1246-1258` (doc of `metric_source_type`): "They name the six kinds of source an SLI config can describe, so the value follows from which source members the request supplied ... and an SLI that names no entity at all is reading plain CloudWatch metrics." and "Reporting `None` would drop a source kind the request did state". The branch that can no longer fire, `state.rs:1292-1295`:
```
    if !metric_data_queries.is_empty() {
        return Some("CloudWatchMetric".to_string());
    }
    None
```

The model leaves the reachable case open — `smithy_query(query="shape", shape="RequestBasedServiceLevelIndicatorMetricConfig")`: `TotalRequestCountMetric` and `MonitoredRequestCountMetric` are both OPTIONAL, with `MonitoredRequestCountMetric` documented as "Use this structure to define the metric that you want to use as the \"good request\" or \"bad request\" value for a request-based SLO".

Where the wrong value is observed — `src/ops/list_service_level_objectives.rs:276-281` (the `MetricSourceTypes` selector reads the derived value):
```
    if let Some(kinds) = input.metric_source_types.as_ref() {
        let stored = slo.metric_source_type.as_deref();
        if !kinds.iter().any(|kind| stored == Some(kind.as_str())) {
            return false;
        }
    }
```
and `state.rs:1388` (`summary_of`): `metric_source_type: slo.metric_source_type.clone(),`. The value is written by `src/ops/create_service_level_objective.rs:298-299` and re-derived by `src/ops/update_service_level_objective.rs:240-243`, both through the same helper.

Test coverage: the classification suite pins the request-based CloudWatch case only through `TotalRequestCountMetric`. `tests/all/slo_indicator_required.rs:60-75`:
```
/// A request-based SLI config that spells its own CloudWatch metric queries instead of naming
/// an Application Signals entity: a different FORM of the same kind of config.
fn request_based_sli_config_with_queries() -> Value {
    json!({
        "RequestBasedSliMetricConfig": {
            "TotalRequestCountMetric": [{ ... }]
        }
    })
}
```
asserted at `tests/all/slo_indicator_required.rs:245-250` as `expected: Some(("RequestBased", "CloudWatchMetric", "RequestBasedSli"))`. No row supplies `MonitoredRequestCountMetric` alone, so the divergence is unpinned.

## Steps to Reproduce

1. `POST /slo` with `{"Name":"good-count-only","RequestBasedSliConfig":{"RequestBasedSliMetricConfig":{"MonitoredRequestCountMetric":{"GoodCountMetric":[{"Id":"good","MetricStat":{"Metric":{"Namespace":"AWS/ApiGateway","MetricName":"Count"},"Period":60,"Stat":"Sum"}}]}}}}` — no KeyAttributes, no MetricSource, no DependencyConfig, no OperationName, no TotalRequestCountMetric.
2. Observed in the 200 response: `Slo.MetricSourceType` is absent. Expected `"CloudWatchMetric"`, by `metric_source_type`'s own rule (`state.rs:1246-1258`) that an SLI naming no Application Signals entity is reading plain CloudWatch metrics — the indicator does carry CloudWatch metric queries, and the crate's sibling rule at `state.rs:1143-1147` already treats `MonitoredRequestCountMetric` as the config naming its own metric.
3. For contrast, `POST /slo` with `{"Name":"total-count","RequestBasedSliConfig":{"RequestBasedSliMetricConfig":{"TotalRequestCountMetric":[{"Id":"total","MetricStat":{"Metric":{"Namespace":"AWS/ApiGateway","MetricName":"Count"},"Period":60,"Stat":"Sum"}}]}}}}` — this one is reported with `MetricSourceType: "CloudWatchMetric"`.
4. `POST /slos` with body `{"MetricSourceTypes":["CloudWatchMetric"]}`.
5. Observed: `SloSummaries` carries `total-count` but not `good-count-only`, and the summary of `good-count-only` (listed with no filter) carries no `MetricSourceType` — the stored record's own CloudWatch queries are invisible to the selector that should select it.

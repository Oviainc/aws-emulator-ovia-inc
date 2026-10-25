Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 2c9ffd59ad5f58010cf4df26da4b1c3c27b76a2c

# SliMetricConfig Statistic, PeriodSeconds and MetricName are dropped, leaving MetricDataQueries empty in every response

_Filed: 2026-10-24T00:15:09Z_
_Operation: CreateServiceLevelObjective | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `2c9ffd59ad5f58010cf4df26da4b1c3c27b76a2c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ServiceLevelIndicatorMetricConfig` carries `MetricName`, `Statistic` and `PeriodSeconds` (and the request-based config carries `MetricName`). The handler never stores any of them, and the indicator it builds leaves the `@required` `SliMetric.MetricDataQueries` list EMPTY, so nothing in any later response reflects which metric, which statistic or which evaluation period the caller asked for.

Expected: on the real service these three members are materialised into the indicator's `MetricDataQueries` — one `MetricDataQuery` whose `MetricStat` names the metric and carries `Period` = the requested `PeriodSeconds` and `Stat` = the requested `Statistic` — so a `GetServiceLevelObjective` after a create reports the metric the SLO evaluates, and a Get → Update round trip preserves it.
Actual: `Sli.SliMetric.MetricDataQueries` is `[]` for every SLO that did not pass `MetricDataQueries` verbatim, and `Statistic`/`PeriodSeconds`/`MetricName` appear nowhere in the stored record or any response — they are accepted and silently discarded. The request-based path has the same shape: when `MonitoredRequestCountMetric` is omitted the stored indicator reports the required member as `GoodCountMetric: []`, an empty good-count list the real service never returns.

The same two helpers serve `UpdateServiceLevelObjective`, so an update that supplies a new `SliConfig` loses the same members.

## Evidence

Spec: `smithy_query(query="shape", shape="ServiceLevelIndicatorMetricConfig")` defines `Statistic` ("The statistic to use for comparison to the threshold"), `PeriodSeconds` (range 60..900, "The number of seconds to use as the period for SLO evaluation") and `MetricName` ("the name of the CloudWatch metric to use for the SLO, when using a custom metric"); the output shape `ServiceLevelIndicatorMetric` has no members for them, so `MetricDataQueries` (`@required`) is the only place they can surface, and `RequestBasedServiceLevelIndicatorMetric.MonitoredRequestCountMetric` is `@required` too.
Handler/state: `src/state.rs:469-498` (`sli_from_config`) copies only key_attributes/operation_name/metric_type/metric_data_queries/dependency_config/metric_source/composite_sli_config — `metric.statistic` and `metric.period_seconds` are never read, and `metric_data_queries` falls back to `unwrap_or_default()` (line 485). `src/state.rs:509-545` does the same for the request-based config and defaults `monitored_request_count_metric` to `GoodCountMetric(Vec::new())` (lines 532-537). Called from `src/ops/create_service_level_objective.rs:268-272`.
Tests: `tests/all/slo_lifecycle.rs:1028-1083` pins the current choice ("expected an empty MetricDataQueries rather than a synthesised query") for a config carrying `MetricName`/`Statistic`/`PeriodSeconds`, i.e. the divergence is deliberate and would need that row updated; no test asserts that the requested statistic or period is recoverable from any response.

## Steps to Reproduce

1. POST /slo with {"Name":"latency-p99","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"},"MetricType":"LATENCY","Statistic":"p99","PeriodSeconds":60},"MetricThreshold":250,"ComparisonOperator":"GreaterThan"}}.
2. Inspect the response: Slo.Sli.SliMetric.MetricDataQueries is [] and no field anywhere carries Stat=p99 or Period=60; expected a MetricDataQueries entry whose MetricStat carries Period 60 and Stat p99.
3. GET /slo/latency-p99 — the same record comes back with the statistic and period still absent, so a client cannot tell what the SLO evaluates.
4. POST /slo with a RequestBasedSliConfig that omits MonitoredRequestCountMetric and observe RequestBasedSli.RequestBasedSliMetric.MonitoredRequestCountMetric reported as {"GoodCountMetric": []}.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5a975aba0b3519330cf325164f4e0d2eee8fa87d

# Enforce the per-query-type MetricName values documented on each widget query's `metrics`

_Filed: 2026-10-29T01:49:12Z_
_Operation: CreateDashboard | Class: validation_

> _Note: this issue was identified against the code base at commit `5a975aba0b3519330cf325164f4e0d2eee8fa87d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A widget's `configs[].queryParameters` is a union, and each member documents WHICH `MetricName` values are valid for its own `metrics` list: `costAndUsage` admits seven cost/usage metrics, `savingsPlansCoverage` admits `SpendCoveredBySavingsPlans` alone, and `reservationCoverage` admits `Hour`, `Unit` and `Cost`. The modelled `MetricName` enum is the UNION of all eleven values, so the boundary validator can only check membership in that union — it cannot tell a `costAndUsage` query asking for `SpendCoveredBySavingsPlans` (or `Hour`, or `Unit`) from a legal one. `CreateDashboard` does not apply the per-member rule either: `crate::state::widget_list_refusal` already walks every widget and every config and resolves the union member through `query_time_range`, but it validates only the query's `timeRange`, so a dashboard whose cost-and-usage widget asks for a Savings Plans coverage metric is created with HTTP 200 and read back by `GetDashboard`.

The crate's own data honours the rule it does not enforce: every `costAndUsage` query in the AWS-managed catalog uses `UnblendedCost` and the `savingsPlansCoverage` entry uses `SpendCoveredBySavingsPlans` — so the store holds only valid combinations until a caller writes one.

`UpdateDashboard` runs the same helper (`src/ops/update_dashboard.rs:204-212`), so it shares the gap and a fix in `widget_list_refusal` covers both dashboard writes. The existing widget-validation test (`tests/all/widget_time_range_validation.rs`) and the lifecycle fixtures only ever submit metrics that are valid for the query they sit in (`tests/all/dashboard_lifecycle.rs:213-227` builds `costAndUsage` configs from `UnblendedCost`/`AmortizedCost`/`NetUnblendedCost`), so nothing pins the acceptance of a mismatched metric.

## Evidence

Smithy, `smithy_query(query="shape",shape="CostAndUsageQuery")` — member `metrics`:
  "<p>The specific cost and usage metrics to retrieve.</p> <note> <p>Valid values for CostAndUsageQuery metrics are <code>AmortizedCost</code>, <code>BlendedCost</code>, <code>NetAmortizedCost</code>, <code>NetUnblendedCost</code>, <code>NormalizedUsageAmount</code>, <code>UnblendedCost</code>, and <code>UsageQuantity</code>.</p> </note>"
`SavingsPlansCoverageQuery$metrics`: "<note> <p>Valid value for SavingsPlansCoverageQuery metrics is <code>SpendCoveredBySavingsPlans</code>.</p> </note>"
`ReservationCoverageQuery$metrics`: "<note> <p>Valid values for ReservationCoverageQuery metrics are <code>Hour</code>, <code>Unit</code>, and <code>Cost</code>.</p> </note>"
Why the boundary cannot do it — `smithy_query(query="shape",shape="MetricName")`: `"type": "enum", "values": ["AmortizedCost","BlendedCost","NetAmortizedCost","NetUnblendedCost","NormalizedUsageAmount","UnblendedCost","UsageQuantity","SpendCoveredBySavingsPlans","Hour","Unit","Cost"]` — the union of all three lists, and every query's `metrics` targets that one shape (`MetricNames`).

Handler/helper, `src/state.rs:1820-1843` (`widget_list_refusal`, the one place a dashboard write judges a widget):
  "for (index, widget) in widgets.iter().enumerate() {
       for (config_index, config) in widget.configs.iter().enumerate() {
           let (query_member, range) = query_time_range(&config.query_parameters);
           … if let Some(message) = date_time_range_refusal(&path, range) { return Some(message); }"
It already resolves which union member the config carries — `src/state.rs:1784-1800`, `fn query_time_range` matches all five members — and checks nothing but the time range; its own doc (`src/state.rs:1802-1816`) lists exactly "Two rules": the time range, and widget-id distinctness.

Called from `src/ops/create_dashboard.rs:209-215` ("The rules a WIDGET's own configuration has to satisfy, which the boundary validator cannot state either") and `src/ops/update_dashboard.rs:204-212`.

The crate's own catalog keeps the rule it does not enforce — `src/state.rs:314-321`: `QueryParameters::costAndUsage(CostAndUsageQuery { metrics: vec!["UnblendedCost".to_string()], … })`; `src/state.rs:393-395`: `QueryParameters::savingsPlansCoverage(SavingsPlansCoverageQuery { … metrics: Some(vec!["SpendCoveredBySavingsPlans".to_string()]), … })`.

Test coverage today: `tests/all/widget_time_range_validation.rs:73` (`"metrics": ["UnblendedCost"]` inside a `costAndUsage` query) and `tests/all/dashboard_lifecycle.rs:213-227` — only valid pairings, so no test pins the current acceptance.

## Steps to Reproduce

1. Call CreateDashboard with name="MetricMismatch" and one widget whose config is `{"queryParameters": {"costAndUsage": {"metrics": ["SpendCoveredBySavingsPlans"], "timeRange": {"startTime": {"type": "RELATIVE", "value": "-P3M"}, "endTime": {"type": "RELATIVE", "value": "P0D"}}, "granularity": "MONTHLY"}}, "displayConfig": {"table": {}}}`.
2. Expected per the model note on `CostAndUsageQuery$metrics` (valid values are AmortizedCost, BlendedCost, NetAmortizedCost, NetUnblendedCost, NormalizedUsageAmount, UnblendedCost, UsageQuantity): HTTP 400 ValidationException naming `widgets[0].configs[0].queryParameters.costAndUsage.metrics`. Actual: HTTP 200 with a minted dashboard ARN.
3. GetDashboard on that ARN — the stored widget reads back with the invalid metric.
4. Repeat with `{"savingsPlansCoverage": {"metrics": ["UnblendedCost"], …}}` and with `{"reservationCoverage": {"metrics": ["UsageQuantity"], …}}` — both accepted as well.
5. Send the same widget list to UpdateDashboard on an existing dashboard — also accepted, since it runs the same `crate::state::widget_list_refusal`.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b1d79d167dfa72d8fff60e6631ca11c09e39e94

# ListServices summaries carry empty MetricReferences and no ServiceGroups

_Filed: 2026-10-24T04:04:13Z_
_Operation: ListServices | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b1d79d167dfa72d8fff60e6631ca11c09e39e94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `ServiceSummary` `ListServices` returns is built with `metric_references: no_metric_references()` (a hardcoded empty list) and `service_groups: None` / `attribute_maps: None`, independent of what the store holds.

Expected: `ServiceSummary.MetricReferences` is `@required` and the real service reports the Application Signals metrics associated with each discovered service; `ServiceSummary.ServiceGroups` reports the groups the service belongs to "based on the configured grouping attributes", so after a `PutGroupingConfiguration` whose definitions declare a `DefaultGroupingValue`, each listed service should carry a group for it (`GroupSource` = `DEFAULT`).

Actual: both are always empty/absent, so a `PutGroupingConfiguration` write is invisible to the list of services it is meant to group, and no page of this list ever names a metric even though the emulator derives its service set from SLOs whose SLI metric configs name namespace/metric name/metric type.

## Evidence

Spec: `ServiceSummary.MetricReferences` is `@required` (`smithy_query shape="ServiceSummary"`); `ServiceSummary.ServiceGroups` doc "An array of service groups that this service belongs to, based on the configured grouping attributes"; `GroupingAttributeDefinition.DefaultGroupingValue` doc "This ensures all services have a grouping value even if the source data is missing".

Handler: `src/ops/list_services.rs` `handle` -> `state.lock().observed_services(...)`; `src/state.rs` `observed_services` maps each identity to `ServiceSummary { key_attributes, attribute_maps: None, service_groups: None, metric_references: no_metric_references() }`.

Tests: `tests/all/list_services.rs` asserts identities, windowing, paging and the account selector only — nothing asserts metric references or service groups.

## Steps to Reproduce

1. POST /slo creating an SLO over KeyAttributes {"Type":"Service","Name":"checkout"} with an SliMetricConfig naming namespace AWS/ApplicationSignals / MetricName Latency
2. PUT /grouping-configuration with [{"GroupingName":"BusinessUnit","GroupingSourceKeys":["business_unit"],"DefaultGroupingValue":"Unassigned"}]
3. GET /services?StartTime=<now-3600>&EndTime=<now>
4. Observe each ServiceSummaries entry has MetricReferences == [] and no ServiceGroups, despite the stored SLO metric and the stored grouping configuration

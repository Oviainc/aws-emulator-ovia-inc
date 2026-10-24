Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b1d79d167dfa72d8fff60e6631ca11c09e39e94

# GetService reports no MetricReferences and never reflects the stored grouping configuration in ServiceGroups

_Filed: 2026-10-24T04:04:13Z_
_Operation: GetService | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b1d79d167dfa72d8fff60e6631ca11c09e39e94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetService` answers with a `Service` whose `MetricReferences` is ALWAYS an empty list and whose `ServiceGroups`/`AttributeMaps`/`LogGroupReferences` are always absent, for every entity and every window — the values are hardcoded in `crate::state` (`no_metric_references()` returns `Vec::new()`, the other three are `None`) rather than computed from anything the store holds.

Two parts are computable from state the emulator already owns:

1. `ServiceGroups` — "An array of service groups that this service belongs to, based on the configured grouping attributes." `PutGroupingConfiguration` persists `GroupingAttributeDefinitions` (each with a `GroupingName` and an optional `DefaultGroupingValue`, which the model documents as the value that "ensures all services have a grouping value even if the source data is missing"). So once a grouping configuration is written, every reported service should carry a `ServiceGroup` per definition that declares a default (`GroupName` = `GroupingName`, `GroupValue` = `DefaultGroupingValue`, `GroupSource` = `DEFAULT`). Today a `PutGroupingConfiguration` is observable only through `ListGroupingAttributeDefinitions` and changes nothing about the services it is supposed to group.

2. `MetricReferences` is `@required` on `Service`, and the real service returns the standard Application Signals metric references (namespace + metric name + metric type, e.g. Latency/Fault/Error) for a discovered service. The emulator derives its discovered services from stored SLOs whose `SliMetricConfig.MetricDataQueries` already name a namespace, a metric name and a metric type (the same values `MetricSourceType` is derived from), so at least one reference per derived entity is computable instead of the empty list.

Additionally, for `KeyAttributes` that match no stored SLO the handler echoes the request's own `KeyAttributes` back as a discovered service, so a caller cannot distinguish a discovered service from a typo'd identity.

## Evidence

Spec: `Service.MetricReferences` is `@required` (`smithy_query shape="Service"`), doc "An array of structures that each contain information about one metric associated with this service"; `Service.ServiceGroups` doc "An array of service groups that this service belongs to, based on the configured grouping attributes"; `GroupingAttributeDefinition.DefaultGroupingValue` doc "The default value to use for this grouping attribute when no value can be derived from the source keys. This ensures all services have a grouping value even if the source data is missing"; `ServiceGroup` requires `GroupName`/`GroupValue`/`GroupSource`/`GroupIdentifier` with `GroupSource` documented as "such as TAG, OTEL, or DEFAULT".

Handler: `src/ops/get_service.rs` `handle` -> `state.lock().observed_service(...)`; `src/state.rs` `observed_service` builds `Service { key_attributes: reported, attribute_maps: None, service_groups: None, metric_references: no_metric_references(), log_group_references: None }`, and `no_metric_references()` is a hardcoded empty `Vec`. `get_service.rs` also sets the output's `log_group_references: None` unconditionally.

Tests: `tests/all/get_service.rs::get_service_reports_the_entity_its_key_attributes_identify` asserts only the key-attribute projection; no test asserts metric references or service groups, and `tests/all/grouping_configuration.rs` exercises grouping config only through `ListGroupingAttributeDefinitions`.

## Steps to Reproduce

1. POST /slo creating an SLO whose SliConfig.SliMetricConfig names KeyAttributes {"Type":"Service","Name":"checkout","Environment":"eks:prod"} plus MetricDataQueries in namespace AWS/ApplicationSignals with MetricName Latency
2. PUT /grouping-configuration with GroupingAttributeDefinitions=[{"GroupingName":"BusinessUnit","GroupingSourceKeys":["business_unit"],"DefaultGroupingValue":"Unassigned"}]
3. POST /service?StartTime=<now-3600>&EndTime=<now> with KeyAttributes {"Type":"Service","Name":"checkout","Environment":"eks:prod"}
4. Observe Service.MetricReferences == [] and no Service.ServiceGroups entry for BusinessUnit/Unassigned, although the SLO named the metric and the grouping configuration declares a default value

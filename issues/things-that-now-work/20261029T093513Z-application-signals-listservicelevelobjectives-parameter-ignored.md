Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 0184958fe7a8a2f43d9a0a5d46b055181f8d4624

# The `MetricSource` filter compares only `MetricSourceKeyAttributes` and silently drops `MetricSourceAttributes`, unlike the `DependencyConfig` filter beside it which the crate compares member for member

_Filed: 2026-10-29T09:35:13Z_
_Operation: ListServiceLevelObjectives | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `0184958fe7a8a2f43d9a0a5d46b055181f8d4624`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`matches_filters` states its own rule: "Whether a stored SLO passes every selector the request supplied... An absent selector filters nothing." Two of its selectors are whole structures, and the crate handles them differently.

The `DependencyConfig` selector goes through `crate::state::same_dependency`, whose doc says the shape "carries no `PartialEq`, so the comparison is written out over its two members" — and it does compare both (`dependency_operation_name` and `dependency_key_attributes`).

The `MetricSource` selector reads only ONE of its two members. The modelled `MetricSource` carries `MetricSourceKeyAttributes` (required) and `MetricSourceAttributes` ("Additional attributes for the metric source."), and the stored side holds both — `sli_from_config`/`request_based_sli_from_config` clone the whole `MetricSource` onto the record, and `SloEntity.metric_source` hands the whole structure to the filter. The filter nevertheless projects `|source| &source.metric_source_key_attributes` on both sides, so a request that narrows by `MetricSourceAttributes` has that half of its selector dropped: SLOs whose stored `MetricSourceAttributes` differ from the requested ones — or that carry none at all — are still reported. A PRESENT selector filters nothing, which the function's own doc says only an ABSENT one does.

## Evidence

THE MODEL MEMBER THAT IS DROPPED — `smithy_query(query="shape", shape="MetricSource")`:
```
"MetricSourceKeyAttributes": {"target": "Attributes", "required": true, "doc": "Key attributes that identify the metric source."},
"MetricSourceAttributes": {"target": "Attributes", "length": {"min": 1, "max": 4}, "doc": "Additional attributes for the metric source."}
```

THE SIDE THAT COMPARES A STRUCTURE-VALUED SELECTOR WHOLE — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:1824-1831:
```
/// Whether two `DependencyConfig`s name the same dependency. The shape carries no
/// `PartialEq`, so the comparison is written out over its two members.
pub fn same_dependency(left: Option<&DependencyConfig>, right: &DependencyConfig) -> bool {
    left.is_some_and(|left| {
        left.dependency_operation_name == right.dependency_operation_name
            && left.dependency_key_attributes == right.dependency_key_attributes
    })
}
```

BOTH SELECTORS IN ONE FUNCTION, SIX LINES APART — src/ops/list_service_level_objectives.rs:247-260:
```
    if let Some(dependency) = input.dependency_config.as_ref() {
        if !crate::state::same_dependency(entity.dependency_config.as_ref(), dependency) {
            return false;
        }
    }
    if let Some(metric_source) = input.metric_source.as_ref() {
        let stored = entity
            .metric_source
            .as_ref()
            .map(|source| &source.metric_source_key_attributes);
        if !crate::state::attributes_contain(stored, &metric_source.metric_source_key_attributes) {
            return false;
        }
    }
```
(`metric_source_attributes` appears on neither side)

THE RULE THE FUNCTION STATES — src/ops/list_service_level_objectives.rs:226-230:
```
/// Whether a stored SLO passes every selector the request supplied.
///
/// Each selector names part of the entity an SLO's SLI identifies, so it is matched against
/// the entity read out of the stored record — never against the request that created it.
/// An absent selector filters nothing.
```

THE STORED SIDE DOES HOLD IT — src/ops/types/shared_types.rs:89-94:
```
#[smithy_macros::smithy(shape = "com.amazonaws.applicationsignals#MetricSource")]
#[derive(Debug, Clone, Default)]
pub struct MetricSource {
    pub metric_source_key_attributes: std::collections::HashMap<String, String>,
    pub metric_source_attributes: Option<std::collections::HashMap<String, String>>,
}
```
and src/state.rs:1066 / 1125 (`metric_source: metric.metric_source.clone()`), src/state.rs:1303 (`pub metric_source: Option<MetricSource>` on `SloEntity`) — the whole structure is stored and handed to the filter.

TEST COVERAGE: no test sends `MetricSourceAttributes` — `grep -rn MetricSourceAttributes tests/` returns nothing. tests/all/slo_lifecycle.rs:825-828 drives the `MetricSource` filter with `MetricSourceKeyAttributes` only, so adding the second member's comparison leaves every existing assertion unchanged.

## Steps to Reproduce

1. Start the emulator and point an `application-signals` client at it.
2. POST /slo with {"Name":"rum-slo","SliConfig":{"SliMetricConfig":{"MetricSource":{"MetricSourceKeyAttributes":{"Type":"AWS::RUM::AppMonitor","Name":"web"},"MetricSourceAttributes":{"Page":"checkout"}},"Statistic":"Average"}}} — 200.
3. POST /slos with {"MetricSource":{"MetricSourceKeyAttributes":{"Type":"AWS::RUM::AppMonitor","Name":"web"},"MetricSourceAttributes":{"Page":"cart"}}} — observe the `rum-slo` summary in SloSummaries, although the request asked for a different `MetricSourceAttributes` value than the record stores.
4. Repeat with "MetricSourceAttributes":{"NoSuchKey":"NoSuchValue"} — the same SLO is still reported, so the member narrows nothing at all.
5. Contrast with the sibling structure-valued selector: POST /slos with a `DependencyConfig` whose `DependencyOperationName` differs from the stored one correctly selects nothing (src/state.rs:1824-1831 compares both members).

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# ListServiceDependencies reports every edge with an empty MetricReferences list

_Filed: 2026-10-24T06:55:27Z_
_Operation: ListServiceDependencies | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Each `ServiceDependency` is built with a hardcoded empty metric-reference list.

Expected: `ServiceDependency.MetricReferences` is `@required` and documented as the metrics "associated with this service dependency that was discovered by Application Signals" — the real service reports the Latency/Fault/Error metric references for the edge (namespace, metric name, metric type and the RemoteService/RemoteOperation dimensions).

Actual: the member is always `[]` regardless of the stored SLO's `SliMetricConfig`/`DependencyConfig`, so the edge is reported without any of the metrics that are the point of the operation.

## Evidence

Spec: `smithy_query shape="ServiceDependency"` — `MetricReferences` (required), doc as quoted.

Handler: `src/ops/list_service_dependencies.rs` `handle` -> `state.lock().observed_dependencies(...)`; `src/state.rs` `observed_dependencies` inserts `ServiceDependency { ..., metric_references: no_metric_references() }` (unconditional `Vec::new()`).

Tests: `tests/all/time_ranged_reads.rs` (~line 622) asserts the current empty list (`"MetricReferences": []`), pinning the emulator's behavior rather than AWS's. Filed separately from the existing `ListServices` MetricReferences issue, which covers only that operation.

## Steps to Reproduce

1. POST /slo with SliConfig.SliMetricConfig naming KeyAttributes {"Type":"Service","Name":"checkout","Environment":"prod"}, OperationName "POST /checkout", MetricType LATENCY and DependencyConfig {DependencyKeyAttributes: payments, DependencyOperationName: "Charge"}
2. POST /service-dependencies?StartTime=<now-3600>&EndTime=<now+3600> with body {"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"}}
3. Observe ServiceDependencies[0].MetricReferences == []

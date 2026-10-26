Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# ListServiceOperations reports every operation with an empty MetricReferences list

_Filed: 2026-10-24T06:55:27Z_
_Operation: ListServiceOperations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Each `ServiceOperation` the handler reports is built with a hardcoded empty metric-reference list, independent of what the store holds.

Expected: `ServiceOperation.MetricReferences` is `@required` and documented as "an array of structures that each contain information about one metric associated with this service operation that was discovered by Application Signals" — the real service reports the standard Application Signals metrics (Latency / Fault / Error in the `ApplicationSignals` namespace, with the operation and service dimensions) for every discovered operation. The emulator derives its operation set from stored SLOs whose `SliMetricConfig` names a metric type/metric name, so a reference can be projected from that state.

Actual: `MetricReferences` is always `[]`, so no caller can ever resolve a metric for a listed operation and the response carries no information beyond the operation name.

## Evidence

Spec: `smithy_query shape="ServiceOperation"` — `MetricReferences` (MetricReferences, required) with the doc quoted above; `MetricReference` requires Namespace, MetricType and MetricName.

Handler: `src/ops/list_service_operations.rs` `handle` -> `state.lock().observed_operations(...)`; `src/state.rs` `observed_operations` maps each name to `ServiceOperation { name, metric_references: no_metric_references() }`, where `no_metric_references()` returns `Vec::new()` unconditionally.

Tests: `tests/all/time_ranged_reads.rs` (~line 585) asserts the emulator's current empty lists (`{ "Name": "GET /cart", "MetricReferences": [] }`), i.e. it pins the divergence rather than the AWS behavior. The sibling gap is already filed for `ListServices` (20261024T040413Z-application-signals-listservices-wrong-behavior) but covers that operation only.

## Steps to Reproduce

1. POST /slo with Name=checkout-latency and SliConfig.SliMetricConfig naming KeyAttributes {"Type":"Service","Name":"checkout","Environment":"prod"}, OperationName "POST /checkout" and MetricType LATENCY
2. POST /service-operations?StartTime=<now-3600>&EndTime=<now+3600> with body {"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"}}
3. Observe ServiceOperations[0].MetricReferences == [] although the stored SLO names a latency metric for that operation

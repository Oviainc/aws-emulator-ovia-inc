Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# ListServiceDependents reports every dependent with an empty MetricReferences list

_Filed: 2026-10-24T06:55:27Z_
_Operation: ListServiceDependents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Each `ServiceDependent` is built with a hardcoded empty metric-reference list.

Expected: `ServiceDependent.MetricReferences` is `@required` and documented as the metrics "associated with this service dependent that was discovered by Application Signals" — the real service reports the Latency/Fault/Error metric references for the invoking entity's edge.

Actual: the member is always `[]` for every dependent and every window, so the inverse edge carries no metric information even though the emulator knows the SLO metric configuration behind the edge.

## Evidence

Spec: `smithy_query shape="ServiceDependent"` — `MetricReferences` (required), doc as quoted.

Handler: `src/ops/list_service_dependents.rs` `handle` -> `state.lock().observed_dependents(...)`; `src/state.rs` `observed_dependents` builds each `ServiceDependent` with `no_metric_references()` (unconditional `Vec::new()`).

Tests: `tests/all/time_ranged_reads.rs` (dependents block, ~line 635 onwards) asserts identities and operation names only, and the shared `no_metric_references()` helper is asserted empty elsewhere in the same file — nothing asserts AWS's non-empty references.

## Steps to Reproduce

1. POST /slo for checkout with OperationName "POST /checkout" and DependencyConfig naming payments / "Charge"
2. POST /service-dependents?StartTime=<now-3600>&EndTime=<now+3600> with body {"KeyAttributes":{"Type":"Service","Name":"payments","Environment":"prod"}}
3. Observe ServiceDependents[0].MetricReferences == []

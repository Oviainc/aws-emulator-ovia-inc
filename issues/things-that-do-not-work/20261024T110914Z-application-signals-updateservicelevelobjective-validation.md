Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: ee0f6e80c5f72b72cdc0a537b4cc8caa49bfe93a

# Refuse an UpdateServiceLevelObjective that switches an SLO between period-based and request-based

_Filed: 2026-10-24T11:09:14Z_
_Operation: UpdateServiceLevelObjective | Class: validation_

> _Note: this issue was identified against the code base at commit `ee0f6e80c5f72b72cdc0a537b4cc8caa49bfe93a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own model documentation states the rule outright: "You cannot change from a period-based SLO to a request-based SLO, or change from a request-based SLO to a period-based SLO."

Expected: PATCH /slo/{Id} that supplies `RequestBasedSliConfig` for an SLO whose stored `EvaluationType` is `PeriodBased` (or `SliConfig` for one stored as `RequestBased`) is refused with HTTP 400 `ValidationException`, and the stored record is left exactly as it was — the SLO keeps its indicator, its `EvaluationType` and its `MetricSourceType`, and a following Get shows no change.

Actual: the handler applies the switch and answers 200. Supplying `RequestBasedSliConfig` sets `slo.request_based_sli`, clears `slo.sli` to null, rewrites `slo.evaluation_type` to `RequestBased` and re-derives `MetricSourceType`; the mirror branch does the same the other way. The only cross-member rule the handler enforces is the "not both in one call" exclusion, which is a different rule (it is about one request naming two configs, not about a request changing the stored kind). The result is an SLO silently converted to the other evaluation kind — a mutation the real service rejects — and every downstream projection (`ListServiceLevelObjectives` summaries, the topology reads that read the entity out of the indicator) follows the converted record.

## Evidence

Spec: `smithy_query(query="doc", shape="UpdateServiceLevelObjective")` -> "Updates an existing service level objective (SLO). If you omit parameters, the previous values of those parameters are retained. You cannot change from a period-based SLO to a request-based SLO, or change from a request-based SLO to a period-based SLO." The model declares `ValidationException` (HTTP 400) on this operation, so the refusal has a modeled home.

Handler: `src/ops/update_service_level_objective.rs:199-217` — `if let Some(config) = input.sli_config { slo.sli = Some(...); slo.request_based_sli = None; slo.evaluation_type = Some(PERIOD_BASED) }` and the symmetric `request_based_sli_config` branch at 205-210, with no comparison against the stored `evaluation_type`. Only the both-configs-in-one-call check exists (lines 169-173).

Tests: `tests/all/slo_lifecycle.rs:531-539` patches a period-based SLO with `RequestBasedSliConfig` and `tests/all/slo_lifecycle.rs:599-610` asserts the record "switched kinds" (`EvaluationType == "RequestBased"`, `Sli` null). So no test asserts the spec rule — the current test pins the divergent behavior and would need to assert the 400 instead.

## Steps to Reproduce

1. POST /slo with {"Name":"kind-switch","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"},"MetricType":"LATENCY"},"MetricThreshold":250,"ComparisonOperator":"GreaterThan"}} and observe EvaluationType = PeriodBased.
2. PATCH /slo/kind-switch with {"RequestBasedSliConfig":{"RequestBasedSliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"}}}}.
3. Observe HTTP 200 and a response whose Slo.EvaluationType is now RequestBased with Slo.Sli removed; expected HTTP 400 ValidationException naming the documented restriction.
4. GET /slo/kind-switch — the converted record is what is stored; expected the original period-based record untouched.

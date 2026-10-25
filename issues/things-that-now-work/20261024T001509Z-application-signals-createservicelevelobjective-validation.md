Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2c9ffd59ad5f58010cf4df26da4b1c3c27b76a2c

# Reject a CreateServiceLevelObjective that supplies no SLI config at all (and more than the documented 50 tags)

_Filed: 2026-10-24T00:15:09Z_
_Operation: CreateServiceLevelObjective | Class: validation_

> _Note: this issue was identified against the code base at commit `2c9ffd59ad5f58010cf4df26da4b1c3c27b76a2c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateServiceLevelObjective` only refuses the case where BOTH `SliConfig` and `RequestBasedSliConfig` are supplied. It accepts a create that supplies NEITHER, and persists an SLO with no indicator at all: `Sli` and `RequestBasedSli` both absent, `EvaluationType` absent, `MetricSourceType` absent. Every later read (`GetServiceLevelObjective`, `ListServiceLevelObjectives`) then reports that indicator-less record.

Expected: an SLO is defined by its service level indicator — the operation's own documentation states that "SLOs use service level indicators (SLIs) to calculate whether the application is performing at the level that you want" and that `EvaluationType` "displays whether this is a period-based SLO or a request-based SLO", a classification that can only come from one of the two configs. A create that names no indicator is refused by the real service with the modelled `ValidationException` (HTTP 400) and nothing is stored.
Actual: HTTP 200 and a stored SLO whose `Sli`, `RequestBasedSli`, `EvaluationType` and `MetricSourceType` are all absent — a record the real service can never produce.

Second symptom of the same class on the same operation: the `Tags` member documents "You can associate as many as 50 tags with an SLO", while the modelled list bound is 0..200. The handler applies no tag-count rule of its own, so a create carrying 51..200 tags is accepted (HTTP 200) where the real service refuses it.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateServiceLevelObjective")` — `SliConfig` and `RequestBasedSliConfig` are both optional in the model (the exclusion and the "at least one" rule are prose-only), `ValidationException` (HTTP 400) is a declared error, and `ServiceLevelObjective.EvaluationType` is documented as "Displays whether this is a period-based SLO or a request-based SLO". `Tags` doc (same operation): "You can associate as many as 50 tags with an SLO" vs the modelled `TagList` `length 0..200`.
Handler: `src/ops/create_service_level_objective.rs:219-311` — the only cross-member check is `if input.sli_config.is_some() && input.request_based_sli_config.is_some()` (line 222); the record is then built with `sli`/`request_based_sli` as `Option` and `evaluation_type = None` when both are absent (lines 268-296). `input.tags` is stored unconditionally (lines 300-307) with no count check.
Tests: `tests/all/slo_lifecycle.rs:450` (`both_sli_configs_in_one_call_is_refused`) covers only the "both supplied" direction; no test creates an SLO with neither config, and no test exercises the tag count. So nothing pins the current acceptance.

## Steps to Reproduce

1. POST /slo with body {"Name": "no-indicator"} (no SliConfig, no RequestBasedSliConfig, no Goal).
2. Observe HTTP 200 with Slo.Arn/CreatedTime set but Slo.Sli, Slo.RequestBasedSli, Slo.EvaluationType and Slo.MetricSourceType all absent; expected HTTP 400 ValidationException and nothing stored.
3. GET /slo/no-indicator — the indicator-less record is reported back, confirming it was persisted.
4. POST /slo with a valid SliConfig and 60 Tags entries; observe HTTP 200 where the real service refuses a resource carrying more than 50 tags.

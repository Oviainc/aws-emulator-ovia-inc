Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# CreateAnomalyDetector accepts labels map keys that violate PrometheusMetricLabelKey (including the reserved __ prefix)

_Filed: 2026-10-24T18:42:09Z_
_Operation: CreateAnomalyDetector | Class: validation_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAnomalyDetectorRequest$labels` targets `PrometheusMetricLabelMap`, whose key is `PrometheusMetricLabelKey`, modelled `@length(min: 1, max: 7168)` and `@pattern("^(?!__)[a-zA-Z_][a-zA-Z0-9_]*$")` — i.e. a Prometheus label name that must NOT begin with the reserved `__` prefix. Being a map key, that constraint is not applied at the request boundary (`vendor/smithy-schema-server/src/constraints.rs:569-570`: "Map *keys* are not covered"); on top of that the pattern uses a negative lookahead, which the Rust `regex` crate the constraint walk compiles with (`constraints.rs:415-435`) cannot express at all.

Expected: `labels` carrying `__name__`, `__anything`, `1bad`, `bad-label` or an empty key is refused 400 `ValidationException` naming the member, and no detector is created.

Actual: `src/ops/create_anomaly_detector.rs:310` stores `input.labels` verbatim (`input.labels.unwrap_or_default().into_iter().collect()`) after validating only the `tags` prefix rule and the RandomCutForest query; the call answers 202 and `DescribeAnomalyDetector` reports the illegal label names back, so a client that mislabels a detector is told it succeeded.

## Evidence

Spec: `com.amazonaws.amp#PrometheusMetricLabelKey` @length(1,7168) @pattern("^(?!__)[a-zA-Z_][a-zA-Z0-9_]*$"); CreateAnomalyDetector models `ValidationException` (HTTP 400); Prometheus reserves the `__` label prefix (well-established). Framework: `vendor/smithy-schema-server/src/constraints.rs:569-570` and :1089-1100 (map keys not validated) — the same reason this crate hand-checks label-name keys for `limitsPerLabelSet` in `src/ops/update_workspace_configuration.rs:366-372`. Handler: `src/ops/create_anomaly_detector.rs:206-222` (validates only tags + configuration) and :310 (labels stored unchecked). Tests: `tests/all/anomaly_detector_lifecycle.rs:223-373` round-trips only well-formed labels; no case sends an illegal label name.

## Steps to Reproduce

1. Create a workspace and wait for ACTIVE.
2. POST /workspaces/{workspaceId}/anomalydetectors with {"alias":"d1","configuration":{"randomCutForest":{"query":"sum(up)"}},"labels":{"__name__":"x"}} — expected 400 ValidationException naming `labels` and no detector created, actual 202.
3. Repeat with {"labels":{"bad-label":"x"}} and {"labels":{"1bad":"x"}} — both expected 400, both accepted.
4. GET /workspaces/{workspaceId}/anomalydetectors/{anomalyDetectorId} — the illegal label names are reported as persisted.

Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 57d818e145087aac91ee9b85e99c9525e7c9e7c0

# ListAuditFindings never consults AuditTargets/Auditors/DetailLevel and always reports zero findings

_Filed: 2026-10-24T12:16:44Z_
_Operation: ListAuditFindings | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `57d818e145087aac91ee9b85e99c9525e7c9e7c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AuditTargets` is `@required` (1..10 entries) and selects the entities to audit; `Auditors` selects which auditors run (`slo`, `operation_metric`, `service_quota`, `trace`) and `DetailLevel` (BRIEF | DETAILED) selects how much of each finding is reported. The emulator's handler reads none of them, and does not touch the service state at all (`let _ = (ctx, &self.engine);`): `AuditFindings` is the empty list for every target, every auditor selection and every window, so the response is identical whatever the caller asks about.

At minimum the `slo` auditor's targets are facts this emulator does hold — an `AuditTarget` of type `slo` names a stored SLO, and the store knows whether that SLO exists — so the answer could depend on the request (a target that names no stored SLO versus one that does). As implemented the operation cannot distinguish them and always answers 200 with `AuditFindings: []`, which makes it indistinguishable from an audit that found nothing on every target.

## Evidence

Spec: `smithy_query(query="operation", operation="ListAuditFindings")` shows `AuditTargets` required with length 1..10, plus the optional `Auditors` and `DetailLevel` inputs, and `AuditFindings` required (len ..10) in the output. Handler: `src/ops/list_audit_findings.rs:179-215` validates only the time range and pages an empty `Vec::new()` through `crate::state::page_of`; `input.audit_targets`, `input.auditors` and `input.detail_level` are never read, and the engine is explicitly discarded at line 184. Tests: no test file asserts any `ListAuditFindings` finding content (only the shared time-range refusal path in `tests/all/time_ranged_reads.rs`).

## Steps to Reproduce

1. Create an SLO: POST /slo with `{"Name":"audited-slo","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"},"MetricType":"LATENCY"}}}`.
2. POST /auditFindings?StartTime=<1h ago>&EndTime=<now> with `{"AuditTargets":[{"Type":"slo","Data":{"ServiceLevelObjective":{"SloName":"audited-slo"}}}],"Auditors":["slo"],"DetailLevel":"DETAILED"}`.
3. Repeat the same call with an `AuditTargets` entry naming an SLO that was never created, and with `"Auditors":["trace"]`.
4. Expected: the response depends on the targets/auditors asked about. Actual: byte-identical 200 responses with `AuditFindings: []` in every case.

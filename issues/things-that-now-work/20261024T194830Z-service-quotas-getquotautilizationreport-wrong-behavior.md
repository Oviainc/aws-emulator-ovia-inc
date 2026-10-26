Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# Quota utilization is a hardcoded zero: every report record reports 0% whatever the account actually consumes

_Filed: 2026-10-24T19:48:30Z_
_Operation: GetQuotaUtilizationReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A quota utilization report is supposed to be the one place Service Quotas reports how much of each quota an account is actually using. In this emulator the usage term of the formula is a compile-time constant zero, so the report is a hollow computation: the `Utilization` of every record that carries one is exactly `0.0`, forever, and nothing a client does to the emulated account can change it.

Expected (AWS): `StartQuotaUtilizationReport` "analyzes your quota usage across all Amazon Web Services services" and `GetQuotaUtilizationReport` returns the records "showing your quota usage ... sorted by utilization percentage in descending order (highest utilization first)"; `QuotaUtilizationInfo.Utilization` is documented as "the utilization percentage of the quota, calculated as (current usage / applied value) x 100. Values range from 0.0 to 100.0 or higher if usage exceeds the quota limit". For an account that holds, say, 4 VPCs against a default of 5, the `vpc`/`L-F678F1CE` record comes back at 80.0, and the report's descending order puts the quotas close to their limit on the first page — which is the entire point of the feature (and of Automatic Management's notifications, which are driven by the same utilization).

Actual (emulator): usage is not read from any state at all. `src/state.rs` declares `const RECORDED_QUOTA_USAGE: f64 = 0.0;` (line 253) and `utilization_pct` (lines 1349-1357) returns `Some(RECORDED_QUOTA_USAGE / applied * 100.0)` for a quota with a usage metric and `None` for one without, so `utilization_snapshot` (lines 918-946) freezes a record set in which every reported `Utilization` is `0.0` and the rest are absent. Two consequences a client observes: (1) no quota is ever reported as approaching its limit, however many resources exist in the emulator's other services (the repo ships stateful EC2/S3/Lambda/etc. emulators whose resource counts are exactly the usage source Service Quotas tracks through the `AWS/Usage` metrics this crate already seeds into `MetricInfo`); (2) the documented "highest utilization first" ordering degenerates to a constant — with every value equal to 0.0 the order collapses onto the `(ServiceCode, QuotaCode)` tie-break in `compare_utilization`, so the sort conveys no information and a client that reads only the first page of a large report sees an arbitrary alphabetical slice rather than its hottest quotas.

This is not the known catalog-content gap: the values here are wrong even for the `(ServiceCode, QuotaCode)` rows the catalog does hold correctly, because the numerator is a constant rather than state.

## Evidence

Model/spec: `GetQuotaUtilizationReport` doc — "This operation returns paginated results showing your quota usage across all Amazon Web Services services, sorted by utilization percentage in descending order (highest utilization first)"; `StartQuotaUtilizationReport` doc — "analyzes your quota usage across all Amazon Web Services services"; `QuotaUtilizationInfo.Utilization` doc (smithy_query shape `QuotaUtilizationInfo`) — "The utilization percentage of the quota, calculated as (current usage / applied value) x 100. Values range from 0.0 to 100.0 or higher if usage exceeds the quota limit." The output members `AppliedValue`/`DefaultValue` are populated from real state, so `Utilization` is the one computed field in the record and it is the one that is canned.

Handler/state: `src/ops/get_quota_utilization_report.rs` lines 187-259 serves `report.quotas` verbatim from the frozen snapshot; the snapshot is built by `src/state.rs` `utilization_snapshot` (lines 918-946), whose `utilization: utilization_pct(default, applied)` call reaches `utilization_pct` (lines 1349-1357) using `RECORDED_QUOTA_USAGE` — declared at `src/state.rs` line 253 as `0.0` with the comment "nothing in this emulator consumes a quota, so no consumption has ever been recorded against one". No code path anywhere in the crate writes a usage figure, and the crate consults no other service's state.

Test coverage: `tests/all/utilization_report.rs` lines 335-356 PIN the divergence — the suite recomputes `recorded_usage = 0.0_f64` and asserts the reported `Utilization` equals `0.0 / applied_value * 100.0` — and lines 370-389 assert only a non-increasing order, which a constant trivially satisfies. Those assertions must move with the fix. No existing Service Quotas issue mentions utilization: the open backlog covers the template plane (`...associateservicequotatemplate-wrong-behavior`), `ContextId`/`QuotaContext` (`...getservicequota-parameter-ignored`), cross-region global quotas (`...getservicequota-wrong-behavior`), unasked page size (`...listawsdefaultservicequotas-pagination`, `...listservices-pagination`), catalog content (`20261023T182924Z-...catalog-is-incomplete-and`), the support case (`...requestservicequotaincrease-wrong-behavior`) and auto-management input validation (`...startautomanagement-validation`, `...updateautomanagement-validation`).

## Steps to Reproduce

1. Boot the emulator and create resources that consume a quota whose usage metric Service Quotas tracks — e.g. POST `CreateVpc` four times at the EC2 endpoint (the `vpc` quota `L-F678F1CE` defaults to 5), or create several Lambda functions / S3 buckets.
2. POST `X-Amz-Target: ServiceQuotasV20190624.StartQuotaUtilizationReport` with `{}` and keep the returned `ReportId`.
3. Wait ~1 s (past the report's completion instant) and POST `GetQuotaUtilizationReport` with `{"ReportId":"<id>"}`.
4. Find the record for `{"ServiceCode":"vpc","QuotaCode":"L-F678F1CE"}` in `$.Quotas`: `AppliedValue` is 5 and `Utilization` is `0.0`, where AWS reports 80.0 for four VPCs in use. Every other record with a `Namespace` also reports exactly `0.0`.
5. Walk the report with `MaxResults` smaller than `TotalCount`: because every `Utilization` is identical, the documented "highest utilization first" ordering is really `(ServiceCode, QuotaCode)` ascending, so the first page is an alphabetical slice rather than the account's most-utilized quotas.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# StartQuotaUtilizationReport never reuses a recent report: every call mints a new ReportId

_Filed: 2026-10-24T21:29:54Z_
_Operation: StartQuotaUtilizationReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `StartQuotaUtilizationReport` call unconditionally mints and stores a brand-new report, so two calls made back-to-back answer with two different `ReportId`s and two independent snapshots, and the account accumulates reports until the emulator's own 50-report ceiling refuses the next start with `TooManyRequestsException`.

Expected (AWS): the service may reuse an existing recent report instead of generating another — the modeled `Message` member is documented as "An optional message providing additional information about the report generation status. This field may contain details about the report initiation or indicate if an existing recent report is being reused." A client that calls `StartQuotaUtilizationReport` again while a recently generated report is still alive (reports live 15 minutes, per `GetQuotaUtilizationReport`'s own documentation) therefore gets back the EXISTING `ReportId` — frequently already `COMPLETED` — with a `Message` saying the report is being reused, rather than a fresh `PENDING` report.

Actual (emulator): `src/ops/start_quota_utilization_report.rs` lines 169-198 calls `start_utilization_report(&region, now)`, which (src/state.rs lines 868-893) checks only the stored-report ceiling and then inserts a new report with a freshly derived id, status `PENDING`, and a fresh snapshot; nothing ever looks for a recent report to return. The `Message` is emulator-authored text ("report generation started for N quotas; call GetQuotaUtilizationReport with this ReportId to retrieve the results") and never carries the documented reuse indication. Observable consequences: a client that polls by re-starting a report never converges on one report; repeated starts inflate state and eventually hit a refusal AWS would not make, because AWS would have handed back the recent report instead.

## Evidence

Model: `StartQuotaUtilizationReportResponse$Message` doc (smithy_query doc, shape `StartQuotaUtilizationReportResponse`, member `Message`) states the field "may ... indicate if an existing recent report is being reused" — i.e. reuse of a recent report is part of this operation's contract. `ReportStatus` (`PENDING|IN_PROGRESS|COMPLETED|FAILED`) and the 15-minute retention documented on `GetQuotaUtilizationReport` define the window in which a recent report exists.

Handler/state: src/ops/start_quota_utilization_report.rs lines 156-198 (unconditional mint, invented `Message`); src/state.rs `start_utilization_report` lines 868-893 (no reuse lookup; only `MAX_LIVE_REPORTS_PER_REGION` = 50 gating, mapped by the handler to `TooManyRequestsException`).

Test coverage: tests/all/utilization_report.rs lines 243-273 (`two_reports_are_two_resources`) PINS the divergence — it asserts two starts mint two different ids — and lines 558+ (`the_report_store_is_bounded_and_refuses_past_its_ceiling`) pins the emulator-only ceiling, so both assertions must move with the fix. No existing issue covers this: the open `GetQuotaUtilizationReport` issue (20261024T194830Z) is about the hardcoded 0% utilization in report RECORDS, not about report identity or reuse.

## Steps to Reproduce

1. Boot the emulator and POST `X-Amz-Target: ServiceQuotasV20190624.StartQuotaUtilizationReport` with body `{}`; note `$.ReportId` and `$.Message`.
2. Immediately POST `StartQuotaUtilizationReport` `{}` again: a different `$.ReportId` comes back with `Status: PENDING`, and neither `Message` ever indicates a reused report — AWS may answer the second call with the first call's `ReportId` (already generated) and say so in `Message`.
3. Repeat the call 51 times in the same region: the 51st is refused with `TooManyRequestsException` (emulator ceiling of 50 live reports), a refusal that cannot occur on AWS if recent reports are reused.

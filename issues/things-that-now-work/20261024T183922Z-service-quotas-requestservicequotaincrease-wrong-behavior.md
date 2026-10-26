Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# A quota increase request never opens a support case: Status never reaches CASE_OPENED and CaseId is never populated

_Filed: 2026-10-24T18:39:22Z_
_Operation: RequestServiceQuotaIncrease | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

On the DEFAULT path (`SupportCaseAllowed` absent or true) the emulator mints the request `PENDING` and then, 500 ms later, flips it straight to `APPROVED` and puts `DesiredValue` in force — with no support case at any point. Consequences a client can observe:

1. `RequestedServiceQuotaChange.Status` only ever takes the values `PENDING`, `APPROVED` or `NOT_APPROVED`. The modeled `CASE_OPENED` ("Service Quotas opened a support case to process the quota increase request") and `CASE_CLOSED` statuses are unreachable, so a client that polls `GetRequestedServiceQuotaChange` / `ListRequestedServiceQuotaChangeHistoryByQuota` waiting for the case state, or that filters the history with `Status=CASE_OPENED`, never sees a row AWS does produce for the same workflow.
2. `RequestedServiceQuotaChange.CaseId` is hardcoded absent for every request, approved or not (`request_view` sets `case_id: request.case_id.clone()` and nothing ever writes `case_id`). AWS returns the Support case ID for a request it opened a case for, and `Created` is documented as "the date and time when the quota increase request was received and the case ID was created" — i.e. a case id belongs to the normal request record. A client that hands the `CaseId` to AWS Support (the documented follow-up for `CASE_OPENED`) has nothing to hand on.

Expected: a request submitted with `SupportCaseAllowed` unset/true is received `PENDING`, and once Service Quotas cannot auto-approve it, it moves to `CASE_OPENED` carrying a non-empty `CaseId` before any terminal status; `NOT_APPROVED`/no-case remains the `SupportCaseAllowed=false` outcome only. Actual: `PENDING -> APPROVED`, `CaseId` always null, the increase granted with no review artefact at all.

## Evidence

Model (smithy_query shape `RequestedServiceQuotaChange`): `Status` enum documents `CASE_OPENED` — "Service Quotas opened a support case to process the quota increase request. Follow-up on the support case for more information." — and `CASE_CLOSED`; member `CaseId` (target `CustomerServiceEngagementId`) is documented "The case ID."; member `Created` is "The date and time when the quota increase request was received and the case ID was created." `RequestServiceQuotaIncreaseRequest$SupportCaseAllowed` documents that by default a support case IS opened (the emulator already relies on the false branch of that same sentence for `NOT_APPROVED`). The service also binds `CreateSupportCase`, whose existence presupposes a case id on the request plane.

Handler/state: `src/ops/request_service_quota_increase.rs` module doc lines 9-21 ("`CaseId` stays absent either way: this emulator opens no support case") and `handle()` lines 225-254; `src/state.rs` `record_request` (~line 689) sets `case_id: None` and no code path ever assigns it; `settle_due_requests` (~lines 725-781) maps `support_case_allowed` directly onto `APPROVED`/`NOT_APPROVED` with no intermediate case state; `REQUEST_DECISION_DELAY` = 500 ms (src/state.rs line 163).

Test coverage: `tests/all/support_case_and_templates.rs` lines 228-231 and 266-271 PIN the divergence — "a request carries no CaseId when it is made" and "this emulator opens none for an approved request either" — so those assertions must move with the fix. The recorded AWS behaviour quoted in the resolved issue `20261022T124557Z-service-quotas-ignores-supportcaseallowed-false-...` polls "until the status leaves PENDING/CASE_OPENED", i.e. the real service passes through `CASE_OPENED`; that issue's three causes were the `SupportCaseAllowed=false` outcome, the RequestId error code and the template gate — none of them the case id or the `CASE_OPENED` step, so this is net-new.

## Steps to Reproduce

1. Boot the emulator and POST `X-Amz-Target: ServiceQuotasV20190624.RequestServiceQuotaIncrease` with `{"ServiceCode":"s3","QuotaCode":"L-DC2B2D3D","DesiredValue":20000}` (no `SupportCaseAllowed`, so the documented default True applies).
2. Read `$.RequestedQuota.Status` -> `PENDING`, `$.RequestedQuota.CaseId` -> absent.
3. Wait ~1 s and POST `GetRequestedServiceQuotaChange` with the returned `Id` -> `Status` is `APPROVED` and `CaseId` is still absent; the status never passed through `CASE_OPENED` and no case id was ever reported.
4. POST `ListRequestedServiceQuotaChangeHistoryByQuota` with `{"ServiceCode":"s3","QuotaCode":"L-DC2B2D3D","Status":"CASE_OPENED"}` -> always an empty `RequestedQuotas` list, for any request the emulator can mint.

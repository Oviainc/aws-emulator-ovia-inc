Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: e42d60e9c80231c2e4470b4ab9cec2534ece4a72

# A retried clientToken must replay the original arn, not answer ConflictException

_Filed: 2026-10-24T18:10:24Z_
_Operation: CreateScheduledReport | Class: error-response_

> _Note: this issue was identified against the code base at commit `e42d60e9c80231c2e4470b4ab9cec2534ece4a72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`clientToken` is modeled `@idempotencyToken` and documented as "A unique, case-sensitive identifier that you provide to ensure the idempotency of the request". The handler instead treats the token as an at-most-once create key: the first call records the token, and EVERY later call presenting it is refused with `ConflictException` (HTTP 409).

That makes the member do the opposite of what it is for. Smithy SDKs auto-generate an idempotency token per call and reuse it across transport retries, so a CreateScheduledReport whose response was lost gets 409 on the automatic retry instead of the original success — the caller cannot tell "already created by me" from "a real conflict", and the arn it created is never returned to it.

Expected: a repeated request presenting a token this service has already seen replays that call's outcome — HTTP 200 with the SAME `arn` — and creates no second report. (A conflict answer is appropriate only for a token reused with a materially different request body, which is a separate branch.) Actual: HTTP 409 ConflictException for any reuse of the token, identical body or not.

Note the sibling handler already gets this right: ExecuteScheduledReport replays the prior outcome for the same token+arn and reserves ConflictException for a token reused against a DIFFERENT report (src/ops/execute_scheduled_report.rs), so the two operations in this crate disagree about what an idempotency token means.

## Evidence

Spec: `smithy_query(query="doc",shape="CreateScheduledReportRequest",member="clientToken")` → "to ensure the idempotency of the request"; the member carries `@idempotencyToken`. `ConflictException` is documented as "a conflict with the current state of the resource. For example, attempting to create a resource that already exists or is being created" — not as the answer to a client's own retry. Handler: src/ops/create_scheduled_report.rs, the `if state.scheduled_report_client_tokens.contains_key(token)` branch returns ConflictException unconditionally; src/state.rs documents `scheduled_report_client_tokens` as "an at-most-once create key". Contrast: src/ops/execute_scheduled_report.rs replays `prior.health_status`/`prior.execution_triggered` for the same token. Tests: tests/all/scheduled_report_lifecycle.rs (~line 416) pins the 409 for an identical retry, i.e. the divergent behavior is asserted today and that assertion needs to change with the fix.

## Steps to Reproduce

1. CreateDashboard to get a dashboard ARN.
2. CreateScheduledReport with clientToken="tok-0001" and a valid scheduledReport — note arn A returned with HTTP 200.
3. Send the byte-identical request again with clientToken="tok-0001" (what an SDK transport retry does) — expect HTTP 200 with arn A and still exactly one report; actual: HTTP 409 ConflictException.
4. ListScheduledReports — confirms only one report exists, so the retry had nothing to conflict with.

Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 4af528f360b03ab2b05e9ad5a75be6ebefdc29e9

# Reusing one clientToken across different scheduled reports is refused with an unmodeled-for-this-case ConflictException instead of executing

_Filed: 2026-10-24T21:45:02Z_
_Operation: ExecuteScheduledReport | Class: error-response_

> _Note: this issue was identified against the code base at commit `4af528f360b03ab2b05e9ad5a75be6ebefdc29e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ExecuteScheduledReport treats `clientToken` as if it identified ONE execution of ONE report: the handler stores `ScheduledReportExecution { arn, executed_at, execution_triggered, health_status }` keyed by the token, and when a later call inside the 60s window presents that token for a DIFFERENT report ARN it answers ConflictException (HTTP 409) and triggers nothing.

Expected: the only documented effect of `clientToken` on this operation is suppressing a repeat trigger of the SAME call — "If a clientToken is provided, the service uses it for idempotency. Requests with the same client token will not trigger a new execution within the same minute." Nothing in the model scopes the token across resources, and a request naming a different, ENABLED, existing scheduled report is a valid request: it must trigger that report's execution (HTTP 200 with healthStatus/executionTriggered), not be refused. The modeled ConflictException is documented as "a conflict with the current state of the resource. For example, attempting to create a resource that already exists or is being created" — the state of the second report is not in conflict with anything.

Actual: a client that reuses one token value per batch/run (a very common pattern — one UUID per sweep, looping over its reports) gets HTTP 200 for the first report and HTTP 409 for every other report in the sweep, with no execution triggered and no health refreshed for any of them. The refusal is emulator-invented behavior derived from the per-token ARN the handler chose to store, not from the spec.

## Evidence

Spec: `smithy_query(query="doc", shape="ExecuteScheduledReport")` — "Triggers an immediate execution of a scheduled report, outside of its regular schedule. The scheduled report must be in ENABLED state. Calling this operation on a DISABLED scheduled report returns a ValidationException." plus the note "If a clientToken is provided, the service uses it for idempotency. Requests with the same client token will not trigger a new execution within the same minute." The only documented refusals are the DISABLED ValidationException and ResourceNotFoundException for an unknown ARN; ConflictException's own doc (smithy_query shape=ConflictException) describes a resource-state conflict on creation.

Handler: `src/ops/execute_scheduled_report.rs` lines ~261-283 — after `expire_execution_tokens`, if `state.scheduled_report_executions.get(token)` exists and `prior.arn != input.arn`, it returns `ConflictException { message: "Client token {token} was already used to execute scheduled report {conflicting_arn}" }`. The ARN-vs-token coupling is stated as a choice in `src/state.rs` (`ScheduledReportExecution` doc: "the same token presented for a different report inside the window is a conflicting reuse, not a retry, and is refused"), citing no spec sentence.

Tests: `tests/all/scheduled_report_execution.rs:527-536` PINS the divergence — it asserts the 409 for the cross-report token reuse — so the current suite locks in the invented refusal rather than covering it. (The replay-the-same-ARN half of this handler, lines 263-272, is correct and is not what this issue asks to change; nor does this issue touch the already-filed ExecuteScheduledReport health-derivation and EventBridge-delivery gaps.)

## Steps to Reproduce

1. CreateDashboard with one widget; note the dashboard ARN
2. CreateScheduledReport twice against that dashboard (different names) to get report ARNs R1 and R2, both ENABLED
3. ExecuteScheduledReport { arn: R1, clientToken: "sweep-2026-01" } → HTTP 200, executionTriggered true
4. ExecuteScheduledReport { arn: R2, clientToken: "sweep-2026-01" } within the same minute → expected HTTP 200 with R2's healthStatus and executionTriggered true; observed HTTP 409 ConflictException ("Client token sweep-2026-01 was already used to execute scheduled report R1")
5. GetScheduledReport on R2 → healthStatus still unrefreshed (no lastRefreshedAt) and lastExecutionAt absent, confirming nothing ran for R2

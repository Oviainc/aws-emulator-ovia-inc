Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards.create-scheduled-report-token-reuse-conflict-rejected
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards.create-scheduled-report-token-reuse-conflict-rejected|run#13|AssertionError|assert message == conflict_message, message
Issue-Commit: e8c5c92d9bc397e4014b473476bd2d32121bdaea

# CreateScheduledReport words a reused clientToken as 'creation in progress' and (past a 500ms window) replays a success, instead of refusing every reuse with the already-exists ConflictException

> _Note: this issue was identified against the code base at commit `e8c5c92d9bc397e4014b473476bd2d32121bdaea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduledReport

## Description
ONE root cause (Cause 1 of 1): the clientToken conflict branch is decided by a synthetic in-process "creation window" plus a request fingerprint, rather than by whether the create under that token has COMPLETED.

=== Cause 1 of 1: a clientToken whose create already succeeded is answered with the in-flight wording (and later with a replayed 200) instead of the terminal already-exists conflict ===
AWS: Smithy (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json) — CreateScheduledReport.clientToken targets ClientToken (@length 1..128, @pattern ^[\u0021-\u007E]+$, @idempotencyToken, header X-Amzn-Client-Token); the operation models ConflictException, HTTP 409, documented "attempting to create a resource that already exists or is being created" (ConflictException$message targets GenericString, @length 0..1024). Recorded behavior: once a clientToken has been used to SUCCESSFULLY create a scheduled report, the token is spent — every subsequent CreateScheduledReport presenting it is refused with code ConflictException at HTTP 409 and NO second report is created, whether the rest of the request is materially different (a different scheduledReport.name) or byte-identical to the original. The refusal message for that terminal case is, with its fixed text exactly as AWS words it and its one variable part taken from this service's state (the ARN of the report already created under that token): "A scheduled report with the provided clientToken already exists (ARN: <arn of the report created under that token>). Use a different clientToken to create a new scheduled report, or use the existing report's ARN to update it." The separate in-flight wording ("A scheduled report creation is already in progress for the provided clientToken. Please retry after some time.") belongs only to a duplicate that races a create which has not yet completed — a caller that has already received the first call's 200 is never in that case.
Emulator: the sequential reuse immediately after a successful create is refused ConflictException/409 (status and code are CORRECT) but with the in-flight message: "A scheduled report creation is already in progress for the provided clientToken. Please retry after some time." Source: src/ops/create_scheduled_report.rs:290-308 calls prior.conflict_message(now), and crate::state::ScheduledReportCreation::conflict_message (src/state.rs ~688-701) selects the in-progress text whenever in_progress_at(now) — i.e. within crate::state::SCHEDULED_REPORT_CREATION_WINDOW (src/state.rs:650, a CHOSEN 500ms span, as its own doc comment concedes) of the create — and only otherwise renders the already-exists text that AWS uses here. The same window/fingerprint logic also makes the token NOT spent after it elapses: a reuse past the window whose fingerprint matches is answered with HTTP 200 and the first ARN (src/ops/create_scheduled_report.rs:291-296), a success AWS does not give for a spent token.
Why: the two conflict texts are a contract a client branches on — the in-flight wording tells it to back off and retry the same token (transient), the already-exists wording tells it the token is terminally spent and it must pick a new token or switch to UpdateScheduledReport. A client developed against this emulator will retry a token that can never succeed, and (past the 500ms window) will read a 200 for a request AWS refuses, so duplicate-token handling cannot be developed or tested correctly.
Why this is not covered by an open issue: 20261024T030750Z-bcm-dashboards-createscheduledreport-wrong-behavior.md asks for the OPPOSITE of the recorded behavior (it asks that a reused token replay the original ARN at HTTP 200), and 20261022T215638Z-bcm-dashboards-dashboardname-pattern-unenforced-constraint-not-found.md cause 4 covers only the CONCURRENT racing case (one winner, racers refused with the in-flight wording), which this emulator now satisfies. Neither covers the sequential, post-success reuse this check exercises.
Fix: the CreateScheduledReport handler's idempotency branch plus the message selection it delegates to in this service's state — a creation record must distinguish "still in flight" from "completed", and once completed every reuse of that token (any fingerprint) must take the terminal ConflictException with the already-exists message naming the stored ARN, never the in-flight wording and never a 200 replay.
Traces: bcm-dashboards.create-scheduled-report-token-reuse-conflict-rejected

## Evidence
Suite: simparity-behavioral-aws-v1 — bcm-dashboards 23/26 passed (88%), 3 failed.

[mismatch] bcm-dashboards.create-scheduled-report-token-reuse-conflict-rejected
  Flow: create a dashboard + an execution role, CreateScheduledReport with clientToken T (succeeds, ARN A), then CreateScheduledReport with the SAME token T and a different scheduledReport.name.
  Expected: ConflictException / HTTP 409 with message "A scheduled report with the provided clientToken already exists (ARN: A). Use a different clientToken to create a new scheduled report, or use the existing report's ARN to update it." — fixed text as quoted, variable part = A, the ARN stored under T.
  Actual: ConflictException / HTTP 409 (code and status matched) with message "A scheduled report creation is already in progress for the provided clientToken. Please retry after some time."
  Assertion: check_bcm_dashboards_create_scheduled_report_token_reuse_conflict_rejected.py line 62 — AssertionError: A scheduled report creation is already in progress for the provided clientToken. Please retry after some time.
  Code paths: src/ops/create_scheduled_report.rs:290-308 (conflict branch + 200 replay branch); src/state.rs:650 (SCHEDULED_REPORT_CREATION_WINDOW = 500ms, explicitly "CHOSEN, not cited"); src/state.rs ~688-701 (conflict_message selects by in_progress_at).
  The check's later assertions (that reusing T with IDENTICAL parameters is ALSO ConflictException, and that no second report ever appears under T) were never reached, so they remain unverified and should be re-checked with this check once the wording is fixed.

## Steps to Reproduce
1. Boot the emulator and point a botocore bcm-dashboards client at it (an IAM emulator is also used to mint the execution role).
2. create_dashboard(name=<valid>, widgets=[one cost-and-usage widget]) -> dashboard ARN D.
3. iam.create_role(...) with a trust policy naming bcm-dashboards.amazonaws.com -> role ARN R.
4. create_scheduled_report(scheduledReport={name: N1, dashboardArn: D, scheduledReportExecutionRoleArn: R, scheduleConfig: {scheduleExpression: "cron(0 8 ? * MON *)", scheduleExpressionTimeZone: "UTC"}}, clientToken=T) -> arn A, HTTP 200.
5. Immediately call create_scheduled_report again with the same clientToken=T but scheduledReport.name = N2. Expect ConflictException/409 with "A scheduled report with the provided clientToken already exists (ARN: A). Use a different clientToken to create a new scheduled report, or use the existing report's ARN to update it."; observe the in-flight message "A scheduled report creation is already in progress for the provided clientToken. Please retry after some time."
6. Wait >500ms and repeat step 4 verbatim (same token, same parameters). Expect ConflictException/409 with the same already-exists message; observe HTTP 200 returning arn A.
7. list_scheduled_reports: N2 must be absent in all cases.

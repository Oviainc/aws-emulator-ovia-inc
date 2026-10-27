Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards.create-scheduled-report-idempotent-token-concurrent
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards.create-scheduled-report-idempotent-token-concurrent|run#15|AssertionError|assert all(error in allowed for error in errors), (
Issue-Commit: 01d2c61316e604fc562e841d03e580212f748c20

# CreateScheduledReport ConflictException "creation in progress" message diverges: it interpolates the clientToken and omits the retry sentence

> _Note: this issue was identified against the code base at commit `01d2c61316e604fc562e841d03e580212f748c20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduledReport

## Description
One net-new root cause in the bcm-dashboards emulator's CreateScheduledReport conflict path. The single-winner semantics that open issue 20261022T215638Z-bcm-dashboards-dashboardname-pattern-unenforced-constraint-not-found.md (Cause 4) demanded now hold — this replay saw exactly 1 success and 4 refusals, and the winner's ARN matched the modeled scheduled-report ARN pattern — so this issue is NOT that cause again; what still diverges is the wording of the refusal the losers get.

=== Cause 1 of 1: the ConflictException message for a duplicate clientToken racing an in-flight create is not AWS's message ===

AWS: Smithy (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json) — CreateScheduledReport.clientToken targets ClientToken, @idempotencyToken, header X-Amzn-Client-Token, @length 1..128, @pattern ^[\u0021-\u007E]+$; the operation models ConflictException at HTTP 409, documented as "The request could not be completed due to a conflict with the current state of the resource. For example, attempting to create a resource that already exists or is being created." The model does not carry the message text, so the message is recorded behavior only: when several CreateScheduledReport calls share one clientToken, the non-winning calls are refused with code ConflictException / HTTP 409 and one of two per-branch templates. While the winner is still in flight the message is the WHOLLY FIXED text, with no variable part at all:
  "A scheduled report creation is already in progress for the provided clientToken. Please retry after some time."
The clientToken is deliberately NOT echoed — it is a caller-chosen secret-ish idempotency key, and the recorded text refers to it only as "the provided clientToken". The sibling branch, for a duplicate presented after the winner completed, is "A scheduled report with the provided clientToken already exists (ARN: {arn}). Use a different clientToken to create a new scheduled report, or use the existing report's ARN to update it." — fixed text except {arn}, which is the winner's scheduled-report ARN taken from state. (That second branch was not reached in this replay because all four losers overlapped the winner; it is named here because the same formatter owns both strings. Note that whether that branch refuses or replays the ARN is the subject of open issue 20261024T030750Z-bcm-dashboards-createscheduledreport-wrong-behavior.md and is NOT re-litigated here — only the message text is this issue's concern.)

Emulator: all four racing losers are refused with the right code and status (ConflictException, 409) but the message is "A scheduled report creation is already in progress for client token <the caller's token>" — e.g. "...for client token sim2real-token-018e11cb6366". Two divergences from the fixed text: the phrase "the provided clientToken" is replaced by "client token " plus the request's clientToken value interpolated into the message, and the trailing sentence "Please retry after some time." is missing entirely (there is also no terminating period). The string is formatted in the handler's in-progress branch (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/create_scheduled_report.rs:258-261); the sibling already-exists/different-parameters strings are formatted at lines 262-271 of the same block.

Why: the status, error code and error message are all part of the contract. A client or test suite that matches the conflict message to decide whether to back off and retry (in-progress, transient) versus pick a new token (already exists, terminal) cannot distinguish the branches from the emulator's text, and the missing "Please retry after some time." removes the retry instruction AWS gives for the transient branch. Echoing the caller's clientToken into the error body also leaks an idempotency key into logs and error reporting that real AWS never puts there, so a client that redacts tokens by relying on AWS's wording leaks them when run against the emulator.

Fix: the conflict-message construction in the CreateScheduledReport handler's clientToken branch — emit the fixed in-progress text verbatim for the racing-duplicate branch, and the already-exists text with only the stored winner ARN substituted for the completed branch; no request value belongs in the in-progress message.

Traces: bcm-dashboards.create-scheduled-report-idempotent-token-concurrent

## Evidence
Suite: simparity-behavioral-ga-prod-v1 — bcm-dashboards 10/12 passed (83%), 2 failed.

[mismatch] bcm-dashboards.create-scheduled-report-idempotent-token-concurrent (check_bcm_dashboards_create_scheduled_report_idempotent_token_concurrent.py line 81):
  Five concurrent CreateScheduledReport calls sharing one clientToken: 1 success (ARN matched arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/[-a-z0-9]+) + 4 refusals, which is the correct single-winner outcome.
  Expected each non-winning call to be one of:
    ("ConflictException", "A scheduled report creation is already in progress for the provided clientToken. Please retry after some time.")
    ("ConflictException", "A scheduled report with the provided clientToken already exists (ARN: <winner arn>). Use a different clientToken to create a new scheduled report, or use the existing report's ARN to update it.")
  Actual (all four losers):
    ("ConflictException", "A scheduled report creation is already in progress for client token sim2real-token-018e11cb6366")
  AssertionError: expected every non-winning call to get ConflictException with an in-progress or already-exists message, got [('ConflictException', 'A scheduled report creation is already in progress for client token sim2real-token-018e11cb6366'), ... x4]
  Fixed text vs values: the whole expected in-progress message is fixed text — there is NO variable part. The token 'sim2real-token-018e11cb6366' in the emulator's message is a per-run request value and must not appear at all; in the already-exists template only '{arn}' is variable (the winner's ARN from state).
  Source of the actual string: src/ops/create_scheduled_report.rs:258-261 (format!("A scheduled report creation is already in progress for client token {token}")).

## Steps to Reproduce
1. Boot the emulator and point a botocore bcm-dashboards client at it.
2. create_dashboard(name=<valid name>, widgets=[one cost-and-usage widget]) to get a dashboard ARN; create an IAM role usable as scheduledReportExecutionRoleArn.
3. Issue 5 parallel create_scheduled_report calls (released from a common barrier) with identical scheduledReport parameters and ONE shared clientToken.
4. Observe 1 success and 4 ConflictException/409 refusals (correct), then read the refusals' Error.Message.
5. Expected, for every loser that overlapped the winner: exactly "A scheduled report creation is already in progress for the provided clientToken. Please retry after some time." Observed: "A scheduled report creation is already in progress for client token <clientToken>" — token interpolated, retry sentence absent.
6. For the post-completion branch, repeat create_scheduled_report with the same clientToken after the winner has settled (past the creation window) and inspect that message against "A scheduled report with the provided clientToken already exists (ARN: <winner arn>). Use a different clientToken to create a new scheduled report, or use the existing report's ARN to update it."

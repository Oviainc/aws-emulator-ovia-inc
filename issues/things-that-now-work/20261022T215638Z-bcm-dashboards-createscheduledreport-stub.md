Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards.create-scheduled-report-token-reuse-conflict-rejected bcm-dashboards.delete-scheduled-report-already-deleted-not-found bcm-dashboards.execute-scheduled-report-deleted-dashboard-unhealthy bcm-dashboards.execute-scheduled-report-dry-run-no-execution-triggered bcm-dashboards.execute-scheduled-report-health-status-populated-after-dry-run bcm-dashboards.execute-scheduled-report-role-not-assumable-unhealthy bcm-dashboards.scheduled-report-default-schedule-state-enabled bcm-dashboards.update-scheduled-report-clear-widget-ids-resets-empty bcm-dashboards.update-scheduled-report-toggle-disabled-enabled
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards|CreateScheduledReport|NotImplemented
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Implement bcm-dashboards CreateScheduledReport: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduledReport

## Description
`CreateScheduledReport` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/create_scheduled_report.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `CreateScheduledReport` call, so each one unblocks only when `CreateScheduledReport` works: `bcm-dashboards.create-scheduled-report-token-reuse-conflict-rejected`, `bcm-dashboards.delete-scheduled-report-already-deleted-not-found`, `bcm-dashboards.execute-scheduled-report-deleted-dashboard-unhealthy`, `bcm-dashboards.execute-scheduled-report-dry-run-no-execution-triggered`, `bcm-dashboards.execute-scheduled-report-health-status-populated-after-dry-run`, `bcm-dashboards.execute-scheduled-report-role-not-assumable-unhealthy`, `bcm-dashboards.scheduled-report-default-schedule-state-enabled`, `bcm-dashboards.update-scheduled-report-clear-widget-ids-resets-empty`, `bcm-dashboards.update-scheduled-report-toggle-disabled-enabled`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/create_scheduled_report.rs`. Replaying the corpus, the emulator refused the `CreateScheduledReport` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateScheduledReport operation: operation CreateScheduledReport is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bcm-dashboards emulator
Call CreateScheduledReport and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

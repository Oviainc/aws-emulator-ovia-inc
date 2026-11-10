Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards.execute-scheduled-report-role-not-assumable-unhealthy
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards.execute-scheduled-report-role-not-assumable-unhealthy|run#14|AssertionError|assert health is not None, "health status never became UNHEALTHY after breaking role trust"
Issue-Commit: e8c5c92d9bc397e4014b473476bd2d32121bdaea

# ExecuteScheduledReport health never reports EXECUTION_ROLE_ASSUME_FAILED: breaking the execution role's trust policy leaves the report HEALTHY

> _Note: this issue was identified against the code base at commit `e8c5c92d9bc397e4014b473476bd2d32121bdaea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteScheduledReport

## Description
ONE root cause (Cause 1 of 1): the health a scheduled report reports is derived from the dashboard store and the EventBridge delivery only; the scheduledReportExecutionRoleArn's assumability is never evaluated, so the role-related StatusReason values are unreachable.

=== Cause 1 of 1: health derivation ignores whether the execution role can still be assumed ===
AWS: Smithy (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json) — HealthStatus.statusCode (@required, HealthStatusCode) is documented "HEALTHY indicates the scheduled report is configured properly and has all required permissions to execute"; StatusReason enumerates DATA_SOURCE_ACCESS_DENIED, EXECUTION_ROLE_ASSUME_FAILED, EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS, DASHBOARD_NOT_FOUND, DASHBOARD_ACCESS_DENIED, INTERNAL_FAILURE, WIDGET_ID_NOT_FOUND, and HealthStatus.statusReasons is "Only present when the status is UNHEALTHY". ScheduledReportInput.scheduledReportExecutionRoleArn is documented as the role the service "will assume ... while executing the scheduled report". Recorded behavior: a report created while its role WAS assumable, whose role trust policy is then changed to no longer name the bcm-dashboards service principal, is reported by ExecuteScheduledReport (dryRun=true) as healthStatus.statusCode = UNHEALTHY with EXECUTION_ROLE_ASSUME_FAILED in statusReasons — a health verdict, not a hard API failure (the call still succeeds with HTTP 200) — and returns to HEALTHY once the trust policy names that principal again.
Emulator: the dry-run call keeps answering HTTP 200 with healthStatus.statusCode = HEALTHY after the trust policy is broken, for the full 45s the check polls; EXECUTION_ROLE_ASSUME_FAILED is never produced. In this crate the record's execution_role_arn (src/state.rs:542) is only stored, projected back out (src/state.rs:590) and folded into the create fingerprint (src/state.rs:752) — no code path evaluates it against IAM. Health is resolved by reported_scheduled_report_health from the dashboard store (dashboard existence, widget ids) and the EventBridge notification delivery alone (src/state.rs, undelivered_notification_health / deliver_execution_notification ~2313+), so of the seven modelled StatusReason values only the two dashboard-side ones can ever be reported.
Why: HEALTHY is the signal a client acts on — it means the report will actually run. A client (or a console/automation flow) built against this emulator cannot develop or test the "your execution role is no longer assumable, fix it" path at all, and reads a healthy, triggerable report for a configuration real AWS reports as broken and will not execute. The divergence is also directional: the emulator is permissive, so misconfiguration is silently hidden.
Relation to open issues: 20261024T164554Z-bcm-dashboards-executescheduledreport-wrong-behavior.md (open, "Health derivation ignores scheduledReportExecutionRoleArn...") describes the same component from static review of a role ARN that names no IAM role; this behavioral check grounds it in the recorded sequence where the role EXISTS and was assumable at create time and its trust policy is broken afterwards, and it is still failing, so it is filed with the failing check id attached for replay.
Fix: the health derivation in this service's state (reported_scheduled_report_health / the ExecuteScheduledReport handler that calls it) must evaluate the stored execution_role_arn against the IAM store on every health resolution — role missing or trust policy not admitting the bcm-dashboards service principal => UNHEALTHY with statusReasons [EXECUTION_ROLE_ASSUME_FAILED] and executionTriggered false, with that health also visible to GetScheduledReport/ListScheduledReports, and the verdict re-derived (not cached) so restoring the trust policy restores HEALTHY. The in-tree facility service_common::iam_trust_policy::admits_service_principal is what other services use for this judgement; this crate depends on neither it nor the IAM store.
Traces: bcm-dashboards.execute-scheduled-report-role-not-assumable-unhealthy

## Evidence
Suite: simparity-behavioral-aws-v1 — bcm-dashboards 23/26 passed (88%), 3 failed.

[mismatch] bcm-dashboards.execute-scheduled-report-role-not-assumable-unhealthy
  Flow: create dashboard; create an IAM role trusting bcm-dashboards.amazonaws.com; CreateScheduledReport on it (succeeds); ExecuteScheduledReport(dryRun=True) -> HEALTHY (positive control, passed); iam.update_assume_role_policy to trust ONLY ec2.amazonaws.com; poll ExecuteScheduledReport(dryRun=True) for 45s.
  Expected: healthStatus.statusCode == "UNHEALTHY" with "EXECUTION_ROLE_ASSUME_FAILED" in healthStatus.statusReasons (HTTP 200 throughout), then HEALTHY again after the trust policy is restored.
  Actual: healthStatus.statusCode stays "HEALTHY" for the whole poll window — AssertionError at check_bcm_dashboards_execute_scheduled_report_role_not_assumable_unhealthy.py line 66: "health status never became UNHEALTHY after breaking role trust".
  Code paths: src/state.rs:542 / :590 / :752 are the only uses of execution_role_arn (store, project, fingerprint); health is resolved from the dashboard store and the EventBridge delivery outcome only (src/state.rs reported_scheduled_report_health, undelivered_notification_health, deliver_execution_notification ~2313+). No IAM lookup exists in this crate.
  The restore-to-HEALTHY assertion after the trust policy is repaired was never reached and remains unverified; re-replay this check to confirm both directions.

## Steps to Reproduce
1. Boot the emulator with the bcm-dashboards and IAM services on one engine.
2. iam.create_role(RoleName=R, AssumeRolePolicyDocument={Effect Allow, Principal {Service: bcm-dashboards.amazonaws.com}, Action sts:AssumeRole}) -> role ARN RA; attach any policy.
3. create_dashboard(name=<valid>, widgets=[one cost-and-usage widget]) -> dashboard ARN D.
4. create_scheduled_report(scheduledReport={name: N, dashboardArn: D, scheduledReportExecutionRoleArn: RA, scheduleConfig: {scheduleExpression: "cron(0 8 ? * MON *)", scheduleExpressionTimeZone: "UTC"}}) -> report ARN A.
5. execute_scheduled_report(arn=A, dryRun=True) -> healthStatus.statusCode == HEALTHY (correct today).
6. iam.update_assume_role_policy(RoleName=R, PolicyDocument={Principal {Service: ec2.amazonaws.com}}) — the role no longer trusts bcm-dashboards.
7. execute_scheduled_report(arn=A, dryRun=True) again (poll for up to 45s). Expect HTTP 200 with healthStatus UNHEALTHY and statusReasons containing EXECUTION_ROLE_ASSUME_FAILED; observe HEALTHY with no statusReasons. Also check get_scheduled_report(arn=A)/list_scheduled_reports report the same UNHEALTHY health, and that a non-dryRun execute returns executionTriggered=false.
8. Restore the trust policy to name bcm-dashboards.amazonaws.com and repeat — the identical dry-run call must report HEALTHY again.

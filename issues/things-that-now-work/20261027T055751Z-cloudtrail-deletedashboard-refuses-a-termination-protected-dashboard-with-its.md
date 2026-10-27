Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.concurrent-dashboard-update-no-lost-writes-corruption
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|DeleteDashboard|ValidationException
Issue-Commit: 1aab6ad9a4dd8a2817d61de8022dab6fc1da2f9a

# DeleteDashboard refuses a termination-protected dashboard with its own invented message instead of AWS's fixed sentence

> _Note: this issue was identified against the code base at commit `1aab6ad9a4dd8a2817d61de8022dab6fc1da2f9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDashboard

## Description
One net-new root cause.

Cause 1 of 1: the message CloudTrail Lake's DeleteDashboard returns when the addressed dashboard has termination protection enabled is wrong. Failing check: cloudtrail.concurrent-dashboard-update-no-lost-writes-corruption.

AWS: DeleteDashboard is documented as "Deletes the specified dashboard. You cannot delete a dashboard that has termination protection enabled." (Smithy: DeleteDashboard, input DashboardId -> DashboardArn, pattern ^[a-zA-Z0-9._/\-:]+$, required; modeled errors are only ConflictException/409, ResourceNotFoundException/400, UnsupportedOperationException/400 — this refusal is not modeled, so it is recorded behavior only). The recorded refusal is HTTP 400 with error code ValidationException and the message, in full, `This dashboard has termination protection enabled.` That message is ALL fixed text: it has no variable part — no dashboard name, ARN, account or Region is interpolated into it.

Emulator: the code and status are right (ValidationException), but the message is a different, self-authored sentence that interpolates the dashboard ARN: `The dashboard arn:aws:cloudtrail:<region>:<account>:dashboard/<name> cannot be deleted while termination protection is enabled for it: disable termination protection and try again.` (the arn/region/account/name shown here are this run's values filling a template the emulator invented; the whole sentence, template and all, is the divergence). The replay asserted the error Message and got that text instead, so the check fails after the concurrent UpdateDashboard phase has already succeeded. services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/delete_dashboard.rs:117-122 builds it, and its own doc comment states "AWS publishes no wording for this refusal" — the recording shows AWS does have fixed wording.

Why: a client that matches on the refusal text (and the AWS console/CLI messages users see, plus any retry/branching logic keyed to "This dashboard has termination protection enabled.") does not recognize the refusal, and a test asserting the documented message fails against the emulator while passing against AWS. The message also leaks a per-request ARN into text AWS keeps resource-free, so emulator output is not byte-comparable to AWS output.

Fix: the DeleteDashboard handler's refusal constructor (the `termination_protected` helper that feeds the ValidationException channel) must emit the fixed sentence `This dashboard has termination protection enabled.` with no interpolated values, keeping HTTP 400 and code ValidationException unchanged; the rest of the handler (not-found resolution, state release) is correct and the protected dashboard must still be left untouched.

No open issue covers this: the nearest CloudTrail issues (20261024T114720Z-cloudtrail-control-plane-fidelity-gettrailstatus-omits-its-deprecated.md and 20261023T135552Z-cloudtrail-createtrail-updatetrail-skip-every-referenced-resource-and.md) concern GetTrailStatus/ListTags/Lake queries and trail resource validation, not dashboard termination protection.

Traces: cloudtrail.concurrent-dashboard-update-no-lost-writes-corruption.

## Evidence
Check cloudtrail.concurrent-dashboard-update-no-lost-writes-corruption, verdict mismatch.

Expected (AWS, recorded): HTTP 400, Code=ValidationException, Message="This dashboard has termination protection enabled." (entirely fixed text).

Actual (emulator): HTTP 400, Code=ValidationException, Message="The dashboard arn:aws:cloudtrail:us-west-2:000000000000:dashboard/sim2real-dash-race-3a7f01fd6f83 cannot be deleted while termination protection is enabled for it: disable termination protection and try again."

Assertion from the replay:
  assert e.response["Error"].get("Message") == ("This dashboard has termination protection enabled.")
  AssertionError: {'Message': 'The dashboard arn:aws:cloudtrail:us-west-2:000000000000:dashboard/sim2real-dash-race-3a7f01fd6f83 cannot be deleted while termination protection is enabled for it: disable termination protection and try again.', 'Code': 'ValidationException'}

Source of the divergence: services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/delete_dashboard.rs:117-122 (`fn termination_protected`), reached from handle() at line 221-222 when record.termination_protection_enabled is true.

## Steps to Reproduce
1. ct.create_dashboard(Name=<name>, TerminationProtectionEnabled=False) and keep the returned DashboardArn.
2. ct.update_dashboard(DashboardId=<arn>, TerminationProtectionEnabled=True) (the check races four of these; one accepted update is enough).
3. Poll ct.get_dashboard(DashboardId=<arn>) until Status==UPDATED and TerminationProtectionEnabled is True.
4. Call ct.delete_dashboard(DashboardId=<arn>) and inspect the ClientError: Code is ValidationException as AWS returns, but Message is the emulator's ARN-interpolated sentence instead of the fixed "This dashboard has termination protection enabled."

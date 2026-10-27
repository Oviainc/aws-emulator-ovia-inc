Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.dashboard-create-update-delete-lifecycle
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.dashboard-create-update-delete-lifecycle|run#12|AssertionError|assert updated["Status"] == "UPDATED", updated
Issue-Commit: d20f3863f60340619cdbd48f5982ebeef462a933

# UpdateDashboard never advances DashboardStatus out of CREATED, so GetDashboard never reports UPDATING/UPDATED (1 cause)

> _Note: this issue was identified against the code base at commit `d20f3863f60340619cdbd48f5982ebeef462a933`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDashboard

## Description
One root cause.

Cause 1 of 1: a successful UpdateDashboard mutates the dashboard but leaves its DashboardStatus at the value the create path settled, so the dashboard's status never leaves CREATED and the UPDATING/UPDATED half of the status machine is unreachable. Failing check: cloudtrail.dashboard-create-update-delete-lifecycle.

AWS: Smithy (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/smithy-model/com-amazonaws-cloudtrail-cloudtrail-20131101.json) models GetDashboardResponse.Status as the enum DashboardStatus with exactly the values CREATING | CREATED | UPDATING | UPDATED | DELETING. Two of those five values — UPDATING and UPDATED — exist only for the update path: CREATING/CREATED belong to CreateDashboard and DELETING to DeleteDashboard, so the modelled enum itself states that a mutation moves a dashboard into UPDATING and settles it in UPDATED. Recorded behavior confirms it: after an UpdateDashboard that changes a member of the dashboard, a GetDashboard on that dashboard reports Status=UPDATED (the transient UPDATING may or may not be observed, depending on when the read lands, but the settled value is UPDATED, never CREATED). UpdateDashboard is modelled @idempotent and returns HTTP 200; no error is involved here — the divergence is entirely in the state the successful call leaves behind and in what GetDashboard reports for it.

Emulator: the UpdateDashboard handler applies the requested changes and stamps UpdatedTimestamp, but deliberately does not touch the stored status — its own module doc says so ("The dashboard's DashboardStatus is left as it stands ... this handler does not invent one — it stamps UpdatedTimestamp and leaves the status the create path settled"). The replay shows the consequence: after CreateDashboard the dashboard reads Status=CREATED with TerminationProtectionEnabled=true; after an UpdateDashboard(TerminationProtectionEnabled=false) that is accepted with HTTP 200, the subsequent GetDashboard returns TerminationProtectionEnabled=false (correctly applied) and a bumped UpdatedTimestamp strictly greater than CreatedTimestamp (correctly stamped) — but Status is still 'CREATED'. The check polls GetDashboard for 60 s and the status never advances, so DashboardStatus can only ever hold CREATED (and whatever CREATING/DELETING the create and delete paths set); UPDATING and UPDATED are values the emulator can never produce for any dashboard.

Why: a client that drives CloudTrail Lake dashboards reads Status to learn whether its edit has been taken up — that is the only field in GetDashboardResponse that reports the dashboard's lifecycle state, and the modelled enum makes UPDATED the terminal state of an edit. Against the emulator, any such wait-for-UPDATED loop (console flows, CloudFormation-style convergence logic, CI that edits widgets then blocks until the dashboard settles) never observes its terminal value and either spins until its own timeout or concludes the update was never applied — even though the widgets/refresh schedule/termination-protection change did in fact land. Conversely, code that distinguishes a never-edited dashboard from an edited one by Status==CREATED vs UPDATED misclassifies every edited dashboard as pristine. The emulator is self-consistently wrong: the write succeeds, the data changes, only the advertised lifecycle state lies.

Fix: the UpdateDashboard handler's state mutation, and the shared dashboard record's status transition in crate::state that the resolve/settle path drives. The update path must move the record's status into UPDATING and settle it to UPDATED on the same clock-driven transition the create path already uses to go CREATING -> CREATED, so GetDashboard, ListDashboards and the UpdateDashboard response all report the settled value. This is distinct from the already-fixed issue 20261023T135552Z-cloudtrail-dashboard-termination-protection-is-dropped-and-putresourcepolicy.md: termination protection is now stored and round-trips correctly (the replay proves it flips true -> false), and that issue says nothing about DashboardStatus — this is the remaining, separate status-machine gap, not a regression of that fix.

Traces: cloudtrail.dashboard-create-update-delete-lifecycle — asserts, after a successful UpdateDashboard, that GetDashboard reports Status == 'UPDATED'; it fails with AssertionError on a GetDashboard payload carrying Status='CREATED' alongside the correctly-updated TerminationProtectionEnabled=false and an UpdatedTimestamp later than CreatedTimestamp. Handler: services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/update_dashboard.rs — module doc lines 23-26 state the omission explicitly, and the mutation block at lines 319-335 stamps record.updated_at without any status transition.

## Evidence
Expected (AWS): after UpdateDashboard succeeds (HTTP 200), GetDashboard(DashboardId=<arn>) reports Status=UPDATED (DashboardStatus enum: CREATING|CREATED|UPDATING|UPDATED|DELETING; UPDATING/UPDATED are the update path's states).

Actual (emulator): GetDashboard reports Status='CREATED' indefinitely after a successful UpdateDashboard. Replayed GetDashboard response body:
{'DashboardArn': 'arn:aws:cloudtrail:<region>:<account>:dashboard/<name>', 'Type': 'CUSTOM', 'Status': 'CREATED', 'Widgets': [], 'CreatedTimestamp': <t0>, 'UpdatedTimestamp': <t0+2s>, 'TerminationProtectionEnabled': False, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Note the two fields that ARE right — TerminationProtectionEnabled=False (the value the update sent) and UpdatedTimestamp > CreatedTimestamp — which isolate the defect to the status field alone. The ARN, region, account, dashboard name and timestamps above are per-run values from one replay and are not part of the requirement; the requirement is Status=UPDATED.

Failure: AssertionError at check_cloudtrail_dashboard_create_update_delete_lifecycle.py line 106, `assert updated["Status"] == "UPDATED"`, after a 60 s poll in which the status never advanced.

Source evidence: services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/update_dashboard.rs lines 23-26 — "The dashboard's DashboardStatus is left as it stands ... this handler does not invent one"; lines 332-335 apply termination protection and `record.updated_at = now;` with no status write.

## Steps to Reproduce
1. CreateDashboard(Name=<fresh-name>, TagsList=[...]) and capture DashboardArn.
2. Poll GetDashboard(DashboardId=<arn>) until Status=CREATED. Observe TerminationProtectionEnabled=true, Widgets=[], UpdatedTimestamp == CreatedTimestamp.
3. Call UpdateDashboard(DashboardId=<arn>, TerminationProtectionEnabled=false). It returns HTTP 200.
4. Poll GetDashboard(DashboardId=<arn>).
   - AWS: TerminationProtectionEnabled=false, UpdatedTimestamp advanced, and Status transitions to UPDATED (possibly via UPDATING).
   - Emulator: TerminationProtectionEnabled=false and UpdatedTimestamp advanced, but Status stays 'CREATED' for as long as it is polled.
5. Any other mutating UpdateDashboard (e.g. replacing Widgets, or setting RefreshSchedule) reproduces the same: the data changes, Status does not.

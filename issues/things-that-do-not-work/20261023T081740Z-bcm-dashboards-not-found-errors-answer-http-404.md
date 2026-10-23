Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-dashboards-dashboard-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-dashboards-dashboard-error-paths bcm-dashboards-dashboard-lifecycle bcm-dashboards-dashboard-tagging bcm-dashboards-scheduled-report-errors-and-pagination
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards|GetDashboard|4xx->4xx|$.message
Issue-Key: bcm-dashboards|GetDashboard|4xx->4xx|$.resourceId
Issue-Key: bcm-dashboards|GetDashboard|4xx->4xx|$headers.content-type
Issue-Key: bcm-dashboards|GetDashboard|4xx->4xx|status
Issue-Key: bcm-dashboards|UpdateDashboard|4xx->4xx|$.message
Issue-Key: bcm-dashboards|UpdateDashboard|4xx->4xx|$headers.content-type
Issue-Key: bcm-dashboards|UpdateDashboard|4xx->4xx|status
Issue-Key: bcm-dashboards|ListTagsForResource|4xx->4xx|$.message
Issue-Key: bcm-dashboards|ListTagsForResource|4xx->4xx|status
Issue-Key: bcm-dashboards|GetScheduledReport|4xx->4xx|$.message
Issue-Key: bcm-dashboards|GetScheduledReport|4xx->4xx|$headers.content-type
Issue-Key: bcm-dashboards|GetScheduledReport|4xx->4xx|status
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# bcm-dashboards not-found errors answer HTTP 404 instead of 400 and omit resourceId

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboard

## Description
Replay of the simparity-wire-aws-v1 wire corpus against the bcm-dashboards emulator shows that every resource-not-found path answers with a different HTTP status than the recorded service does, and the GetDashboard variant also drops a member the recorded body carries. FOUR causes (one per operation whose not-found path was recorded); all four must be closed before this issue is submitted. The error CODE (__type) matched in every case - only the status line, and for GetDashboard the body shape, diverge.

=== Cause 1 of 4: GetDashboard not-found answers 404 and omits the resourceId member ===
AWS: recorded behavior only for the status. Smithy (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json): GetDashboard models ResourceNotFoundException with @httpError 404 and a single @required `message` member, but the RECORDING of a GetDashboard on a well-formed DashboardArn that no dashboard is stored under is HTTP 400 carrying the not-found error code plus a `resourceId` member holding the requested ARN. The recording is the wire record, so the requirement is: status 400, the same error code, and a `resourceId` member echoing the request ARN. The message is request-derived ("Resource <arn> does not exist." as recorded) - that template is already owned by the open behavioral issue "bcm-dashboards: DashboardName pattern unenforced, constraint/not-found error messages diverge, ...", which does NOT carry this wire step, so the status and resourceId requirements recorded here are net-new.
Emulator: HTTP 404, no `resourceId` member at all (expected 'arn:aws:bcm-dashboards::000000000000:dashboard/9cf42a2aabae420780b3', got None), message "No dashboard found for arn <arn>".
Why: SDKs and HTTP middleware branch on the status line before they ever parse the body - a 404 is commonly treated as a routing/endpoint failure (and some proxies and retry policies handle 4xx subclasses differently) rather than a modeled client error, so a caller written against the recorded service takes the wrong branch. A client reading `resourceId` to report which ARN was missing reads nothing.
Fix: the not-found error construction in the GetDashboard handler and the error serializer that maps this service's error shapes to a status line.
Traces: bcm-dashboards-dashboard-error-paths#task#1 (also affected: bcm-dashboards-dashboard-lifecycle#teardown#6)

=== Cause 2 of 4: UpdateDashboard not-found answers 404 instead of 400 ===
AWS: recorded behavior only - the recorded UpdateDashboard against an ARN no dashboard is stored under is HTTP 400 with the not-found error code (Smithy models ResourceNotFoundException at @httpError 404 for this operation; the recording overrides that).
Emulator: HTTP 404.
Why: same as cause 1 - an update against a deleted dashboard is reported with a status the client does not expect for a modeled client error.
Fix: the not-found error construction in the UpdateDashboard handler plus the shared error-to-status mapping.
Traces: bcm-dashboards-dashboard-error-paths#task#2

=== Cause 3 of 4: ListTagsForResource not-found answers 404 instead of 400 ===
AWS: recorded behavior only - the recorded ListTagsForResource on a ResourceArn that resolves to nothing is HTTP 400 with the not-found error code (Smithy: ResourceNotFoundException, @httpError 404).
Emulator: HTTP 404.
Why: same as cause 1, on the tagging read path.
Fix: the not-found error construction in the ListTagsForResource handler plus the shared error-to-status mapping.
Traces: bcm-dashboards-dashboard-error-paths#task#4 (also affected: bcm-dashboards-dashboard-tagging#teardown#8)

=== Cause 4 of 4: GetScheduledReport not-found answers 404 instead of 400 ===
AWS: recorded behavior only - the recorded GetScheduledReport on a well-formed ScheduledReportArn that no report is stored under is HTTP 400 with the not-found error code (Smithy: ResourceNotFoundException, @httpError 404).
Emulator: HTTP 404.
Why: same as cause 1, on the scheduled-report read path.
Fix: the not-found error construction in the GetScheduledReport handler plus the shared error-to-status mapping.
Traces: bcm-dashboards-scheduled-report-errors-and-pagination#task#6

Note on messages: the recorded messages differ per operation ("Resource <arn> does not exist.", "Dashboard not found: <arn>", "Resource not found", "Scheduled report not found: <arn>") and the emulator emits its own wording. Those are quoted below as examples only; the requirements this issue gates are the status, the error code and the resourceId member.

## Evidence
Corpus simparity-wire-aws-v1, 50 steps, 17 passed (34%), 14 divergence clusters - all bcm-dashboards. emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

Cause 1 - bcm-dashboards-dashboard-error-paths#task#1 (GetDashboard) [mismatch], cluster of 2 (also bcm-dashboards-dashboard-lifecycle#teardown#6):
  status: expected 400, got 404
  $.resourceId: missing (expected 'arn:aws:bcm-dashboards::000000000000:dashboard/9cf42a2aabae420780b3', got None)
  $.message (example only): expected 'Resource arn:aws:bcm-dashboards::000000000000:dashboard/9cf42a2aabae420780b3 does not exist.', got 'No dashboard found for arn arn:aws:bcm-dashboards::000000000000:dashboard/9cf42a2aabae420780b3'

Cause 2 - bcm-dashboards-dashboard-error-paths#task#2 (UpdateDashboard) [mismatch]:
  status: expected 400, got 404
  $.message (example only): expected 'Dashboard not found: <arn>', got 'No dashboard found for arn <arn>'

Cause 3 - bcm-dashboards-dashboard-error-paths#task#4 (ListTagsForResource) [mismatch], cluster of 2 (also bcm-dashboards-dashboard-tagging#teardown#8):
  status: expected 400, got 404
  $.message (example only): expected 'Resource not found', got 'No resource found for arn <arn>'

Cause 4 - bcm-dashboards-scheduled-report-errors-and-pagination#task#6 (GetScheduledReport) [mismatch]:
  status: expected 400, got 404
  $.message (example only): expected 'Scheduled report not found: <arn>', got 'Resource <arn> does not exist.'

In all four the error code (__type) matched - it is not reported as a diverging key.

## Steps to Reproduce
Boot the emulator and point a sigv4 client at the bcm-dashboards endpoint (protocol awsJson1_0, target prefix per the model).

Cause 1: GetDashboard with arn=arn:aws:bcm-dashboards::000000000000:dashboard/<20-hex id never created> (well-formed per the DashboardArn pattern). Expect HTTP 400 with the not-found code and a resourceId member equal to the request ARN; observe HTTP 404 with no resourceId. Equivalently: CreateDashboard -> DeleteDashboard -> GetDashboard on the same ARN (bcm-dashboards-dashboard-lifecycle#teardown#6).
Cause 2: UpdateDashboard with that same never-created ARN and a valid name/widgets. Expect HTTP 400; observe 404.
Cause 3: ListTagsForResource with resourceArn set to that never-created dashboard ARN. Expect HTTP 400; observe 404.
Cause 4: GetScheduledReport with arn=arn:aws:bcm-dashboards::000000000000:scheduled-report/<id never created>. Expect HTTP 400; observe 404.

Then replay the fixtures named in Traces to confirm.

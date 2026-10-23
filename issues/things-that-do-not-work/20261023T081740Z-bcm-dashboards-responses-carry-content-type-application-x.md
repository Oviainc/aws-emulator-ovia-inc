Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-dashboards-dashboard-lifecycle
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-dashboards-dashboard-lifecycle bcm-dashboards-dashboard-widget-variety bcm-dashboards-update-dashboard-replaces-widgets-completely bcm-dashboards-scheduled-report-errors-and-pagination
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$headers.content-type
Issue-Key: bcm-dashboards|UpdateDashboard|2xx->2xx|$headers.content-type
Issue-Key: bcm-dashboards|DeleteDashboard|2xx->2xx|$headers.content-type
Issue-Key: bcm-dashboards|ListScheduledReports|2xx->2xx|$headers.content-type
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# bcm-dashboards responses carry content-type application/x-amz-json-1.0 instead of the recorded 1.1

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboard

## Description
ONE cause, reproduced on four separate operations; the fixture bindings for all four are listed so a fixer re-replays the whole set.

=== Cause 1 of 1: the response content-type header does not match the recorded wire ===
AWS: recorded behavior only. Every recorded bcm-dashboards response in this corpus - successes and errors alike - carries `content-type: application/x-amz-json-1.1`. Note the tension a fixer must resolve deliberately: the on-disk Smithy model (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json) declares the protocol `awsJson1_0`, whose default response media type is application/x-amz-json-1.0, so the emulator's value is what the protocol trait implies. Hypothesis: the real service fronts an awsJson1_0 API with a dispatcher that stamps the 1.1 media type on responses, which is why the recording and the trait disagree. The recording is the wire record, so the recorded header is the target; the request-side protocol (target header, body encoding) is unchanged and matched throughout.
Emulator: every response carries `application/x-amz-json-1.0`. Observed on GetDashboard, UpdateDashboard, DeleteDashboard and ListScheduledReports as its own cluster, and alongside other defects on ListDashboards, GetResourcePolicy, GetDashboard-not-found, UpdateDashboard-not-found and GetScheduledReport-not-found.
Why: a client or recording proxy that selects its JSON-protocol decoder from the response media type, or a strict wire-conformance test pinned to the recorded transcript, sees a different protocol version than the service it is emulating announces. It does not change the decoded body, which is why this is LOW on its own - but it is the single highest-volume divergence in the corpus (10 of 14 clusters touch it) and it is a per-response header, so fixing it once clears it everywhere.
Fix: the response serializer / content-type stamping for this service (the one place the awsJson media type is emitted), not any individual operation handler - the header must be identical on success and on error responses.
Traces: bcm-dashboards-dashboard-lifecycle#task#2 (GetDashboard; cluster also covers bcm-dashboards-dashboard-lifecycle#task#4, bcm-dashboards-dashboard-widget-variety#task#2, bcm-dashboards-update-dashboard-replaces-widgets-completely#task#2 and #task#4); bcm-dashboards-dashboard-lifecycle#task#3 (UpdateDashboard; also bcm-dashboards-update-dashboard-replaces-widgets-completely#task#3); bcm-dashboards-dashboard-lifecycle#teardown#5 (DeleteDashboard; also bcm-dashboards-dashboard-widget-variety#teardown#4); bcm-dashboards-scheduled-report-errors-and-pagination#task#8 (ListScheduledReports; also #task#9, #task#10).

## Evidence
Corpus simparity-wire-aws-v1, 50 steps, 17 passed (34%). emulator_up=true, state_reset_failures=0.

The only diverging key in these four clusters:
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0')

Cluster representatives and sizes:
  bcm-dashboards-dashboard-lifecycle#task#2 - GetDashboard, 200 -> 200, 5 affected steps
  bcm-dashboards-dashboard-lifecycle#task#3 - UpdateDashboard, 200 -> 200, 2 affected steps
  bcm-dashboards-dashboard-lifecycle#teardown#5 - DeleteDashboard, 200 -> 200, 2 affected steps
  bcm-dashboards-scheduled-report-errors-and-pagination#task#8 - ListScheduledReports, 200 -> 200, 3 affected steps

The same header mismatch also appears as a secondary key inside the ListDashboards, GetResourcePolicy and not-found clusters filed in the other two bcm-dashboards wire issues.

## Steps to Reproduce
Boot the emulator, point a sigv4 client at the bcm-dashboards endpoint, and inspect the RAW response headers (not the parsed body):
  1. CreateDashboard with a valid name and one widget, then GetDashboard on the returned ARN -> response content-type is application/x-amz-json-1.0, recorded is application/x-amz-json-1.1.
  2. UpdateDashboard on that ARN - same header.
  3. DeleteDashboard on that ARN - same header.
  4. ListScheduledReports - same header.
Then replay the fixtures named in Traces.

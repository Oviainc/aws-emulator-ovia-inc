Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-alarm-contributor-check
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-alarm-contributor-check cloudwatch-composite-alarm-contributors-and-log-alarm cloudwatch-composite-alarm-history-and-contributors cloudwatch-metric-alarm-history-and-contributors cloudwatch-metric-alarm-queries cloudwatch-dashboard-create-update-and-invalid-body cloudwatch-dashboard-crud-round-trip monitoring-dashboard-lifecycle monitoring-dashboard-put-get-delete-lifecycle monitoring-dashboard-tags-and-not-found-boundary cloudwatch-mstx-traj-rebuild-exclude-stream-refusals
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DescribeAlarmContributors|4xx->2xx|$.__type
Issue-Key: monitoring|DescribeAlarmContributors|4xx->2xx|$.message
Issue-Key: monitoring|DescribeAlarmContributors|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|DescribeAlarmContributors|4xx->2xx|status
Issue-Key: monitoring|ListDashboards|2xx->2xx|$.DashboardEntries[*]
Issue-Key: monitoring|ListDashboards|2xx->2xx|$.DashboardEntries[*].Size
Issue-Key: monitoring|PutMetricStream|4xx->4xx|$.__type
Issue-Key: monitoring|PutMetricStream|4xx->4xx|$headers.x-amzn-query-error
Issue-Related: 20261102T112036Z-monitoring-cloudwatch-validationexception-messages-drop-the-offending-value-and.md
Issue-Commit: 544316519d780db35004270636d68199c524c41e

# CloudWatch read and write fidelity: DescribeAlarmContributors skips its alarm-type refusal, ListDashboards reports the wrong Size, PutMetricStream raises the wrong exception (4 causes)

> _Note: this issue was identified against the code base at commit `544316519d780db35004270636d68199c524c41e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmContributors

## Description
Four net-new wire divergences on CloudWatch, each a different handler answering the right status-or-value contract incorrectly. None is attributed to an open issue by the replay report; the nearest open monitoring issues ('CloudWatch metric-read parameter refusals use the wrong exception type' and 'CloudWatch ValidationException messages drop the offending value and re-sort the enum set') cover GetMetricData/GetMetricStatistics/GetMetricStream and the alarm-read enum messages, and carry none of these replay keys.

Cause 1 of 4: DescribeAlarmContributors on a plain (single-time-series) metric alarm succeeds instead of being refused.
AWS: Smithy: DescribeAlarmContributors takes a required AlarmName and models InvalidNextToken (400) and ResourceNotFoundException (404); the alarm-type precondition is not modeled, so recorded behavior only — AWS refuses the call for an alarm that is not a multi-time-series alarm with HTTP 400, $.__type 'com.amazon.coral.validate#ValidationException' and header x-amzn-query-error 'ValidationError;Sender'. The message template is the fixed text 'Alarm ' + <AlarmName taken from the request> + ' is not a multi time series alarm'; the only variable part is the alarm name echoed from the request.
Emulator: answers HTTP 200 with a contributors body, emitting no __type, no message and no x-amzn-query-error header.
Why: a client uses this refusal to distinguish a multi-time-series alarm from a plain metric alarm. Getting 200 instead, it concludes the plain alarm has contributors, so contributor-driven diagnostics and dashboards attribute data to an alarm that has no contributor dimension at all, and the ValidationException branch is never exercised.
Fix: the DescribeAlarmContributors handler's input validation — gate on the alarm's type before projecting contributors, and serialize the coral ValidationException (400 + __type + query-error header + the message above).
Traces: cloudwatch-alarm-contributor-check#task#2 (also cloudwatch-composite-alarm-contributors-and-log-alarm#task#2, cloudwatch-composite-alarm-history-and-contributors#task#4, cloudwatch-metric-alarm-history-and-contributors#task#4, cloudwatch-metric-alarm-queries#task#4).

Cause 2 of 4: ListDashboards reports a DashboardEntry.Size that does not match the stored dashboard body.
AWS: Smithy: DashboardEntry.Size is a long documented as 'The size of the dashboard, in bytes' — i.e. the byte length of the dashboard body as stored by PutDashboard. The recording shows the Size of the dashboard body the fixture put.
Emulator: reports a smaller byte count for the same dashboard body, so Size is not the length of what was stored (it appears to be computed from a re-serialized/normalized form rather than the stored bytes).
Why: a client that uses Size to budget against the dashboard size quota, to detect that a dashboard body changed, or to decide whether to re-fetch, reads a value inconsistent with the body GetDashboard returns — size-based change detection never fires and quota accounting under-counts.
Fix: the ListDashboards projection / dashboard state — carry the byte length of the body as stored by PutDashboard. (The LastModified difference in this cluster is a per-run clock value and is NOT part of the finding.)
Traces: cloudwatch-dashboard-create-update-and-invalid-body#task#3 (also cloudwatch-dashboard-crud-round-trip#task#3, monitoring-dashboard-lifecycle#task#3).

Cause 3 of 4: the same Size defect on a second dashboard body, confirming it is systematic rather than one body's rounding.
AWS: as cause 2 — Size is the stored body's byte length.
Emulator: again reports a byte count below the stored body's length for a different dashboard.
Why: as cause 2; recorded on an independent fixture so a fixer cannot close it by special-casing one body.
Fix: same projection as cause 2.
Traces: monitoring-dashboard-put-get-delete-lifecycle#task#3 (also monitoring-dashboard-tags-and-not-found-boundary#task#2).

Cause 4 of 4: PutMetricStream with OutputFormat omitted raises MissingRequiredParameterException instead of the coral ValidationException AWS raises.
AWS: Smithy: PutMetricStream.OutputFormat is @required, targets the MetricStreamOutputFormat enum (values json, opentelemetry0.7, opentelemetry1.0), and the operation models MissingRequiredParameterException (awsQueryError MissingParameter/400) among its errors — but the recorded refusal for a null OutputFormat is the Coral input-validation path: HTTP 400, $.__type 'com.amazon.coral.validate#ValidationException', header x-amzn-query-error 'ValidationError;Sender', message "1 validation error detected: Value null at 'outputFormat' failed to satisfy constraint: Member must not be null". Fixed text of that template: '1 validation error detected: Value null at ' + "'<lowerCamel member path>'" + ' failed to satisfy constraint: Member must not be null'; the only variable part is the member path, taken from the request's missing member (here outputFormat).
Emulator: answers HTTP 400 with $.__type 'com.amazon.coral.service#MissingRequiredParameterException', header x-amzn-query-error 'MissingParameter;Sender' and the emulator-authored message 'The parameter OutputFormat is required.'
Why: the status matches but the error code does not, so a client's retry/branch logic keyed on ValidationError (or on the awsQueryError code ValidationError) does not match, and a parser that reads the '1 validation error detected: ... at '<member>'' template to point the user at the offending member gets prose it cannot parse and the lower-camel member path is lost.
Fix: PutMetricStream's input validation — route missing required members through the same Coral validation serializer the rest of the service's validation uses (ValidationException + ValidationError;Sender + the numbered 'validation error detected' message), not the MissingRequiredParameter path.
Traces: cloudwatch-mstx-traj-rebuild-exclude-stream-refusals#task#2.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (all four clusters report stub_path='' and owned_by=[], i.e. net-new).

Cause 1 — cloudwatch-alarm-contributor-check#task#2, DescribeAlarmContributors:
  status: expected 400, got 200
  $.__type: missing (expected 'com.amazon.coral.validate#ValidationException', got None)
  $.message: missing (expected 'Alarm <AlarmName> is not a multi time series alarm' — recorded instance: 'Alarm sim2real-probe-cloudwatch-plain-alarm-dd2b58dc is not a multi time series alarm'; the probe name is the request's alarm name, not part of the template — got None)
  $headers.x-amzn-query-error: missing (expected 'ValidationError;Sender', got None)
  affected_count 5

Cause 2 — cloudwatch-dashboard-create-update-and-invalid-body#task#3, ListDashboards, 200 vs 200:
  $.DashboardEntries[0]: expected {...'Size': 149}, got {...'Size': 125} for the same DashboardName/DashboardArn (LastModified also differs, but that is a per-run clock value and is excluded)
  affected_count 3

Cause 3 — monitoring-dashboard-put-get-delete-lifecycle#task#3, ListDashboards, 200 vs 200:
  $.DashboardEntries[0].Size: expected 165, got 141
  affected_count 2

Cause 4 — cloudwatch-mstx-traj-rebuild-exclude-stream-refusals#task#2, PutMetricStream, 400 vs 400:
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got 'com.amazon.coral.service#MissingRequiredParameterException'
  $.message: expected "1 validation error detected: Value null at 'outputFormat' failed to satisfy constraint: Member must not be null", got 'The parameter OutputFormat is required.'
  $headers.x-amzn-query-error: expected 'ValidationError;Sender', got 'MissingParameter;Sender'

Model citations: smithy_query operation=DescribeAlarmContributors,ListDashboards,PutMetricStream and shape=DashboardEntry,MetricStreamOutputFormat on services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture cloudwatch-alarm-contributor-check step task#2 — DescribeAlarmContributors against a plain metric alarm must answer HTTP 400 with __type 'com.amazon.coral.validate#ValidationException', header x-amzn-query-error 'ValidationError;Sender' and message 'Alarm <requested alarm name> is not a multi time series alarm'.
4. Cause 2: fixture cloudwatch-dashboard-create-update-and-invalid-body step task#3 — PutDashboard then ListDashboards; DashboardEntries[0].Size must equal the byte length of the stored dashboard body.
5. Cause 3: fixture monitoring-dashboard-put-get-delete-lifecycle step task#3 — same check on a second dashboard body.
6. Cause 4: fixture cloudwatch-mstx-traj-rebuild-exclude-stream-refusals step task#2 — PutMetricStream with OutputFormat omitted must answer the Coral ValidationException (400, __type 'com.amazon.coral.validate#ValidationException', x-amzn-query-error 'ValidationError;Sender', message "1 validation error detected: Value null at 'outputFormat' failed to satisfy constraint: Member must not be null").
7. Re-replay all four fixture#step bindings after the fix; every one must pass.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguru-security.get-metrics-summary-works-regardless-of-date-age
Issue-Shape-Id: com.amazonaws.codegurusecurity#AwsCodeGuruSecurity
Issue-Key: codeguru-security|GetMetricsSummary|ValidationException
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# GetMetricsSummary refuses a date older than 730 days with 400 ValidationException instead of returning a metricsSummary

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricsSummary

## Description
One root cause.

Cause 1 of 1: GetMetricsSummary applies a hard two-year recency window to the required `date` query parameter and refuses anything older, where the service accepts it and answers 200. Failing check: codeguru-security.get-metrics-summary-works-regardless-of-date-age

AWS:      Smithy: GetMetricsSummary is a readonly GET on /metrics/summary; input GetMetricsSummaryRequest.date is @required, targets Timestamp and is bound @httpQuery("date"). The member carries NO @range / no constraint trait of any kind, and the operation's modeled errors are AccessDeniedException(403), InternalServerException(500), ThrottlingException(429) and ValidationException(400) — none of them tied to date age. The member's own documentation says "The date you want to retrieve summary metrics from, rounded to the nearest day. The date must be within the past two years", but that prose is advisory: recorded behavior shows the service does not enforce it. In the recording, GetMetricsSummary with a date ~3 years in the past (as well as a date 1 day in the past) returns HTTP 200 with a well-formed metricsSummary containing `date` plus the list members categoriesWithMostFindings, scansWithMostOpenFindings and scansWithMostOpenCriticalFindings (and an openFindings severity breakdown). Notably, GetMetricsSummary is still fully functional on this surface while the scan/finding-management operations are unconditionally refused with FeatureNoLongerAvailableException — so this is the one operation that must keep answering.

Emulator: the call is refused before any summary is produced. Replay output: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetMetricsSummary operation: date must be within the past 730 days: <requested date> is not in <now-365d>..<now+365d>. So the emulator (a) turned the advisory documentation into an enforced 730-day window and (b) implemented that window as a symmetric +/- 365-day band around the current time rather than a backward-looking two-year window, meaning it also accepts dates up to a year in the FUTURE while rejecting dates in the recent past beyond one year. The status/code divergence is 400 ValidationException where AWS returns 200 with a metricsSummary body.

Why:     a client that asks for historical security metrics — any dashboard or report that backfills more than a year, or any caller that passes an archived date — gets a hard 400 instead of the summary AWS returns. Because ValidationException is a client error and is not retryable, SDK retry logic will not recover: the call fails permanently and the caller concludes its own date argument is invalid. Since every other metrics/finding entry point on this service is retired, this operation failing takes the whole remaining data-plane surface of codeguru-security with it. The future-date half of the same band is the mirror defect: requests AWS would validate are silently accepted.

Fix:     remove the recency gate from the GetMetricsSummary handler's input validation and let the handler serve a well-formed metricsSummary for any parseable timestamp, as the model (an unconstrained required Timestamp) specifies. The handler should keep rejecting only what the model makes invalid (a missing or unparseable `date`), echo the requested date (rounded to the nearest day) into metricsSummary.date, and emit the three list members as present-but-possibly-empty lists so the response shape is stable for old and recent dates alike.

Traces:  codeguru-security.get-metrics-summary-works-regardless-of-date-age (group data-plane) — fails on the first call, with `date` set to now minus 3 years; the same check's second call uses now minus 1 day and must also return a well-formed summary.

## Evidence
Expected (recorded AWS behavior, and the model): GetMetricsSummary(date=now-3years) -> HTTP 200 with a metricsSummary object containing `date` and list members categoriesWithMostFindings, scansWithMostOpenFindings, scansWithMostOpenCriticalFindings (plus optional openFindings). Same for date=now-1day. Smithy places no range constraint on GetMetricsSummaryRequest.date (@required Timestamp, @httpQuery("date")).

Actual (emulator): HTTP 400 ValidationException. Replay output:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetMetricsSummary operation: date must be within the past 730 days: 2023-10-26T00:00:00Z is not in 2024-10-25T00:00:00Z..2026-10-25T00:00:00Z
(The literal dates and the request-time-relative bounds are environment-specific examples; the requirement being violated is the 400/ValidationException where AWS answers 200, and the window being a symmetric +/-365d band around 'now' rather than no window at all. The message text itself is an example, not a requirement — the enforced refusal is the defect.)

## Steps to Reproduce
1. Boot the emulator and point a codeguru-security client at it.
2. Call GetMetricsSummary with date = (now, UTC) - 3 years, e.g. client.get_metrics_summary(date=datetime.now(timezone.utc) - timedelta(days=365*3)).
3. Observe HTTP 400 ValidationException ("date must be within the past 730 days: ...") instead of HTTP 200 with a metricsSummary body.
4. Repeat with date = now + 200 days to see the mirror half of the same band: the emulator accepts a future date it should not be special-casing at all.
5. Call GetMetricsSummary with date = now - 1 day and confirm that path does return a well-formed metricsSummary, isolating the failure to the recency gate.

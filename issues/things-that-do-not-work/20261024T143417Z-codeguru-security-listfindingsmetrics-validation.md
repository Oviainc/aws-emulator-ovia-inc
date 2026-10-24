Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# An inverted startDate/endDate interval is accepted with 200 and an empty list instead of being refused

_Filed: 2026-10-24T14:34:17Z_
_Operation: ListFindingsMetrics | Class: validation_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `startDate` and `endDate` are both `@required` and define "the interval which you want to retrieve metrics from". An interval whose end precedes its start is not an interval, and AWS date-range APIs refuse it with the operation's modeled ValidationException (400, "The input fails to satisfy the specified constraints") naming the offending member, rather than reporting a successful measurement over nothing.

Actual: the handler floors both bounds to UTC days and then simply walks `day = start_day; while day <= end_day`, so an inverted interval produces zero iterations and the call answers 200 with `findingsMetrics: []` and no `nextToken`. The handler's own comment states this choice explicitly ("An interval whose end day precedes its start day contains no day, so it reports no metrics"). A client that swapped its two dates — the classic caller mistake this validation exists to catch — is told the account has no metrics in that window instead of being told its request was malformed, and the empty answer is indistinguishable from a genuinely quiet (correctly ordered) interval. The same leniency applies to an interval entirely in the future.

## Evidence

Smithy model: `ListFindingsMetricsRequest$startDate` / `$endDate` are both `@required` `@httpQuery` timestamps documented as the start/end "of the interval which you want to retrieve metrics from", and the operation declares ValidationException (400); the sibling `GetMetricsSummaryRequest$date` shows the service does constrain the dates it accepts ("The date must be within the past two years"). Handler: services/service-com-amazonaws-codegurusecurity-awscodegurusecurity/src/ops/list_findings_metrics.rs lines 313-326 (both bounds floored, with the comment that an end-before-start interval "reports no metrics — the model declares no error for it") and lines 376-386 (the `while day <= end_day` loop that therefore yields nothing). No range relationship between the two members is checked anywhere in the handler. Test coverage: tests/all/findings_metrics.rs covers pagination and the per-member `maxResults` range refusal but never submits endDate < startDate, so the inverted interval is unasserted.

## Steps to Reproduce

1. Boot the emulator and point a codeguru-security client at it
2. GET /metrics/findings?startDate=<2026-01-10T00:00:00Z epoch>&endDate=<2026-01-05T00:00:00Z epoch> (end five days BEFORE start)
3. Observe HTTP 200 with {"findingsMetrics": []} instead of a 400 ValidationException identifying the inverted interval
4. Control: GET /metrics/findings?startDate=<2026-01-05>&endDate=<2026-01-10> -> 200 with six day entries, so the empty answer in step 3 is not distinguishable from a legitimately empty correctly-ordered window

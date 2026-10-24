Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# GetMetricsSummary accepts a date outside the documented two-year window (and dates in the future) instead of refusing it

_Filed: 2026-10-24T02:28:10Z_
_Operation: GetMetricsSummary | Class: validation_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: GetMetricsSummaryRequest$date is documented as "The date you want to retrieve summary metrics from, rounded to the nearest day. The date must be within the past two years." A date outside that window (e.g. 1970, or a date years in the future) violates a documented input constraint and is reported with the modelled ValidationException (HTTP 400).

Actual: the handler explicitly declines to range-check the date (src/ops/get_metrics_summary.rs lines 243-250: "The date is NOT range-checked") and answers 200 with an all-zero MetricsSummary whose `date` echoes whatever day the caller named — including future days, for which the counts are also computed as-of a day that has not happened. Invalid input is accepted silently, so a client bug (unit-vs-millisecond timestamps, an unset date) looks like a legitimately empty account.

## Evidence

Smithy model: GetMetricsSummaryRequest$date documentation states "The date must be within the past two years"; the operation declares ValidationException (HTTP 400) for input that fails the specified constraints. Handler: src/ops/get_metrics_summary.rs lines 237-250 (no bound check) and 310-319 (always 200). Tests ASSERT the permissive behavior: tests/all/metrics_summary.rs:437 metrics_summary_answers_any_date_age_with_the_day_the_date_falls_in.

## Steps to Reproduce

1. GET /metrics/summary?date=0 (1970-01-01)
2. Observe 200 with metricsSummary.date = 1970-01-01 and zero counts instead of 400 ValidationException
3. GET /metrics/summary?date=<epoch seconds five years in the future> -> also 200

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# A partial scheduleConfig update silently drops the stored scheduleExpressionTimeZone and schedulePeriod

_Filed: 2026-10-29T17:08:21Z_
_Operation: UpdateScheduledReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateScheduledReport` replaces the stored `scheduleConfig` structure WHOLESALE with the submitted one, carrying over exactly one nested property (`state`). Every other property of the stored schedule that the request does not name — `scheduleExpressionTimeZone` and `schedulePeriod` — is dropped, so the most ordinary partial update ("re-cron this report") silently loses stored configuration.

The crate reads the operation's documented partial-update rule at the NESTED property level: that is the stated justification for carrying `state` over. Applied consistently, the same reading carries over the time zone and the active period, neither of which the request models a clear flag for either. As written, the handler keeps `state` and discards the other two, which is the inconsistency: one nested property obeys the quoted rule and the rest do not.

## Evidence

Model (kind A), `smithy_query(query="doc",shape="UpdateScheduledReport")`: "Updates an existing scheduled report's properties, including its name, description, schedule configuration, and widget settings. **Only the parameters included in the request are updated; all other properties remain unchanged.**"

The crate quotes that very sentence and then applies it to ONE nested property only — src/state.rs:1519-1540:
```
/// An update applies only the properties its request names — "Only the parameters
/// included in the request are updated; all other properties remain unchanged"
/// (`smithy_query(query="doc",shape="UpdateScheduledReport")`) — and this service spells
/// out how a caller unsets or re-defaults an optional property: an explicit flag on the
/// request (`clearWidgetIds`, `clearWidgetDateRangeOverride`). `UpdateScheduledReportRequest`
/// models no such flag for the schedule state ...
pub fn updated_schedule_config(
    stored: &ScheduleConfig,
    mut submitted: ScheduleConfig,
) -> ScheduleConfig {
    if submitted.state.as_deref().is_none_or(str::is_empty) {
        submitted.state = Some(schedule_config_state(stored).to_string());
    }
    submitted
}
```
`submitted` is returned as-is apart from `state`, so `scheduleExpressionTimeZone` and `schedulePeriod` come only from the request.

The handler call site, src/ops/update_scheduled_report.rs:346-355:
```
if let Some(schedule_config) = input.schedule_config {
    // The config the request carries replaces the stored one, with ONE property
    // carried over: its `state`. ...
    report.schedule_config =
        crate::state::updated_schedule_config(&report.schedule_config, schedule_config);
}
```
The request models no flag that clears a time zone or a schedule period either — `smithy_query(query="operation",operation="UpdateScheduledReport")` lists only `clearWidgetIds` and `clearWidgetDateRangeOverride`.

Test coverage today pins the wipe rather than the quoted rule — tests/all/update_schedule_state_preservation.rs:232-242:
```
// The members the submitted config carries are the update's own parameters, so
// each of them replaces what was stored.
if let Some(submitted) = &case.submitted {
    for member in ["scheduleExpression", "scheduleExpressionTimeZone"] {
        assert_eq!(schedule.get(member), submitted.get(member), ...);
```
while the report under test was created WITH a time zone (same file, lines 136-139: `"scheduleExpressionTimeZone": "UTC"`), so row 1 (line 55, which submits `scheduleExpression` alone) asserts the stored `UTC` is gone.

## Steps to Reproduce

1. CreateScheduledReport with scheduleConfig = {"scheduleExpression": "cron(0 8 1 * ? *)", "scheduleExpressionTimeZone": "UTC", "schedulePeriod": {"startTime": 1767225600, "endTime": 1798761600}}
2. GetScheduledReport on the returned arn — the stored scheduleConfig carries the expression, the time zone and the period
3. UpdateScheduledReport with {"arn": <arn>, "scheduleConfig": {"scheduleExpression": "cron(0 9 1 * ? *)"}} — a request that names no time zone and no period
4. GetScheduledReport again: scheduleConfig now carries only scheduleExpression and state; scheduleExpressionTimeZone and schedulePeriod are gone, although the request did not name them and the operation documents that "all other properties remain unchanged"
5. ListScheduledReports: the summary's scheduleExpressionTimeZone is likewise absent

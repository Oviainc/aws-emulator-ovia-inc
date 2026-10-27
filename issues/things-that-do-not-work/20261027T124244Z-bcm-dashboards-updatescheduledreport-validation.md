Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ac318820ce0cc5f19770024ed132bf63c4007170

# UpdateScheduledReport accepts a schedulePeriod.endTime the model says must be refused

_Filed: 2026-10-27T12:42:44Z_
_Operation: UpdateScheduledReport | Class: validation_

> _Note: this issue was identified against the code base at commit `ac318820ce0cc5f19770024ed132bf63c4007170`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The documented refusal rule on `SchedulePeriod$endTime` is written for both write paths — "3 years from the time of the create **or update** request" — and `UpdateScheduledReport` replaces the stored `scheduleConfig` wholesale with whatever the request carries, after running `resolve_schedule_config`, which only fills in `state`. No part of the handler examines `schedule_config.schedule_period`, so an update that re-points a report's schedule period past the documented ceiling is accepted with HTTP 200 and persisted.

This is the same unchecked rule as on the create path, but it is a separate code path with its own refusals: the handler already answers its modelled `ValidationException` for a member combination the model cannot express (`clearWidgetIds` together with `widgetIds`) and the service's not-found answer for a `dashboardArn` that resolves to nothing, so the refusal channel is present and used — the schedule period is simply never looked at.

## Evidence

Smithy model, `smithy_query(query="shape",shape="SchedulePeriod")`, member `endTime`:
> "The end time of the schedule period. If not specified, defaults to 3 years from the time of the create or update request. The maximum allowed value is 3 years from the current time. Setting an end time beyond this limit returns a `ValidationException`."

Handler — the submitted config is stored with only `state` resolved, `src/ops/update_scheduled_report.rs:313-317`:
```rust
        if let Some(schedule_config) = input.schedule_config {
            // Resolved at write time, like every other write of a schedule, so every
            // reader sees a state that was decided once rather than guessing one.
            report.schedule_config = crate::state::resolve_schedule_config(schedule_config);
        }
```
`resolve_schedule_config` touches `state` and nothing else — `src/state.rs:926-931`:
```rust
pub fn resolve_schedule_config(mut config: ScheduleConfig) -> ScheduleConfig {
    if config.state.as_deref().is_none_or(str::is_empty) {
        config.state = Some(SCHEDULE_STATE_ENABLED.to_string());
    }
    config
}
```
The same handler does refuse other unexpressible-in-model input, so the channel exists — `src/ops/update_scheduled_report.rs:259-266`:
```rust
                return Err(UpdateScheduledReportErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "{flag}: cannot be true in a request that also specifies {member}"
```

Test coverage: `tests/all/update_scheduled_report.rs` exercises `scheduleConfig` replacement but never submits a `schedulePeriod` at all (grep for `schedulePeriod` in that file: no match), so no test pins acceptance of an out-of-range end time.

## Steps to Reproduce

1. Create a dashboard, then create a scheduled report on it and keep the report `arn`.
2. Call `UpdateScheduledReport` with `{"arn": <report arn>, "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)", "schedulePeriod": {"endTime": 9999999999}}}`.
3. Observe HTTP 200 and the `arn` echoed, where the model documents that setting an end time beyond the 3-year ceiling "returns a ValidationException" (expected: HTTP 400 `ValidationException`, stored record unchanged).
4. Call `GetScheduledReport` and observe the out-of-range `schedulePeriod.endTime` persisted on the record.

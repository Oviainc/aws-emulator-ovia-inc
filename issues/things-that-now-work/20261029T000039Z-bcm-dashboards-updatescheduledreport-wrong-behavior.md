Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# An update that supplies scheduleConfig without `state` silently re-ENABLES a DISABLED scheduled report

_Filed: 2026-10-29T00:00:39Z_
_Operation: UpdateScheduledReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateScheduledReport` runs every supplied `scheduleConfig` through `crate::state::resolve_schedule_config`, which is the CREATE-time default resolution: it writes `state = ENABLED` whenever the submitted config leaves `state` unset. So an update whose only purpose is to change the cron expression of a PAUSED report also flips that report back to `ENABLED` — a property the caller never named.

The model attaches the ENABLED default to the CREATE input alone (`ScheduledReportInput$scheduleConfig`), not to the update member, and `UpdateScheduledReport`'s own documentation says only the parameters in the request are updated while all other properties remain unchanged. The crate also states the opposite rule for itself twice: this handler's own comment says "Present replaces, absent leaves alone", and `update_dashboard.rs` argues from the model that this service unsets an optional member only through an explicit clear flag (`clearWidgetIds`, `clearWidgetDateRangeOverride`) — and `UpdateScheduledReportRequest` models NO clear flag for the schedule state, so an omitted `state` is not an instruction to clear or re-default it.

The effect is observable and not cosmetic: after such an update `GetScheduledReport`/`ListScheduledReports` report `state: ENABLED`, and `ExecuteScheduledReport` — which refuses a non-ENABLED report with `ValidationException` — now triggers an execution and publishes an EventBridge notification for a report the account had paused.

## Evidence

Model (smithy_query query="doc" shape="UpdateScheduledReport"): "Updates an existing scheduled report's properties, including its name, description, schedule configuration, and widget settings. Only the parameters included in the request are updated; all other properties remain unchanged."

Model (smithy_query query="shape" shape="ScheduledReportInput"), the CREATE member: `scheduleConfig` — "The schedule configuration that defines when and how often the report is generated. If the schedule state is not specified, it defaults to ENABLED."
Model (smithy_query query="shape" shape="UpdateScheduledReportRequest"), the UPDATE member: `scheduleConfig` — "The updated schedule configuration for the report." (no defaulting sentence), and the only clear flags it declares are `clearWidgetIds` — "Set to true to clear existing widgetIds." — and `clearWidgetDateRangeOverride`.
Model (`ScheduleConfig$state`): "ENABLED means the scheduled report runs according to its schedule expression. DISABLED means the scheduled report is paused and will not run until re-enabled."

Handler, services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/update_scheduled_report.rs:329-333:
```
        if let Some(schedule_config) = input.schedule_config {
            // Resolved at write time, like every other write of a schedule, so every
            // reader sees a state that was decided once rather than guessing one.
            report.schedule_config = crate::state::resolve_schedule_config(schedule_config);
        }
```
The resolution it calls, src/state.rs:1382-1387:
```
pub fn resolve_schedule_config(mut config: ScheduleConfig) -> ScheduleConfig {
    if config.state.as_deref().is_none_or(str::is_empty) {
        config.state = Some(SCHEDULE_STATE_ENABLED.to_string());
    }
    config
}
```

The crate's own contradicting rule, src/ops/update_scheduled_report.rs:314-316:
"// Present replaces, absent leaves alone — the partial update this operation
// documents. `description` has no `clear` flag of its own, so an absent one keeps
// the stored description rather than dropping it."
and src/ops/update_dashboard.rs:252-259:
"This service spells out how a caller unsets an optional member instead: `UpdateScheduledReportRequest` carries `clearWidgetIds` and `clearWidgetDateRangeOverride` (boolean, @default false) for precisely that, and `UpdateDashboardRequest` models no such flag for `description` or `widgets` — so omission is not a clear here."

The refusal the flip removes, src/ops/execute_scheduled_report.rs:262:
```
            if record.schedule_state() != crate::state::SCHEDULE_STATE_ENABLED {
```

Test coverage: tests/all/update_scheduled_report.rs:144-159 pins only "schedule config without a state is stored ENABLED", and its baseline report is created with `"state": "ENABLED"` (tests/all/update_scheduled_report.rs:55-58), so no test pins the DISABLED -> ENABLED transition; preserving a stored state when the update omits it leaves that row passing.

## Steps to Reproduce

1. CreateDashboard with one widget, note the dashboard ARN.
2. CreateScheduledReport with `scheduledReport.scheduleConfig = {"scheduleExpression": "cron(0 8 1 * ? *)", "state": "DISABLED"}`; note the report ARN.
3. GetScheduledReport -> `scheduledReport.scheduleConfig.state` is `DISABLED`; ExecuteScheduledReport on it is refused 400 ValidationException ("Member must be in ENABLED state").
4. UpdateScheduledReport with `{"arn": <report>, "scheduleConfig": {"scheduleExpression": "cron(0 9 1 * ? *)"}}` — the request names no `state`.
5. GetScheduledReport again: `scheduleConfig.state` now reads `ENABLED` (expected: still `DISABLED`, since the request included no state and the model declares no clear flag for it), and `ListScheduledReports` reports `state: ENABLED`.
6. ExecuteScheduledReport on the same ARN is now accepted and triggers an execution (and the EventBridge notification) for the report the account had paused.

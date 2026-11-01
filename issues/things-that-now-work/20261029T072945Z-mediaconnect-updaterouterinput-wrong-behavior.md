Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8acf21a252327a123d13b159a352e2f9f2a954c2

# A replaced MaintenanceConfiguration leaves the router input's MaintenanceSchedule at the window derived from the previous preference

_Filed: 2026-10-29T07:29:45Z_
_Operation: UpdateRouterInput | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8acf21a252327a123d13b159a352e2f9f2a954c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateRouterInput stores a replacement `MaintenanceConfiguration` on the record but never touches the window that was DERIVED from the old one (`StoredRouterInput::maintenance_schedule`). For an input that StartRouterInput has already scheduled a window for, the result is a record that contradicts itself: `MaintenanceConfiguration` reports the new preferred day/time while `MaintenanceSchedule` keeps reporting the concrete window computed from the preference the input no longer carries. The stale window is served by GetRouterInput, BatchGetRouterInput and ListRouterInputs, and `StoredRouterInput::settle` only re-derives it once that old window has CLOSED (`maintenance_window_has_closed`), i.e. up to a week later for a weekly preference.

The crate applies the opposite rule on the flow side: UpdateFlow's `merged_maintenance` explicitly drops the service-derived `MaintenanceDeadline` as soon as the window is rescheduled, "because the deadline belongs to the window it was issued for". The derived router window is the same kind of value and is left in place.

`grep -n maintenance_schedule src/ops` shows the only writers are create (`None`) and the two start handlers: no update path clears or re-derives it.

## Evidence

The model says the update member carries changes to the SCHEDULES, and that the reported schedule is the CURRENT one:
- `smithy_query(query="doc",shape="UpdateRouterInputRequest",member="MaintenanceConfiguration")`: "The updated maintenance configuration settings for the router input, including any changes to preferred maintenance windows and schedules."
- `smithy_query(query="doc",shape="RouterInput",member="MaintenanceSchedule")`: "The current maintenance schedule details for this router input."

The handler writes the new preference and nothing else — src/ops/update_router_input.rs:345-351 and :377-379:
```
        // A replacement maintenance configuration is judged the way the create judges the one
        // it stores: a `PreferredDayTime` preference whose `Time` names no time of day names
        // no preference, and the window a start schedules is derived from it, so the update
        // refuses it rather than storing it ...
        if let Some(rejection) = input
            .maintenance_configuration
...
        if let Some(maintenance_configuration) = input.maintenance_configuration.clone() {
            router_input.maintenance_configuration = maintenance_configuration;
        }
```
(the comment itself states "the window a start schedules is derived from it", and no line of this handler touches `router_input.maintenance_schedule`).

The stored window is documented as derived from the configuration — src/state.rs:4037-4045:
```
    /// The maintenance window this input is next maintained in
    /// (`RouterInput$MaintenanceSchedule`), or `None` while nothing has scheduled one.
    ///
    /// StartRouterInput is the call that schedules the window, from the input's maintenance
    /// CONFIGURATION ([`maintenance_window_schedule`]); it is held on the record rather than
    /// recomputed per read so every later read reports the same window ...
    pub maintenance_schedule: Option<MaintenanceSchedule>,
```
and src/state.rs:3555-3560:
```
/// The maintenance window a resource configured by `configuration` is next maintained in,
/// as of `from`.
///
/// The schedule is DERIVED from the resource's maintenance configuration and the instant it
/// was scheduled at, never carried by a request ...
```
The clock only repairs it after the stale window closes — src/state.rs:4075-4084 (`StoredRouterInput::settle`):
```
        if self
            .maintenance_schedule
            .as_ref()
            .is_some_and(|schedule| maintenance_window_has_closed(schedule, now))
        {
            self.maintenance_schedule = Some(maintenance_window_schedule(
```
The sibling rule this crate applies to a derived maintenance value when the preference moves — src/ops/update_flow.rs:167-172:
```
    if let Some(date) = update.maintenance_scheduled_date.clone() {
        merged.maintenance_scheduled_date = Some(date);
        // The deadline belongs to the window it was issued for: a window that is
        // rescheduled has no deadline until the service issues one for the new date.
        merged.maintenance_deadline = None;
    }
```
Test coverage: tests/all/router_input_start.rs:540-594 pins that a SPENT window is re-derived from the configuration on the next read; tests/all/update_router_input.rs only lists `maintenanceSchedule` among the members a read reports (lines 73-74) and no test pins a schedule surviving a configuration change.

## Steps to Reproduce

1. POST /v1/routerInput with {"maintenanceConfiguration":{"preferredDayTime":{"day":"MONDAY","time":"02:00"}}, ...} — 201; keep the Arn.
2. Wait one transition span, then POST /v1/routerInput/{Arn}/start — 202; note the returned maintenanceSchedule window (next Monday 02:00 UTC).
3. PUT /v1/routerInput/{Arn} with {"maintenanceConfiguration":{"preferredDayTime":{"day":"FRIDAY","time":"10:00"}}} — 202.
4. GET /v1/routerInput/{Arn} — observe maintenanceConfiguration reporting FRIDAY/10:00 while maintenanceSchedule still reports the Monday 02:00 window derived from the replaced preference.

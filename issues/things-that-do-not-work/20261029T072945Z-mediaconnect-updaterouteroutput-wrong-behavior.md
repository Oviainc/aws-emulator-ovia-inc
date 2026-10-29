Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8acf21a252327a123d13b159a352e2f9f2a954c2

# A replaced MaintenanceConfiguration leaves the router output's MaintenanceSchedule at the window derived from the previous preference

_Filed: 2026-10-29T07:29:45Z_
_Operation: UpdateRouterOutput | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8acf21a252327a123d13b159a352e2f9f2a954c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateRouterOutput stores a replacement `MaintenanceConfiguration` but leaves `StoredRouterOutput::maintenance_schedule` — the concrete window StartRouterOutput derived from the PREVIOUS preference — untouched. A started output therefore reports a `MaintenanceSchedule` its own `MaintenanceConfiguration` no longer implies, through GetRouterOutput, BatchGetRouterOutput and ListRouterOutputs, and `StoredRouterOutput::settle` only re-derives the window once the stale one has CLOSED (up to a week later for a weekly preference).

This is the router-output half of the same defect as on UpdateRouterInput; the crate's own flow-side update does the opposite, dropping the derived `MaintenanceDeadline` as soon as the window is rescheduled.

## Evidence

The model states both halves:
- `smithy_query(query="doc",shape="UpdateRouterOutputRequest",member="MaintenanceConfiguration")`: "The updated maintenance configuration settings for the router output, including any changes to preferred maintenance windows and schedules."
- `smithy_query(query="doc",shape="RouterOutput",member="MaintenanceSchedule")`: "The current maintenance schedule details for this router output."

The handler writes only the preference — src/ops/update_router_output.rs:345-356 and :379-381:
```
        // A replacement maintenance configuration is judged the way the create judges the
        // one it stores: a `PreferredDayTime` preference whose `Time` names no time of day
        // names no preference, and the window a start schedules is derived from it, so the
        // update refuses it rather than storing it ...
...
        if let Some(maintenance_configuration) = input.maintenance_configuration.clone() {
            output.maintenance_configuration = maintenance_configuration;
        }
```
Nothing in the file assigns `output.maintenance_schedule`.

The stored window is documented as the window the service RESOLVED the configuration to, naming an update as a stater of that preference — src/state.rs:4523-4535:
```
    /// The maintenance window currently scheduled for this output, or `None` while none is.
    ///
    /// `RouterOutput$MaintenanceSchedule` is optional while
    /// [`Self::maintenance_configuration`] is reported for every output: the configuration
    /// is the PREFERENCE a create or an update states, and this is the concrete window the
    /// service resolved it to ([`maintenance_window_schedule`]) — which StartRouterOutput
    /// schedules and answers ...
    pub maintenance_schedule: Option<MaintenanceSchedule>,
```
Where the window is derived from the configuration — src/ops/start_router_output.rs:257-269:
```
        let maintenance_schedule = match router_output.maintenance_schedule.clone() {
            Some(maintenance_schedule) => maintenance_schedule,
            None => {
                let maintenance_schedule = crate::state::maintenance_window_schedule(
                    &router_output.maintenance_configuration,
                    now,
                );
```
and the clock's repair only after the window closes — src/state.rs:4580-4589 (`StoredRouterOutput::settle`).
The crate's opposite rule for a derived maintenance value — src/ops/update_flow.rs:169-171: "The deadline belongs to the window it was issued for: a window that is rescheduled has no deadline until the service issues one for the new date."
Test coverage: tests/all/router_output_start.rs:460-640 pins the window a start schedules and its re-derivation once spent; tests/all/update_router_output.rs pins no relation between a replaced configuration and the stored schedule.

## Steps to Reproduce

1. POST /v1/routerOutput with {"maintenanceConfiguration":{"preferredDayTime":{"day":"MONDAY","time":"02:00"}}, ...} — 201; keep the Arn.
2. Wait one transition span, then POST /v1/routerOutput/{Arn}/start — 202; note the returned maintenanceSchedule window.
3. PUT /v1/routerOutput/{Arn} with {"maintenanceConfiguration":{"preferredDayTime":{"day":"FRIDAY","time":"10:00"}}} — 202.
4. GET /v1/routerOutput/{Arn} — observe the new maintenanceConfiguration beside the OLD maintenanceSchedule window.

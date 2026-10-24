Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# CreateFlow stores a malformed Maintenance window and an out-of-Region AvailabilityZone that UpdateFlow and the router creates refuse

_Filed: 2026-10-24T07:18:02Z_
_Operation: CreateFlow | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two members CreateFlow stores verbatim carry documented formats the handler never checks, and the crate already encodes both rules for other operations:

1. `AddMaintenance$MaintenanceStartHour` — "Use 24-hour HH:MM format. Minutes must be 00. Example: 13:00." CreateFlow copies the string into the stored flow unchanged, so `{"maintenance":{"maintenanceDay":"Monday","maintenanceStartHour":"25:77"}}` answers 201 and DescribeFlow reports `maintenanceStartHour: "25:77"` forever. The IDENTICAL value on UpdateFlow is refused with 400 BadRequestException (`is_maintenance_start_hour`), so the same flow can hold a value its own update path calls invalid — expected is a 400 at create too, with nothing stored.

2. `CreateFlowRequest$AvailabilityZone` — "The Availability Zone that you want to create the flow in. These options are limited to the Availability Zones within the current Region." CreateFlow accepts any non-empty string: a request signed for us-west-2 naming `"eu-west-1b"` (or `"not-a-zone"`) is stored and reported as the flow's Availability Zone. CreateRouterInput and CreateRouterOutput in this same crate refuse exactly that with `crate::state::is_zone_of_region`, so the rule and its wording already exist; expected is a 400 BadRequestException for a zone that is not of the request's Region.

Both are accepted-invalid-input gaps: the flow is created and every later read reports a value the real service would never have stored.

## Evidence

Spec: `smithy_query(query="shape",shape="AddMaintenance")` — `MaintenanceStartHour` (required): "UTC time when the maintenance will happen. Use 24-hour HH:MM format. Minutes must be 00. Example: 13:00."; `smithy_query(query="operation",operation="CreateFlow")` input member `AvailabilityZone` with the "limited to the Availability Zones within the current Region" doc.

Handler: `src/ops/create_flow.rs:513-519` builds `Maintenance` straight from the request (`maintenance_start_hour: Some(maintenance.maintenance_start_hour.clone())`) with no format check, and `src/ops/create_flow.rs:502-506` takes `AvailabilityZone` as given (only emptiness is filtered). The checks the crate already owns: `src/ops/update_flow.rs:133` (`is_maintenance_start_hour`) applied at `src/ops/update_flow.rs:343-364`, and `crate::state::is_zone_of_region` applied at `src/ops/create_router_input.rs:252-263` / `src/ops/create_router_output.rs:243-256`.

Test coverage: none for the create path. `tests/all/error_envelopes.rs:718-726` sends `maintenanceStartHour: "25:77"` by PUT (UpdateFlow) only, and `tests/all/update_flow.rs:500-508,745-791` cover the update path; `tests/all/flow_lifecycle.rs:237-257` only ever passes an Availability Zone of the request's own Region. Nothing asserts the create-side behaviour either way. (The earlier CreateFlow validation issue — two-source bound and NdiState on a non-LARGE flow — is a different rule and is already fixed.)

## Steps to Reproduce

1. POST /v1/flows with {"name":"f-maint","source":{"name":"s","protocol":"rtp","whitelistCidr":"10.0.0.0/16"},"maintenance":{"maintenanceDay":"Monday","maintenanceStartHour":"25:77"}} — observe HTTP 201 where AWS answers 400 BadRequestException
2. GET /v1/flows/{FlowArn} — flow.maintenance.maintenanceStartHour is reported as "25:77"
3. PUT /v1/flows/{FlowArn} with {"maintenance":{"maintenanceStartHour":"25:77"}} — observe 400 BadRequestException, i.e. the create accepted what the update refuses
4. POST /v1/flows (request signed for us-west-2) with {"name":"f-az","availabilityZone":"eu-west-1b","source":{...}} — observe HTTP 201 and DescribeFlow reporting availabilityZone "eu-west-1b", where AWS refuses a zone outside the current Region (compare POST /v1/routerInput with the same zone, which this emulator already refuses)

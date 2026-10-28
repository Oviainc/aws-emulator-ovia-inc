Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# GetGateway never reports MaintenanceStartTime: a newly created gateway is given no default maintenance window

_Filed: 2026-10-24T03:08:27Z_
_Operation: GetGateway | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a gateway appliance always has a weekly maintenance window — the service assigns a default one when the gateway is activated, and `GatewayDetails$MaintenanceStartTime` is documented as "Returns your gateway's weekly maintenance start time including the day and time of the week". So GetGateway on a gateway that has never been touched by PutMaintenanceStartTime still reports a MaintenanceStartTime (HourOfDay/MinuteOfHour plus DayOfWeek or DayOfMonth).

Actual: `state::create_gateway` mints the record with `maintenance_start_time: None` and `StoredGateway::details()` passes that straight through, so GetGateway omits MaintenanceStartTime entirely for every gateway; the member can only ever appear once PutMaintenanceStartTime writes it (and that operation is still a 501 scaffold), meaning no response of this emulator ever carries the member. A client that reads `Gateway.MaintenanceStartTime.HourOfDay` to display or compare the window gets an absent member where AWS returns a value.

## Evidence

Spec: smithy_query(shape="GatewayDetails") — MaintenanceStartTime (MaintenanceStartTime shape) documented as "Returns your gateway's weekly maintenance start time including the day and time of the week...", and PutMaintenanceStartTime is documented as SETting (overriding) the window rather than creating the only one; well-established gateway-appliance behavior (the same appliance stack assigns a default maintenance window at activation). Handler/state: src/state.rs `create_gateway` sets `maintenance_start_time: None` with the comment "absent until PutMaintenanceStartTime sets one"; src/ops/get_gateway.rs returns `gateway.details()` unchanged. Tests: tests/all/gateway_lifecycle.rs asserts the create/read round trip but never asserts a MaintenanceStartTime on a fresh gateway, so nothing covers it.

## Steps to Reproduce

1. Boot the emulator, make a gateway exist (CreateGateway with an issued activation key), and note its GatewayArn.
2. Call GetGateway with that GatewayArn.
3. Observe the Gateway object has no MaintenanceStartTime member; expected a default weekly window (HourOfDay/MinuteOfHour, DayOfWeek) assigned at activation.
4. Observe the same absence in a GetGateway on any gateway for which PutMaintenanceStartTime was never called.

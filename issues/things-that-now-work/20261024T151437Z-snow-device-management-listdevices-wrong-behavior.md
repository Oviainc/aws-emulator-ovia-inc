Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# ListDevices answers 403 AccessDeniedException instead of an empty device list, and no device inventory is ever seeded

_Filed: 2026-10-24T15:14:37Z_
_Operation: ListDevices | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For any client talking to the running emulator, `GET /managed-devices` can only answer HTTP 403 `AccessDeniedException` ("no devices in this Region"), never a 200 with a `devices` list.

Two problems compound:

1. The inventory is unreachable. `ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::activate_devices` is documented as "the ONLY way a device enters the inventory", and it is called from NOTHING in `src/` — only from the four integration tests. There is no startup seeding and no request path, so a live emulator's `devices` map is permanently empty.
2. On an empty inventory the handler refuses the call: `if !store.has_activated_device(&region) { return Err(AccessDeniedException(no_devices_in_region())) }`. Expected: an account/Region with no Snow devices is not an authorization failure — ListDevices is documented as "Returns a list of all devices on your account that have Snow Device Management enabled in the Region where the command is run", and AWS list APIs answer an empty collection (`{"devices": []}`, HTTP 200) when there is nothing to enumerate. `AccessDeniedException` ("You don't have sufficient access to perform this action") is an IAM outcome, not an inventory-size outcome.

Because of (1), every other gated operation (CreateTask, DescribeDevice, DescribeTask, DescribeExecution, ListExecutions, ListTasks, CancelTask, DescribeDeviceEc2Instances) is also unconditionally 403 in a real emulator run, so the whole device/task surface is unusable outside the crate's own tests.

## Evidence

Spec: Smithy model `ListDevices` -> `GET /managed-devices`, output `devices` (DeviceSummaryList) + `nextToken`, errors AccessDenied/InternalServer/Throttling/Validation; the operation doc describes enumeration of the account's devices with no notion of an account-level 403 gate. The already-FIXED bootstrap issue for this service (20261021T222258Z-bootstrap-...) explicitly scoped ListDevices as "the emulator seeds a small device inventory at startup, since the API models no device-creation operation" and its step 2 expects "a non-empty `devices` list of seeded managed devices with stable ids/ARNs" — that seeding was never implemented.

Handler: `src/ops/list_devices.rs` lines ~164-174 (gate before anything else). State: `src/state.rs` `has_activated_device` / `activate_devices` (doc: "deliberately not reachable from a request"). `grep activate_devices` matches only `tests/all/*.rs` — no `src/` caller.

Tests: `tests/all/account_device_gate.rs` asserts the 403 gate, and every other suite injects devices through `service_state(...).lock().activate_devices(...)` before calling — i.e. the only passing path is one no HTTP client can take.

## Steps to Reproduce

1. Start the emulator with a fresh in-memory engine (no test harness).
2. Call ListDevices (GET /managed-devices) signed for any region.
3. Observe HTTP 403 with `__type` AccessDeniedException; expected HTTP 200 with a `devices` array (empty for a truly empty account, or the seeded inventory).
4. Call DescribeDevice / CreateTask / ListTasks and observe the same 403, showing no device can ever exist in a live run.

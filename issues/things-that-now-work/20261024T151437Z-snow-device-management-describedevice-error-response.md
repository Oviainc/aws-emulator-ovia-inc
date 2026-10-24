Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# DescribeDevice (and the other id-addressed operations) answer 403 AccessDeniedException where the modeled ResourceNotFoundException is owed

_Filed: 2026-10-24T15:14:37Z_
_Operation: DescribeDevice | Class: error-response_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`POST /managed-device/{managedDeviceId}/describe` first consults the account-level device gate and answers HTTP 403 `AccessDeniedException` whenever the signed Region holds no activated device — which, in a live emulator, is always (nothing in `src/` populates the inventory). A caller addressing a device id therefore never receives the modeled HTTP 404 `ResourceNotFoundException` for an id that does not exist; it receives an authorization failure instead.

Expected: an id that names no managed device is answered with `ResourceNotFoundException` (404); `AccessDeniedException` is reserved for a caller whose credentials/policy do not permit the action. The same gate is applied identically in `describe_task.rs`, `describe_execution.rs`, `cancel_task.rs`, `list_executions.rs` and `create_task.rs`, so every id-addressed operation of this service substitutes 403 for the modeled 404.

## Evidence

Spec: `smithy_query(operation="DescribeDevice")` declares `ResourceNotFoundException` (404, "The request references a resource that doesn't exist") alongside `AccessDeniedException` (403, "You don't have sufficient access to perform this action") — two distinct failure modes; nothing in the model makes a device-free account an authorization failure. The already-fixed bootstrap issue for this service also specified "An unknown id must return `ResourceNotFoundException` (HTTP 404)".

Handler: `src/ops/describe_device.rs` lines ~175-194 — the gate (`has_activated_device`) runs and returns AccessDeniedException before the `device_in(...)` lookup that would produce the 404. `src/state.rs::no_devices_in_region` documents the design. Same pattern in `create_task.rs` ~182-187, `describe_task.rs` ~167-173, `describe_execution.rs` ~167-173, `cancel_task.rs` ~160-166, `list_executions.rs` ~169-174.

Tests: `tests/all/account_device_gate.rs` asserts the 403-before-id-resolution behavior, so it is pinned by a test rather than flagged.

## Steps to Reproduce

1. Start the emulator with a fresh engine (no test-only activate_devices call).
2. Call DescribeDevice with `managedDeviceId="smd-0000000000000000"`.
3. Observe HTTP 403 AccessDeniedException; expected HTTP 404 ResourceNotFoundException for an id that names no device.
4. Repeat with DescribeTask / DescribeExecution / CancelTask / ListExecutions and observe the same 403 substitution.

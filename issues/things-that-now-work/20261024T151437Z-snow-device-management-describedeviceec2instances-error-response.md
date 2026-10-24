Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# DescribeDeviceEc2Instances answers 403 AccessDeniedException for an unknown managedDeviceId instead of the modeled ResourceNotFoundException

_Filed: 2026-10-24T15:14:37Z_
_Operation: DescribeDeviceEc2Instances | Class: error-response_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `managedDeviceId` names no device in the Region — even with a non-empty inventory, i.e. the account IS activated and other devices resolve — the handler returns HTTP 403 `AccessDeniedException` via `store.gated_device(...)`. Expected: HTTP 404 `ResourceNotFoundException` ("The request references a resource that doesn't exist"), which the model declares for exactly this operation and which the sibling `DescribeDevice` handler already returns for the same condition on the same id space.

The result is an internally inconsistent emulator: `POST /managed-device/does-not-exist/describe` -> 404, while `POST /managed-device/does-not-exist/resources/ec2/describe` -> 403, for the same non-existent device and the same caller.

## Evidence

Spec: `smithy_query(operation="DescribeDeviceEc2Instances")` lists `ResourceNotFoundException` (httpError 404) among its errors, documented "The request references a resource that doesn't exist" — the standard AWS code for an id that resolves to nothing. AccessDeniedException is modeled too, but as the insufficient-permission outcome.

Handler: `src/ops/describe_device_ec2_instances.rs` lines ~182-189 — `match store.gated_device(&region, &input.managed_device_id) { Err(denied) => return Err(...AccessDeniedException(denied)) }`; `src/state.rs::gated_device` maps a missing device to `no_devices_in_region()`. Contrast `src/ops/describe_device.rs` lines ~186-194, which returns `ResourceNotFoundException` for the identical condition.

Tests: `tests/all/device_ec2_instances.rs` lines 18 and 447-453 assert the 403, so the divergence is asserted rather than detected.

## Steps to Reproduce

1. Start an emulator whose inventory holds at least one device in the signed Region (so the account-level gate does not fire).
2. Call DescribeDeviceEc2Instances with `managedDeviceId` set to an id that does not exist and `instanceIds=["i-0123456789abcdef0"]`.
3. Observe HTTP 403 with `__type` AccessDeniedException; expected HTTP 404 ResourceNotFoundException.
4. Call DescribeDevice with the same unknown id and observe HTTP 404 — the two operations disagree about the same missing device.

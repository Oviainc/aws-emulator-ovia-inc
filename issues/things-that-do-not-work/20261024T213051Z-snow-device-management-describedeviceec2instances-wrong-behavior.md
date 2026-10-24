Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# The device's EC2 instance cache is frozen: transient instance states never resolve, lastUpdatedAt never advances, and a completed reboot task changes nothing

_Filed: 2026-10-24T21:30:51Z_
_Operation: DescribeDeviceEc2Instances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeDeviceEc2Instances` reads a per-device Amazon EC2 instance cache that is written exactly once, when a device enters the inventory, and is NEVER written again by any code path. Three observable consequences:

1. Transient instance states never resolve. The fixture cycles instances through all six `InstanceStateName` values, so the cache permanently holds instances in `PENDING` (code 0), `SHUTTING_DOWN` (32) and `STOPPING` (64). Those three states are by definition in-flight: a `PENDING` instance must become `RUNNING`, a `SHUTTING_DOWN` instance must become `TERMINATED`, a `STOPPING` instance must become `STOPPED`. In the emulator a caller that polls `DescribeDeviceEc2Instances` waiting for `PENDING` -> `RUNNING` polls forever: every call returns the same state/code pair for the life of the process.

2. `InstanceSummary$lastUpdatedAt` ("When the instance summary was last updated") and `Instance$updatedAt` are pinned to the instant the device was activated and never advance, even though the value is modeled precisely to tell a caller how fresh the cloud-side cache of that instance is.

3. A task whose command is `reboot` has NO effect on the instances the rebooted device runs. `src/state.rs::settle` matches `TaskCommand::Reboot => {}` (with the comment "A reboot leaves no member of the device record different afterwards"), so after a reboot execution reaches `SUCCEEDED` the device's instances still report the identical state, `createdAt`, `updatedAt` and `lastUpdatedAt` they reported before the reboot. Expected: rebooting a Snow Family device restarts the device's compute environment, so the instance cache the cloud side reports is refreshed afterwards — at minimum the reported `lastUpdatedAt`/`updatedAt` move, and an instance that was mid-transition is no longer reported in that same transient state.

Expected overall: the instance cache is state that evolves (the device pushes updates); the emulator treats it as an immutable fixture, so no sequence of API calls and no passage of time ever produces a different answer for a given device.

## Evidence

Spec: `smithy_query(query="closure", operation="DescribeDeviceEc2Instances")` — `InstanceSummary` carries `lastUpdatedAt` ("When the instance summary was last updated") and `Instance` carries `updatedAt` plus `state` targeting `InstanceStateName` with values `[PENDING, RUNNING, SHUTTING_DOWN, TERMINATED, STOPPING, STOPPED]`. `PENDING`/`SHUTTING_DOWN`/`STOPPING` resolving to `RUNNING`/`TERMINATED`/`STOPPED` is well-established EC2 semantics (the emulator's own `src/state.rs` lines ~689-701 quotes the documented state-code catalog). `CreateTask`'s `Command$reboot` is documented "Reboots the device."

Handler/state: `src/ops/describe_device_ec2_instances.rs` lines ~196-214 projects `device.ec2_instances` with `Ec2Instance::to_summary()` and performs no state update. `src/state.rs::fixture_instances` (~748-788) is the ONLY writer of `ec2_instances`, called from `activate_devices` (~511) with `created_at: now, updated_at: now`; nothing else in the crate ever mutates an `Ec2Instance` (no `ec2_instances` write outside that fixture, and `settle` touches only `device.last_reached_out_at`, `device.device_state`, executions and tasks). `src/state.rs::settle` ~607-610 is the reboot no-op.

Tests: the frozen-ness is NOT asserted as intended behavior over time. `tests/all/device_ec2_instances.rs::assert_modelled_summary` (~237-318) only checks that `lastUpdatedAt` is present and that the state name/code pair is one of the documented ones; `two_identical_calls_answer_identically` (~485) pins only that two back-to-back calls agree, which a cache that advances on an event would still satisfy. No test creates a reboot task and then re-reads the instances, and no test asserts a transient state resolving.

## Steps to Reproduce

1. Start the emulator with an activated device inventory in the signed Region (the only seam that populates devices) so the account-level device gate lifts.
2. Call `POST /managed-device/{managedDeviceId}/resources/ec2/describe` with the device's full instance id list and record each instance's `state.name`, `instance.updatedAt` and `lastUpdatedAt`. Note that some instances report `PENDING`, `SHUTTING_DOWN` or `STOPPING`.
3. Wait (seconds to minutes) and repeat the same call: every instance still reports the identical transient state, and `lastUpdatedAt`/`updatedAt` are unchanged — a `PENDING` instance never reaches `RUNNING`.
4. Call `CreateTask` with `{"targets":["<managedDeviceId>"],"command":{"reboot":{}}}` and poll `DescribeExecution`/`DescribeTask` until the execution is `SUCCEEDED` and the task `COMPLETED`.
5. Repeat the `DescribeDeviceEc2Instances` call: the answer is byte-for-byte what it was before the reboot — same states, same `updatedAt`, same `lastUpdatedAt` — so the completed reboot left no trace in the instance cache the operation reports.

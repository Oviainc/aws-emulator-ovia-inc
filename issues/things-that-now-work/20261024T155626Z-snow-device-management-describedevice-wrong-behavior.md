Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# deviceState never reports the modeled UNLOCKING transient: an unlock task flips LOCKED straight to UNLOCKED

_Filed: 2026-10-24T15:56:26Z_
_Operation: DescribeDevice | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeDevice$deviceState` is modeled as `UnlockState`, whose values are `LOCKED`, `UNLOCKED` and `UNLOCKING`. In the emulator a device only ever carries `LOCKED` or `UNLOCKED`: `src/state.rs::settle` applies `TaskCommand::Unlock` at the instant the execution reaches `SUCCEEDED`, setting `device_state = "UNLOCKED"` in one step. While the unlock execution is `QUEUED`/`IN_PROGRESS` the device still reports `LOCKED`, and the very next read reports `UNLOCKED`, so `UNLOCKING` is unreachable through the API.

Expected: once an unlock task has been dispatched to the device and is running, `DescribeDevice` reports `deviceState = UNLOCKING`, and only when the device finishes unlocking does it report `UNLOCKED` — which is what the third enum value exists to express (a Snow Family device unlocking is a multi-second transition the caller polls for). A client that polls `DescribeDevice` to wait for the unlock therefore never observes the in-flight state the real service reports, and code that branches on `UNLOCKING` is dead against this emulator.

## Evidence

Spec: `smithy_query(shape="UnlockState")` → values `["UNLOCKED", "LOCKED", "UNLOCKING"]`; `DescribeDeviceOutput$deviceState` targets it and is documented "The current state of the device." `CreateTask`'s `Command$unlock` is documented "Unlocks the device" — the operation whose progress `UNLOCKING` reports. Snow Family device unlocking is a well-known polled transition, which is why the enum carries the intermediate value.

Handler/state: `src/state.rs::settle` lines ~610-630 — the only writer of `device_state`, which moves the device directly to `"UNLOCKED"` when the execution settles to `SUCCEEDED`; `activate_devices` (~465) mints every device `"LOCKED"`. No code path ever assigns `"UNLOCKING"` (grep for `UNLOCKING` in `src/` returns nothing). `src/ops/describe_device.rs` ~196-210 echoes the stored value.

Tests: not covered. `tests/all/device_task_lifecycle.rs` ~1166-1243 asserts only `LOCKED` before the task and `UNLOCKED`/`LOCKED` after it settles; the only mention of `UNLOCKING` is the enum list at line 42, and nothing asserts the device state while the execution is `IN_PROGRESS`.

## Steps to Reproduce

1. Start the emulator with an activated device inventory and confirm DescribeDevice reports deviceState=LOCKED for a device.
2. Call CreateTask with `{"targets":["<managedDeviceId>"],"command":{"unlock":{}}}`.
3. Poll ListExecutions/DescribeExecution until the execution reports IN_PROGRESS (past the dispatch span), then call DescribeDevice for the target device.
4. Observe deviceState=LOCKED; expected UNLOCKING while the unlock is in flight.
5. Poll past the finish span and observe deviceState jumps to UNLOCKED, so no call ever returns the modeled UNLOCKING value.

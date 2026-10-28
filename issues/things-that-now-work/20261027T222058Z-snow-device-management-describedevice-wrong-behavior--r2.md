Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# DescribeDevice lastUpdatedAt never advances when a reboot refreshes the device's instance cache

_Filed: 2026-10-27T22:20:58Z_
_Operation: DescribeDevice | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeDeviceOutput$lastUpdatedAt` is modelled as the instant the device cache was refreshed, but the crate writes `Device::last_updated_at` in exactly ONE place — inside the branch that changes the device's `UnlockState` — and the `reboot` command deliberately reports no `UnlockState` value. The same settling pass that decides 'no UnlockState change' then REFRESHES the whole cloud-side instance cache of that device (`instance.settle(at); instance.restart(at)` for every entry), which advances each entry's own `updated_at` (served as `InstanceSummary$lastUpdatedAt` / `Instance$updatedAt`).

Result: after a `reboot` task completes, `DescribeDeviceEc2Instances` reports every instance as refreshed at the reboot-finish instant and `DescribeDevice` reports `lastReachedOutAt` advanced to that same instant, while `lastUpdatedAt` still carries the activation instant — the device record claims its cache has not been refreshed since activation, contradicting both the modelled sentence and the instance records the crate refreshed in the same statement. A caller polling `lastUpdatedAt` to detect a cache refresh never sees a reboot. Outside the fixture's activation, `device.last_updated_at` is written at exactly one line (state.rs:1192), so a reboot-only device never moves it at all.

## Evidence

Model sentence, quoted in the handler's own header — services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/ops/describe_device.rs:65-66:
```
//! * `lastUpdatedAt` (Timestamp) — When the device last pushed an update to the Amazon Web
//!   Services Cloud. Indicates when the device cache was refreshed.
```
(identical to `smithy_query(query="doc",shape="DescribeDeviceOutput",member="lastUpdatedAt")`: "When the device last pushed an update to the Amazon Web Services Cloud. Indicates when the device cache was refreshed.")

The reboot branch refuses to touch the device record — src/state.rs:1183-1188:
```
// "Reboots the device." A reboot leaves no member of the device
// record different afterwards, and `UnlockState` has no value
// that reports one in flight; only the check-in above, and the
// instances the restart brings back up below, record that it
// happened.
(TaskCommand::Reboot, _) => None,
```
and `last_updated_at` is written only when that value is `Some` — src/state.rs:1190-1193:
```
if let Some(reported) = reported.filter(|state| device.device_state != *state) {
    device.device_state = reported.to_string();
    device.last_updated_at = at;
}
```
(a grep of `src/` for `last_updated_at` shows state.rs:1192 is the only write to a Device's field after state.rs:968, the activation seam.)

Yet the cache IS refreshed in the same loop — src/state.rs:1202-1206:
```
if command == TaskCommand::Reboot && phase == DeviceCommandPhase::Finished {
    for instance in device.ec2_instances.values_mut() {
        instance.settle(at);
        instance.restart(at);
    }
}
```
and that refresh advances the instance-level freshness — src/state.rs:400-404 (`Ec2Instance::enter_state` doc): "the freshness members advance to `at`, since what `InstanceSummary$lastUpdatedAt` and `Instance$updatedAt` report is when the cache entry last changed"; src/state.rs:313-314: "`DescribeDeviceEc2Instances` reads the cloud-side cache of exactly this set".

Test coverage: tests/all/device_ec2_instance_cache.rs:15-21 pins rule 2/3 ("the freshness members ... advance to the instant a change happened"; "a device that answered a `reboot` reports its whole instance set afresh: the freshness members of every entry advance"), and tests/all/reboot_cache_refresh_instant.rs:15-16 reads the reboot-finish instant from "the rebooted device's `lastReachedOutAt`". No test asserts the device's own `lastUpdatedAt` after a reboot — the only assertion on it is tests/all/device_unlock_transient.rs:179-185, which covers an UnlockState change, so nothing pins the current behaviour.

## Steps to Reproduce

1. Activate one device in the signed Region (the state's `activate_devices` seam) and call `POST /managed-device/{id}/describe`; record `lastUpdatedAt` and `lastReachedOutAt` (both the activation instant).
2. Call `POST /task` with `{"targets":["<id>"],"command":{"reboot":{}}}` and wait longer than `EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN`.
3. Call `POST /managed-device/{id}/resources/ec2/describe` with the device's instance ids: every `InstanceSummary.lastUpdatedAt` / `instance.updatedAt` has advanced to the reboot-finish instant — the cache was refreshed.
4. Call `POST /managed-device/{id}/describe` again: `lastReachedOutAt` has advanced to that same reboot-finish instant, but `lastUpdatedAt` is still the activation instant, although the model says it "Indicates when the device cache was refreshed".

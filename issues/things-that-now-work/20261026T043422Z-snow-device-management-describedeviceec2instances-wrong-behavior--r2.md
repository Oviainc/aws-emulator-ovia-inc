Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 388872f1987f74fe9682288e62ee8e18a6dfb8b9

# Post-reboot EC2 instance cache is refreshed at the instant someone looked, not the instant the reboot finished

_Filed: 2026-10-26T04:34:22Z_
_Operation: DescribeDeviceEc2Instances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `388872f1987f74fe9682288e62ee8e18a6dfb8b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a `reboot` task's execution settles, `ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::settle` replays the device effects it owes at the PAST instants they came due (`at`), but the instance-cache half of the reboot effect is applied with `now` — the instant the current request happened to call — instead of `at`.

Every other write in the same loop uses `at` (`device.last_reached_out_at = at`, `device.last_updated_at = at`), and the comment directly above the restart says the refresh "is applied when the reboot finished, not when it began" — `at` IS the instant the reboot finished (the effect was pushed as `(due, DeviceCommandPhase::Finished)`), while `now` is the observation instant.

Consequences a client observes through `DescribeDeviceEc2Instances` (and `ListDeviceResources`' sibling state):
- `InstanceSummary$lastUpdatedAt` / `Instance$updatedAt` report the instant of the FIRST call that happened to settle the reboot, so the cache claims the device pushed its set afresh now, long after the reboot actually finished, and contradicts the same device's `lastReachedOutAt`, which correctly reports the finish instant.
- `Ec2Instance::enter_state` re-arms `transition_due = at + INSTANCE_TRANSITION_SPAN` from that same wrong instant, so an instance the restart brought back up is reported `PENDING` for `INSTANCE_TRANSITION_SPAN` after the LOOK rather than after the reboot: a caller that waits an hour and then polls once still sees `PENDING`, where a caller that polled promptly sees `RUNNING`. The answer therefore depends on whether anybody called earlier, which is exactly what the state module says must not happen.

## Evidence

The defect, `services/.../src/state.rs:1189-1198` (inside `settle`, where `at` is in scope and destructured from the effect):
```
            // The device restarted, so it brought the compute environment its
            // instances run in back up and reported that whole set afresh: the
            // cloud-side cache this account reads is the one the restart produced
            // ([`Ec2Instance::restart`]). That is what the device has DONE, so it
            // is applied when the reboot finished, not when it began.
            if command == TaskCommand::Reboot && phase == DeviceCommandPhase::Finished {
                for instance in device.ec2_instances.values_mut() {
                    instance.restart(now);
                }
            }
```
The sibling writes of the SAME effect, eight lines earlier (`state.rs:1166-1167`, `1185-1188`):
```
            // The device took the command, or answered it, so it checked in.
            device.last_reached_out_at = at;
...
                device.device_state = reported.to_string();
                device.last_updated_at = at;
```
The rule the crate states for exactly this kind of write, `state.rs:413-418` (`Ec2Instance::settle`):
```
    /// Materialise the state change the clock already owes this instance.
    ///
    /// An in-flight state resolves to its outcome AT THE INSTANT IT CAME DUE, not at
    /// the instant someone looked: the same change was owed then even if nobody had
    /// called.
```
and `state.rs:1061-1062` (`settle`'s own doc):
```
    /// Nothing here depends on who called: the same changes were owed at this
    /// instant even if nobody had.
```
`Ec2Instance::restart`'s own contract names the instant it expects, `state.rs:434-435`: "The cache entry of this instance after the device it runs on restarted at `at`." — and it forwards that argument into `self.enter_state(next, at)` (`state.rs:454`), which sets `updated_at` and re-arms `transition_due` (`state.rs:405-411`).

Test coverage does NOT pin the wrong instant: `tests/all/device_ec2_instance_cache.rs:583-587` only asserts `refreshed > staged_at + TOLERANCE`, and `:601-606` accepts `PENDING` or `RUNNING`, so both instants pass today.

## Steps to Reproduce

1. Activate devices in the signed Region (the fixture seam used by the tests) so device `smd-...0` runs EC2 instances.
2. `POST /task` with `{"targets": ["<that device>"], "command": {"reboot": {}}}`.
3. Make NO further call for several seconds — well past `EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + INSTANCE_TRANSITION_SPAN`.
4. `POST /managed-device/<device>/resources/ec2/describe` with the device's instance ids once.
5. Observe `lastUpdatedAt` / `instance.updatedAt` ≈ the instant of THIS call (not the instant the reboot finished, which `POST /managed-device/<device>/describe` reports as `lastReachedOutAt`), and instances that were up reported `PENDING` rather than `RUNNING`.
6. Repeat the same run but poll the describe every 100ms: the same instances report `RUNNING` with `lastUpdatedAt` ≈ reboot-finish + `INSTANCE_TRANSITION_SPAN`, so the answer differs purely because somebody looked earlier.

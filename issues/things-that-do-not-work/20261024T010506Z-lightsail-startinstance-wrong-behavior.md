Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Assign a new public IP address when a stopped instance is started again

_Filed: 2026-10-24T01:05:05Z_
_Operation: StartInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Lightsail assigns a NEW public IPv4 address to an instance each time it is started after having been stopped, unless a static IP is attached to it — this is the whole reason static IPs exist and is documented in the StopInstance note.

Expected: GetInstance after StopInstance (settled to `stopped`) -> StartInstance (settled to `running`) reports a DIFFERENT `publicIpAddress` than before the stop (and a different IPv6 address on an ipv6/dualstack instance), while `isStaticIp` is false; with a static IP attached the address is kept.

Actual: neither StopInstance nor StartInstance, nor the clock that materialises the owed `pending -> running` transition, touches the instance's addresses. `StoredInstance.public_ip_address` is written once, by CreateInstances via `store.fresh_addresses(..)`, and is never re-drawn, so a stop/start cycle reports the exact same public address forever. A client that stops and starts an instance to prove it must use a static IP observes no address change at all.

## Evidence

Spec: the Smithy model's StopInstance documentation — "When you start a stopped instance, Lightsail assigns a new public IP address to the instance. To use the same IP address after stopping and starting an instance, create a static IP address and attach it to the instance." The emulator is aware of the rule: `src/ops/reboot_instance.rs` cites it ("unlike a stop and start, after which Lightsail assigns a new public IP address (the modelled StopInstance documentation)") to justify a reboot preserving addresses. Handlers: `src/ops/start_instance.rs` and `src/ops/stop_instance.rs` only move `state`/`due`; `src/state.rs::advance_clock` only sets `instance.state`, and `fresh_addresses` is called nowhere but CreateInstances. Tests: `tests/all/instance_lifecycle.rs::stopping_and_starting_move_the_stored_state` asserts only the lifecycle states; `tests/all/instance_reboot.rs` asserts address PRESERVATION for a reboot (correct) — no test covers a stop/start, so this is uncovered.

## Steps to Reproduce

1. CreateInstances with a dualstack instance and wait for state `running`; GetInstance and record `publicIpAddress`
2. StopInstance and wait for state `stopped`
3. StartInstance and wait for state `running`
4. GetInstance again: `publicIpAddress` is unchanged, where real Lightsail reports a newly assigned public IP (no static IP is attached, `isStaticIp` is false)

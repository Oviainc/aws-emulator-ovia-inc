Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# AssociateUser authorizes instances against a 3-entry hardcoded fleet, refusing every real EC2 instance

_Filed: 2026-10-24T01:06:14Z_
_Operation: AssociateUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AssociateUser gates every request on `state.authorizes_instance(&input.instance_id)`, which is backed by `account_instances` — a map seeded ONLY from the hardcoded list in `crate::state::seed_account_instances()` (`i-0f1e2d3c4b5a69788`, `i-0e2d3c4b5a6978899`, `i-0d3c4b5a697889900`). Any other instance id is refused with the non-modelled `UnauthorizedException` (HTTP 400) before the request shape is even validated (the same gate runs in `service.rs::override_operation`).

Expected: an EC2 instance the account actually owns can be associated. In the real service, `AssociateUser` resolves the `InstanceId` against the caller's EC2 fleet, so an instance created by `ec2:RunInstances` (which this repo's EC2 emulator implements for real, `services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs`) is a valid target and the call answers 200 with an `InstanceUserSummary` in `ASSOCIATING`.

Actual: a client that creates an instance through the EC2 emulator and then calls AssociateUser with that instance id always gets 400 `UnauthorizedException` ("Unauthorized. Please check the request and access policy and try again."). The only way to make an instance resolvable is the in-process test seam `register_account_instance`, which no API client can reach, so the whole user-association plane (AssociateUser -> ListUserAssociations -> DisassociateUser -> ListInstances) is unreachable for any realistically-obtained instance id. The fleet seam should be fed from (or widened to accept) instances that exist in the emulator rather than three fixed ids.

## Evidence

Spec/semantics: `AssociateUser` input `InstanceId` is documented as "The ID of the EC2 instance that provides the user-based subscription" (Smithy model, AssociateUserRequest), i.e. any instance of the caller's account; the model declares no closed set of instance ids. Handler: `src/ops/associate_user.rs` lines 191-193 (`if !state.authorizes_instance(&input.instance_id) { return Err(Scaffolded::unauthorized()) }`) and `src/service.rs` lines 111-120/158-163. State: `src/state.rs` `seed_account_instances()` (line 432) returns exactly 3 ids; `authorizes_instance` (line 615) is a pure `account_instances.contains_key`; the only writer is `register_account_instance` (line 578), used only by the test harness (`tests/all/identity_provider_subscriptions.rs`). Tests: `the_instance_gate_fails_closed_before_any_other_lookup` only pins that ids NOT in the seeded fleet are refused — nothing asserts that an instance which exists elsewhere in the emulator is accepted.

## Steps to Reproduce

1. Start the emulator with both the EC2 and license-manager-user-subscriptions services mounted.
2. Call ec2:RunInstances and note the returned instance id (e.g. i-0abc...).
3. Call license-manager-user-subscriptions:RegisterIdentityProvider for a directory and product so a registration exists.
4. Call AssociateUser with Username=alice, that EC2 instance id, and the registered IdentityProvider.
5. Observe HTTP 400 with error code UnauthorizedException; expected HTTP 200 with InstanceUserSummary{Status: ASSOCIATING, InstanceId: <that id>}.
6. Repeat with the hardcoded id i-0f1e2d3c4b5a69788 and observe it succeeds — proving the gate keys on a fixed 3-entry list rather than on instances the account has.

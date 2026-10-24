Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Authorize AssociateUser against the EC2 emulator's instance store, not a 3-id hardcoded seed

_Filed: 2026-10-24T13:26:29Z_
_Operation: AssociateUser | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AssociateUser only succeeds for one of three hardcoded EC2 instance ids. Every other `InstanceId` — including an instance the caller just launched through the EC2 emulator's implemented `RunInstances` — is refused before anything is looked up, with the non-modelled `UnauthorizedException` (HTTP 400, fixed sentence "Unauthorized. Please check the request and access policy and try again.").

Expected: an instance that exists in the account (i.e. one the EC2 emulator holds in its instance store) can be associated with a user, returning an `InstanceUserSummary` in `ASSOCIATING`; only an instance that genuinely does not exist / is not the caller's is refused.

Actual: authorization reads this crate's OWN `account_instances` map, which is populated only by `seed_account_instances()` with the fixed ids `i-0f1e2d3c4b5a69788`, `i-0e2d3c4b5a6978899`, `i-0d3c4b5a697889900` (or by in-process test helpers). Nothing wires that map to `service-com-amazonaws-ec2-amazonec2`'s instance store, so a realistic cross-service workflow (ec2:RunInstances -> AssociateUser) fails, and so does replaying any recorded trace whose instance id differs from the three seeds. The same stale seed is read by the pre-validation gate in `src/service.rs` (`authorizes_user_association`), so the refusal happens even before constraint checking. DisassociateUser/ListInstances inherit the hole because no association can ever be created for such an instance.

## Evidence

Spec/semantics: `AssociateUser.InstanceId` is documented as "The ID of the EC2 instance that provides the user-based subscription" (Smithy model, AssociateUserRequest) — any EC2 instance of the account qualifies, not a fixed set. Handler: `src/ops/associate_user.rs` returns `Err(crate::scaffolded::Scaffolded::unauthorized())` when `state.authorizes_instance(&input.instance_id)` is false; `src/state.rs` `authorizes_instance` (~line 880) reads `account_instances`, whose only production writer is `seed_account_instances()` (src/state.rs ~line 640, three fixed ids) via `Default`. `src/service.rs` `authorizes_user_association` reads the same map before validation. Cross-service source exists and is implemented: `services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs` mints and stores instance records (`store.insert_instance(InstanceRecord { .. })`). Tests: `tests/all/resource_resolution_refusals.rs::the_instance_gate_fails_closed_before_any_other_lookup` and `tests/all/refusals.rs::associate_user_is_authorized_before_its_shape_is_validated` pin the fail-closed refusal for unseeded ids; no test covers an instance created through EC2.

## Steps to Reproduce

1. Start the emulator with both the EC2 and license-manager-user-subscriptions services mounted on the same engine.
2. Call ec2:RunInstances and note the minted instance id (e.g. i-0123456789abcdef0).
3. Call license-manager-user-subscriptions:RegisterIdentityProvider for a usable DirectoryId and Product=REMOTE_DESKTOP_SERVICES.
4. Call AssociateUser with Username=alice, that IdentityProvider and InstanceId=<the id from step 2>.
5. Expected: HTTP 200 with InstanceUserSummary{Status: ASSOCIATING, InstanceUserArn, InstanceId=<that id>}. Actual: HTTP 400 UnauthorizedException, and ListInstances never reports the instance.
6. Repeat with InstanceId=i-0f1e2d3c4b5a69788 (a hardcoded seed) and observe that the identical call succeeds — proving the gate keys on the seed, not on the account's EC2 fleet.

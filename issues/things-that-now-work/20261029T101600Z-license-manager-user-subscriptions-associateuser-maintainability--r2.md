Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 99c19747b30de4fba88ebdfb7af58fcb882c179f

# The "schedule the clock-owed transition" rule is re-implemented in each creating handler instead of in the one record writer

_Filed: 2026-10-29T10:16:00Z_
_Operation: AssociateUser | Class: maintainability_

> _Note: this issue was identified against the code base at commit `99c19747b30de4fba88ebdfb7af58fcb882c179f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` documents one rule for the pending-transition guard: every writer that stores a record in a non-terminal status must call `schedule_transition` with that record's due instant. The teardown writers obey it INSIDE the single state writer (`begin_instance_user_disassociation`, `begin_license_server_endpoint_deletion`, `begin_identity_provider_deregistration`, `begin_product_user_unsubscription` each end with `self.schedule_transition(due_at)`), so no caller can forget it. The creating writers do not: `insert_instance_user` and `insert_identity_provider` store the record and its indexes but schedule nothing, and the license-server-endpoint create writes its three stores inline in the handler — so the one rule is restated in three handlers (`associate_user.rs`, `register_identity_provider.rs`, `create_license_server_endpoint.rs`), each pairing a record's own due field with a separate `schedule_transition` call.

The copies can drift exactly the way the state module's own comment describes ("forgetting to would delay the transition until another record falls due"): a fourth creating path, or a reordering of either half, silently leaves a record in a non-terminal status that no sweep is scheduled for. Moving the `schedule_transition` into the `insert_*` writers (and giving the endpoint create the same writer the other kinds have) produces the identical pending guard, so every response and every existing test is unchanged.

## Evidence

The rule — src/state.rs:742-751: "The earliest instant at which a stored record owes the clock a status transition ... Every writer that stores a record in a non-terminal status calls `schedule_transition` with that record's due instant; forgetting to would delay the transition until another record falls due, never corrupt a record."

Obeyed inside the one writer (teardown side) — src/state.rs:1350 `self.schedule_transition(due_at);` (in `begin_instance_user_disassociation`), src/state.rs:1468 (in `begin_license_server_endpoint_deletion`), src/state.rs:2240 (in `begin_identity_provider_deregistration`), src/state.rs:2384 (in `begin_product_user_unsubscription`).

Restated per handler (creation side) — src/ops/associate_user.rs:303-305:
```
state.insert_instance_user(record.clone());
// The `ASSOCIATING` -> `ASSOCIATED` transition the clock now owes.
state.schedule_transition(record.associated_due_at);
```
src/ops/register_identity_provider.rs:320-323:
```
state.insert_identity_provider(key, record.clone(), now);
...
state.schedule_transition(record.registered_due_at);
```
src/ops/create_license_server_endpoint.rs:255-263 (record and all three indexes written inline, then) `state.schedule_transition(record.provisioned_due_at);` — while src/state.rs:1196 `pub fn insert_instance_user(&mut self, record: InstanceUserRecord)` and src/state.rs:1591 `pub fn insert_identity_provider(...)` schedule nothing.

Test coverage that must stay unchanged: tests/all/user_associations.rs (ASSOCIATING -> ASSOCIATED on a later read), tests/all/identity_provider_subscriptions.rs (REGISTERING -> REGISTERED), tests/all/license_server_endpoints.rs and tests/all/license_server_endpoint_deletion.rs (PROVISIONING -> PROVISIONED, DELETING -> DELETED), tests/all/instance_status_check_date.rs.

## Steps to Reproduce

1. Read src/state.rs:742-751 for the stated rule, then src/state.rs:1350/1468/2240/2384 where the teardown writers apply it themselves.
2. Read src/state.rs:1196 (`insert_instance_user`) and src/state.rs:1591 (`insert_identity_provider`): neither schedules the transition of the non-terminal status it stores.
3. Read src/ops/associate_user.rs:303-305, src/ops/register_identity_provider.rs:320-323 and src/ops/create_license_server_endpoint.rs:255-263: the same rule is spelled out a third time, once per creating handler, beside a record that already carries its due instant.

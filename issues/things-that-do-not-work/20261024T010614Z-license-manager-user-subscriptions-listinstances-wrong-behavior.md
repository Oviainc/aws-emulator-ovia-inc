Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# InstanceSummary.Products is stale: a registration added after an association is never reflected

_Filed: 2026-10-24T01:06:14Z_
_Operation: ListInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An instance's `Products` list is computed only inside `refresh_instance_view`, which is called by AssociateUser, DisassociateUser and (via `refresh_every_instance_view`) by `remove_identity_provider`. RegisterIdentityProvider does NOT refresh the view, so the products recorded on an instance are frozen at whatever the directory was registered for at association time.

Expected (the state model's own documented contract, and what a client reads `Products` as): "the products are the ones the naming associations' DIRECTORIES are registered for ... so a product stops being reported as soon as the last association or registration behind it is gone" — i.e. `Products` is a function of current state, recomputed on every read/write that can change it.

Actual: associate a user through directory D (registered for product A), then register D for product B. `ListInstances` still reports `Products: [A]`, although the service's own rule now says the instance provides A and B. The stale value is only corrected by an unrelated later write: deregistering A then recomputes the view and the list jumps straight from `[A]` to `[B]`, never showing `[A, B]`. The missing refresh in the register path (or recomputing the view at read time in ListInstances) is the gap.

## Evidence

Spec: `InstanceSummary$Products` is `@required` and documented as "A list of provided user-based subscription products" (model). Code: `src/state.rs::refresh_instance_view` (lines 789-823) and its doc comment (lines 778-788) define Products as a function of the current associations plus registrations; `refresh_every_instance_view` (line 827) is called from `remove_identity_provider` (line 983) but there is NO call in `src/ops/register_identity_provider.rs` (which only inserts the record, indexes it and schedules the status transition, lines 246-257), and `src/ops/list_instances.rs` (lines 152-184) reports `state.instances_in_order()` verbatim without recomputing. Tests: `resource_resolution_refusals.rs::the_instance_view_is_recomputed_from_the_associations_that_remain` registers every provider BEFORE associating, so the register-after-associate ordering is untested.

## Steps to Reproduce

1. RegisterIdentityProvider for directory D with Product=VISUAL_STUDIO_ENTERPRISE.
2. AssociateUser with that IdentityProvider and an instance the service authorizes.
3. ListInstances — the instance reports Products: [VISUAL_STUDIO_ENTERPRISE].
4. RegisterIdentityProvider for the SAME directory D with Product=OFFICE_STANDARD.
5. ListInstances again — Products is still [VISUAL_STUDIO_ENTERPRISE]; expected both products, since the instance's association now resolves through a directory registered for both.
6. DeregisterIdentityProvider for (D, VISUAL_STUDIO_ENTERPRISE) and list again — Products becomes [OFFICE_STANDARD], confirming the value was merely stale.

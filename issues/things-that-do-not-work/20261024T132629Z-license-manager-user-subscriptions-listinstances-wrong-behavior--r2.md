Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Populate InstanceSummary.LastStatusCheckDate for instances in the service's view

_Filed: 2026-10-24T13:26:29Z_
_Operation: ListInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InstanceSummary.LastStatusCheckDate` is never reported by ListInstances: the field is plumbed through the record but nothing in the request path ever writes it.

Expected: an instance that this service tracks (because a user is associated with it) reports the date of its last status check, which is how a client tells a freshly-checked instance from a stale one.

Actual: `InstanceRecord.last_status_check_at` is only ever read — `apply_instance_view` copies whatever is already stored, and every production writer leaves it `None` (the only way it becomes `Some` is an in-process test helper). So every `ListInstances` response omits `LastStatusCheckDate` for every instance, for the whole lifetime of an association.

## Evidence

Spec: `InstanceSummary$LastStatusCheckDate` is a documented output member of ListInstances (crate OPERATIONS.md / Smithy `InstanceSummary`), reported by the real service for instances providing user-based subscriptions. Code: `src/state.rs` `InstanceRecord.last_status_check_at` (line ~446) is read at `apply_instance_view` (lines ~1436-1448) and mapped at `instance_summary` (line ~1842), but no handler or state writer ever assigns a `Some(..)` value (grep for `last_status_check_at` over `src/` returns only the declaration, the copy-forward and the read). Tests: `tests/all/list_instances.rs` line ~252 accepts "the stored instant, or absent", and `tests/all/resource_resolution_refusals.rs` line ~1338 asserts it is absent — no test pins the real-service behavior.

## Steps to Reproduce

1. RegisterIdentityProvider for a usable DirectoryId and Product, then AssociateUser for an authorized InstanceId so the instance enters the service's view.
2. Call ListInstances.
3. Expected: the InstanceSummary for that instance carries LastStatusCheckDate. Actual: the member is always absent, now and after the ASSOCIATING -> ASSOCIATED transition has materialised.

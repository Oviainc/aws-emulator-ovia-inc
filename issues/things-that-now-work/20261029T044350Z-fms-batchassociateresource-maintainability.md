Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3f59b13755f238905b4b26aae43e5ec6021c53f3

# The "re-resolve the resource set under the held lock and read its ResourceTypeList" step is written twice in the two batch handlers, and the two copies have already drifted

_Filed: 2026-10-29T04:43:50Z_
_Operation: BatchAssociateResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3f59b13755f238905b4b26aae43e5ec6021c53f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchAssociateResource` and `BatchDisassociateResource` both have to re-resolve the record for the id `crate::state::resolve_resource_set_id` handed back, under the lock they already hold, and read its `ResourceTypeList` before classifying the batch. Each handler writes that step out itself, and the two copies disagree about the missing-record case:

* `BatchDisassociateResource` reads the record through the operation's not-found channel, and its comment states the rule explicitly: "it is read through the same not-found channel rather than a default, so no future change can classify a batch against an empty `ResourceTypeList`";
* `BatchAssociateResource` reads the identical field with `.unwrap_or_default()`, which is exactly the default the sibling's comment forbids — and then writes the membership through `if let Some(record) = store.resource_sets.get_mut(&id)`, which silently writes nothing on the same miss.

Today the miss is unreachable (`resolve_resource_set_id`, state.rs:2672-2684, only ever returns a key that is in `resource_sets`), so no response and no test changes if `BatchAssociateResource` takes the same not-found channel — or if the shared step moves into one helper beside `resolve_resource_set_id`, the way every other rule the two batch handlers share already lives in `crate::state` (`MAX_RESOURCE_SET_ITEMS_PER_CALL`, `resource_set_item_failure_reason`, `ResourceSetRecord::set_resources`, `resource_set_mutation_refusal`). Left as it is, the drifted copy is the one that would turn a lookup miss into a 200 whose `FailedItems` blames every item with `NOT_VALID_RESOURCE_TYPE` (an empty `resource_types` makes `resource_set_item_failure_reason` reject everything) while the membership is left untouched.

## Evidence

The copy that states the rule — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/batch_disassociate_resource.rs:261-275:
```
        // The set's own `ResourceTypeList` and its current membership, both read
        // ONCE before anything is written. The identifier resolved above, so this
        // lookup finds the record under the same held lock; it is read through
        // the same not-found channel rather than a default, so no future change
        // can classify a batch against an empty `ResourceTypeList`.
        let Some(record) = store.resource_sets.get(&id) else {
            return Err(
                BatchDisassociateResourceErrorModeled::ResourceNotFoundException(
                    crate::state::resource_not_found(),
                )
                .into(),
            );
        };
        let resource_types = record.resource_set.resource_type_list.clone();
        let held = record.resources.clone();
```

The copy that breaks it — src/ops/batch_associate_resource.rs:274-280:
```
        // Classified against the resource set's own `ResourceTypeList` and the
        // store, all read ONCE before the membership is written.
        let resource_types = store
            .resource_sets
            .get(&id)
            .map(|record| record.resource_set.resource_type_list.clone())
            .unwrap_or_default();
```
and the same handler's membership write, src/ops/batch_associate_resource.rs:330-334:
```
        if let Some(record) = store.resource_sets.get_mut(&id) {
            let mut membership = record.resources.clone();
            membership.extend(associated);
            record.set_resources(membership, now);
        }
```

Why the miss is unreachable today, so a fix changes nothing observable — src/state.rs:2672-2684:
```
pub fn resolve_resource_set_id(
    store: &ComAmazonawsFmsAwsfms20180101State,
    identifier: &str,
) -> Option<String> {
    if store.resource_sets.contains_key(identifier) {
        return Some(identifier.to_string());
    }
    store
        .resource_sets
        .values()
        .find(|record| record.arn == identifier)
        .map(|record| record.id.clone())
}
```

The crate's own precedent for single-homing what the two batch handlers share — src/state.rs:2659-2661 ("Shared by both batch operations, so neither of them can admit a batch the other refuses.", `MAX_RESOURCE_SET_ITEMS_PER_CALL`) and src/state.rs:2814-2817 ("It is one shared classifier so the operations that associate and disassociate a resource's membership cannot disagree about which items a set can hold.", `resource_set_item_failure_reason`).

Tests that cover both handlers and must stay green unchanged: tests/all/resource_set_disassociation.rs, tests/all/resource_set_disassociation_membership_invariants.rs, tests/all/resource_set_membership_last_update_time.rs (rows at lines 98-144 drive both membership writers), tests/all/resource_set_mutation_admin_scope.rs, tests/all/resource_set_resources.rs, tests/all/associable_discovered_resources.rs and tests/all/arn_form_single_rule.rs.

## Steps to Reproduce

1. Open src/ops/batch_disassociate_resource.rs and read lines 261-275: the resource set is re-resolved under the held lock through `ResourceNotFoundException`, with a comment stating that reading it "through the same not-found channel rather than a default" is what stops a batch being classified against an empty `ResourceTypeList`.
2. Open src/ops/batch_associate_resource.rs and read lines 274-280: the same field is read with `.unwrap_or_default()`, the default the sibling comment forbids; lines 330-334 then write the membership behind `if let Some(record) = ...`, which drops the write on the same miss.
3. Confirm from src/state.rs:2672-2684 that `resolve_resource_set_id` only ever yields a key present in `resource_sets`, so the two copies agree on every reachable input today and have drifted only on the unreachable one — a fix that makes `BatchAssociateResource` take the same not-found channel (or that moves the step into one shared helper) leaves every response and every test above unchanged.

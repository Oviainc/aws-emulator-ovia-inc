Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 74cfda6b1b1701e11661fc7b5d0b64a8c57e063b

# BatchDisassociateResource removes resource-set members without moving the set's LastUpdateTime

_Filed: 2026-10-27T19:10:25Z_
_Operation: BatchDisassociateResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `74cfda6b1b1701e11661fc7b5d0b64a8c57e063b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same defect as on the association side, in the sibling handler: `BatchDisassociateResource` removes URIs from `ResourceSetRecord::resources` and never touches `ResourceSetRecord::updated_at`, which is the only source of the modelled `ResourceSet$LastUpdateTime` ("The last time that the resource set was changed") that `GetResourceSet`/`ListResourceSets` report. A successful disassociation therefore changes the set — `ListResourceSetResources` no longer reports the removed member — while every reader keeps reporting the timestamp of the last document write.

This is filed separately from the `BatchAssociateResource` issue because it is a separate handler with its own write site; fixing one does not fix the other.

## Evidence

Smithy (`smithy_query(query="doc", shape="com.amazonaws.fms#ResourceSet", member="LastUpdateTime")`): `"documentation": "<p>The last time that the resource set was changed.</p>"`.

The write that changes the set and skips the timestamp — `src/ops/batch_disassociate_resource.rs:301-305`:
```rust
        if let Some(record) = store.resource_sets.get_mut(&id) {
            for uri in &disassociated {
                record.resources.remove(uri);
            }
        }
```

The sibling writer that does bump it, and whose comment names THIS operation as the one that changes the membership — `src/ops/put_resource_set.rs:309-315`:
```rust
                    // The resources associated with the set are its own
                    // membership, changed by BatchAssociateResource /
                    // BatchDisassociateResource and not by rewriting the
                    // document.
                    resources: existing.resources.clone(),
                    created_at: existing.created_at,
                    updated_at: now,
```

What reaches the wire — `src/state.rs:440`:
```rust
        resource_set.last_update_time = Some(::aws_smithy_types::DateTime::from(self.updated_at));
```

Test coverage: `tests/all/resource_set_disassociation.rs` asserts only membership and `FailedItems` (e.g. lines 500-509, 536-539) and never reads `LastUpdateTime`; `tests/all/resource_set_mutation_admin_scope.rs:251-305` asserts only the membership after a successful batch.

## Steps to Reproduce

1. POST AssociateAdminAccount with {"AdminAccount": "<emulator account id>"}.
2. POST PutResourceSet with {"ResourceSet": {"Name": "set-b", "ResourceTypeList": ["AWS::EC2::Instance"]}}; note the Id.
3. POST BatchAssociateResource with a valid instance ARN so the set holds one member.
4. POST GetResourceSet and record ResourceSet.LastUpdateTime (t0); wait more than one second.
5. POST BatchDisassociateResource with {"ResourceSetIdentifier": "<Id>", "Items": ["<instance ARN>"]}. Observe 200 and that ListResourceSetResources now reports an empty membership.
6. POST GetResourceSet again. Observed: LastUpdateTime is still t0. Expected: the instant the membership changed.

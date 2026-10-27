Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 74cfda6b1b1701e11661fc7b5d0b64a8c57e063b

# BatchAssociateResource changes a resource set's membership without moving its LastUpdateTime

_Filed: 2026-10-27T19:10:25Z_
_Operation: BatchAssociateResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `74cfda6b1b1701e11661fc7b5d0b64a8c57e063b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchAssociateResource` adds URIs to `ResourceSetRecord::resources` but never touches `ResourceSetRecord::updated_at`, which is the ONLY source of the modelled `ResourceSet$LastUpdateTime` that `GetResourceSet`, `PutResourceSet` and the `ListResourceSets` summary report. The model documents that member as "The last time that the resource set was changed", and this crate itself treats a membership change as a change TO THE SET: `PutResourceSet`'s own comment says the membership is "changed by BatchAssociateResource / BatchDisassociateResource and not by rewriting the document", and the sibling writer (`PutResourceSet`) sets `updated_at: now` on every successful write.

Result: after a successful association the set demonstrably changed — `ListResourceSetResources` reports the new member — but every reader keeps reporting the `LastUpdateTime` of the last *document* write, so a client polling `LastUpdateTime` (or diffing a `ListResourceSets` page) cannot see that the set changed at all. A set created and then only ever mutated through the batch operations reports the creation instant forever.

## Evidence

Smithy (`smithy_query(query="doc", shape="com.amazonaws.fms#ResourceSet", member="LastUpdateTime")`): `"documentation": "<p>The last time that the resource set was changed.</p>"`.

The write that changes the set and skips the timestamp — `src/ops/batch_associate_resource.rs:319-321`:
```rust
        if let Some(record) = store.resource_sets.get_mut(&id) {
            record.resources.extend(associated);
        }
```
(no `record.updated_at = …` anywhere in the handler).

The sibling writer, which does both — `src/ops/put_resource_set.rs:309-315`:
```rust
                    // The resources associated with the set are its own
                    // membership, changed by BatchAssociateResource /
                    // BatchDisassociateResource and not by rewriting the
                    // document.
                    resources: existing.resources.clone(),
                    created_at: existing.created_at,
                    updated_at: now,
```

The projection that proves `updated_at` is what reaches the wire — `src/state.rs:440`:
```rust
        resource_set.last_update_time = Some(::aws_smithy_types::DateTime::from(self.updated_at));
```

Test coverage: nothing pins the current behaviour. `tests/all/resource_set_listing.rs:337-389` only asserts that *listing* leaves `update_token`/`updated_at` alone; `tests/all/resource_set_status_derivation.rs:344-353` only asserts a *scope change* leaves `LastUpdateTime` alone; `tests/all/resource_set_mutation_admin_scope.rs:418-428` only asserts a *refused* mutation leaves the document alone. `tests/all/resource_set_resources.rs` and `tests/all/resource_set_disassociation.rs` never read `LastUpdateTime`.

## Steps to Reproduce

1. POST AssociateAdminAccount with {"AdminAccount": "<emulator account id>"}.
2. POST PutResourceSet with {"ResourceSet": {"Name": "set-a", "ResourceTypeList": ["AWS::EC2::Instance"]}} and record ResourceSet.Id and ResourceSet.LastUpdateTime (call it t0).
3. Create an EC2 instance in the same account/Region through the EC2 emulator so its ARN is discoverable, and wait more than one second (LastUpdateTime is reported in whole seconds).
4. POST BatchAssociateResource with {"ResourceSetIdentifier": "<Id>", "Items": ["<instance ARN>"]}. Observe 200 with "FailedItems": [].
5. POST ListResourceSetResources with {"Identifier": "<Id>"}: the instance IS now a member, so the set changed.
6. POST GetResourceSet with {"Identifier": "<Id>"}. Observed: ResourceSet.LastUpdateTime is still t0. Expected: the instant the membership changed, per "The last time that the resource set was changed".

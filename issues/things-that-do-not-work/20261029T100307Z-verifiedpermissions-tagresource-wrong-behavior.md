Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1d0d0c28dcee365463f48da791546814f4205587

# Tag writes rewrite the policy-store record without moving its lastUpdatedDate, the stamp PutSchema and UpdatePolicyStore apply for the same kind of write

_Filed: 2026-10-29T10:03:07Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1d0d0c28dcee365463f48da791546814f4205587`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` and `UntagResource` mutate a member of the persisted `PolicyStore` record (`store.tags`) and leave `store.last_updated_date` untouched, so `GetPolicyStore` keeps reporting the stamp from before the tag change. The two other handlers in this crate that write a member of the same record both move the stamp, and `PutSchema` states the rule in a comment right above the assignment: writing the store's own `schema` member means "the store itself changed, so its own `lastUpdatedDate` moves too". Writing the store's own `tags` member is the same kind of write, so the record is now observably stale for one class of writes and fresh for the others.

## Evidence

The rule, stated and applied — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/put_schema.rs:213-218:
```
        // The store itself changed, so its own `lastUpdatedDate` moves too. What
        // the store already holds is NOT re-validated: ...
        store.last_updated_date = now;
```
and services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/update_policy_store.rs:186:
```
        store.last_updated_date = crate::state::now();
```
The same kind of write, without the stamp — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/tag_resource.rs:166-185:
```
        let mut merged = store.tags.clone();
        merged.extend(...);
        ...
        store.tags = merged;

        crate::ops::audit(ctx, "TagResource", &store_id, "tagged");
```
and services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/untag_resource.rs:145-149:
```
        for key in &input.tag_keys {
            store.tags.remove(key);
        }

        crate::ops::audit(ctx, "UntagResource", &store_id, "untagged");
```
The stamp is read straight off that record by the reader — src/ops/get_policy_store.rs:124: `last_updated_date: store.last_updated_date,`.

Test coverage: tests/all/policy_store_settings.rs:13-18 states the crate's own expectation for a settings write ("`UpdatePolicyStore` replaces the store's mutable settings in that record and moves `lastUpdatedDate`"); tests/all/tagging.rs asserts only the tag set (lines 179-194) and pins nothing about `lastUpdatedDate`, so no test justifies the divergence.

## Steps to Reproduce

1. Boot the emulator and point a Verified Permissions client at its data listener.
2. store = CreatePolicyStore(validationSettings={"mode":"OFF"}); record arn and lastUpdatedDate L0.
3. Wait a moment, then TagResource(resourceArn=arn, tags={"team":"platform"}) -> 200.
4. GetPolicyStore(policyStoreId=store, tags=true) -> tags report the new pair, but lastUpdatedDate is still L0.
5. For contrast, PutSchema(policyStoreId=store, definition={"cedarJson":"{}"}) then GetPolicyStore -> lastUpdatedDate has advanced, as it also does after UpdatePolicyStore.
6. UntagResource(resourceArn=arn, tagKeys=["team"]) -> 200; GetPolicyStore again -> the tag is gone and lastUpdatedDate has not moved for that write either.

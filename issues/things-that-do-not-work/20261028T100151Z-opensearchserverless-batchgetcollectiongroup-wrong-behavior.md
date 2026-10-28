Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# BatchGetCollectionGroup reads collections without settling due transitions, so numberOfCollections counts already-purged collections

_Filed: 2026-10-28T10:01:50Z_
_Operation: BatchGetCollectionGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchGetCollectionGroup` derives `numberOfCollections` from the stored collections (`store.collections_in_group(&record.name)`), but it is the ONLY handler in this crate that touches `collections` without first calling `store.settle_due_transitions()`. That helper is what applies the purge of a collection whose deletion was accepted and whose purge instant has already fallen due (it removes the record from `self.collections`). Because this handler skips it, a group whose only collection was deleted and whose purge is already due is still reported with `numberOfCollections: 1`, while `DeleteCollectionGroup` (which does settle) treats the very same group as EMPTY and deletes it, and `ListCollections`/`BatchGetCollection` (which also settle) no longer report that collection at all. The count the group read reports therefore contradicts every other read of the same state.

## Evidence

The rule, stated by the state helper itself: "Apply every collection transition the clock has already made due: a status a collection settles into, and the PURGE of one whose deletion was accepted. Call it before reading or writing a collection, so no caller observes a status the service has already moved on from." — src/state.rs:1990-1995, with the purge loop at src/state.rs:2002-2010 (`.filter(|record| record.is_purge_due(now))` … `self.remove_collection(&id)`).

The sibling that reads the SAME derived count does settle, and says why: "// A collection whose create is still settling `CREATING -> ACTIVE` is a collection\n        // this group holds, so the emptiness check below reads the collections as of now\n        // rather than as of the last call that touched them.\n        store.settle_due_transitions();" — src/ops/delete_collection_group.rs:160-163, then `let held = store.collections_in_group(&group_name);` at src/ops/delete_collection_group.rs:188.

This handler takes a non-mut guard and never settles: "let store = state.lock();" — src/ops/batch_get_collection_group.rs:173 — and then counts: "let collections = store.collections_in_group(&record.name);" — src/ops/batch_get_collection_group.rs:182. `BatchGetCollection` (src/ops/batch_get_collection.rs:171), `ListCollections` (src/ops/list_collections.rs:178), `GetIndex` (src/ops/get_index.rs:156), `ListTagsForResource` (src/ops/list_tags_for_resource.rs:153), `TagResource` (src/ops/tag_resource.rs:160) and `UntagResource` (src/ops/untag_resource.rs:155) all call `store.settle_due_transitions()` first.

Test coverage: tests/all/collection_group_delete.rs:430-435 asserts `numberOfCollections` only while the collections are still live, and tests/all/collection_group_delete.rs:464-468 waits for the purge to fall due and then calls `DeleteCollectionGroup` (which settles) — no test reads the group through `BatchGetCollectionGroup` after the purge is due, so nothing pins the stale count.

## Steps to Reproduce

1. CreateCollectionGroup(name="g", standbyReplicas="ENABLED") and note the returned id.
2. CreateCollection(name="c", collectionGroupName="g", encryptionConfig={aWSOwnedKey:true}); wait for it to settle to ACTIVE.
3. DeleteCollection(id=<collection id>) and wait longer than the purge window (SETTLE_AFTER) WITHOUT calling any other operation.
4. BatchGetCollectionGroup(ids=[<group id>]) → collectionGroupDetails[0].numberOfCollections is 1, because the purge the clock already made due was never applied.
5. Now call DeleteCollectionGroup(id=<group id>) → it settles, sees the group as empty and succeeds, contradicting the count just reported.

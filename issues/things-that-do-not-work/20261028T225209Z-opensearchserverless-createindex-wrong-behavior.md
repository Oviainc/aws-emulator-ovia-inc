Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f08062a3928279f71031748dc453b175f9086c5b

# The collection-status conflict check runs ahead of CreateIndex's own idempotent short-circuit, so a retry of an already-accepted create is refused 409

_Filed: 2026-10-28T22:52:09Z_
_Operation: CreateIndex | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f08062a3928279f71031748dc453b175f9086c5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateIndex` is `@idempotent`, and the handler implements that by answering 200 for a repeat of the SAME create (same `indexName`, same `indexSchema`) rather than conflicting with it. But the "collection must be ACTIVE" ConflictException check is placed BEFORE that short-circuit, so whenever the addressed collection is momentarily not `ACTIVE` the retry is refused with 409 — even though the index it asks for already exists with exactly the schema it carries, and the first call already returned 200.

The handler itself states the rule the ordering breaks twice: once on the short-circuit ("a repeat of the SAME create answers the index it already made rather than conflicting with it") and again on the per-collection index cap, which was deliberately placed AFTER the short-circuit for precisely this reason ("so a retry of an accepted create is never refused by it"). The status check was not given the same treatment, so the one guard the author explicitly ordered around the retry path is bypassed by a guard sitting in front of it.

A collection reaches a non-`ACTIVE` status on its own in this crate: `UpdateCollection` moves it to `UPDATING` (`CollectionRecord::begin_update` -> `begin_transition(COLLECTION_UPDATING, COLLECTION_ACTIVE)`), and it settles back only once the clock makes that due. So an ordinary client that retries `CreateIndex` while any unrelated `UpdateCollection` is in flight turns a successful, idempotent create into a 409.

## Evidence

The status check is FIRST — `services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_index.rs:202-216`:
```
        // An index lives in the collection's data plane, which exists once the collection
        // is "ready to use" (`CollectionStatus$ACTIVE`'s own documentation) — so a
        // collection still being created, updated or deleted cannot take one yet. That is
        // the state conflict `ConflictException` describes.
        if collection.status != crate::state::COLLECTION_ACTIVE {
            return Err(CreateIndexErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "Collection with ID {} is in status {} and cannot take an index; it must be {}",
```

The idempotent short-circuit comes AFTER it — `create_index.rs:218-225`:
```
        if let Some(existing) = collection.indexes.get(&input.index_name) {
            // `CreateIndex` is `@idempotent`, so a repeat of the SAME create answers the
            // index it already made rather than conflicting with it; a different schema
            // under a name already taken is a genuinely different request, which is the
            // conflict `ConflictException` describes.
            if existing.index_schema == input.index_schema {
                return Ok(CreateIndexOutput {});
            }
```

And the crate states the ordering rule it broke here — `create_index.rs:235-240`:
```
        // The per-collection index store is BOUNDED: `CreateIndex` adds one caller-named
        // entry per request to a collection that may live indefinitely, and nothing
        // reclaims an index until its collection is purged, so the cap is what keeps that
        // store finite. Checked AFTER the idempotent short-circuit above, so a retry of an
        // accepted create is never refused by it.
        if collection.indexes.len() >= crate::state::MAX_INDEXES_PER_COLLECTION {
```

The non-`ACTIVE` window is produced by a sibling handler in the same crate — `src/ops/update_collection.rs:255` (`record.begin_update(updated_at);`) and `src/state.rs:707-710`:
```
    pub fn begin_update(&mut self, updated_at: i64) {
        self.begin_transition(COLLECTION_UPDATING, COLLECTION_ACTIVE);
        self.last_modified_date = updated_at;
    }
```

Test coverage: the crate's replay test exercises the short-circuit only on an `ACTIVE` collection — `tests/all/encryption_indexes_and_tagging.rs:600-605` asserts `CreateIndex(..., replayed)` answers 200, and the refusal table at `tests/all/encryption_indexes_and_tagging.rs:642-647` only covers "a collection still CREATING" with NO index of that name yet. No test pins a retry of an already-stored index while the collection is `UPDATING`, so nothing in the crate justifies the ordering.

## Steps to Reproduce

1. CreateCollection {"name":"idem-index","encryptionConfig":{"aWSOwnedKey":true}} and poll BatchGetCollection until the collection reports status ACTIVE.
2. CreateIndex {"id":"<id>","indexName":"logs","indexSchema":{"mappings":{"properties":{"ts":{"type":"date"}}}}} -> 200, the index is stored.
3. UpdateCollection {"id":"<id>","description":"touched"} -> 200; the collection is now reported in status UPDATING (begin_update).
4. Retry the IDENTICAL CreateIndex request from step 2 (same id, same indexName, same indexSchema).
5. Expected (the handler's own stated rule): 200 — "a repeat of the SAME create answers the index it already made rather than conflicting with it", and "a retry of an accepted create is never refused".
6. Actual: 409 ConflictException "Collection with ID <id> is in status UPDATING and cannot take an index; it must be ACTIVE", because create_index.rs:206 runs before create_index.rs:218.

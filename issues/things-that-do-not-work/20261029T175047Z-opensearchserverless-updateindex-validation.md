Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# UpdateIndex writes an index schema into a collection that is not ACTIVE, the state its sibling writers all refuse

_Filed: 2026-10-29T17:50:47Z_
_Operation: UpdateIndex | Class: validation_

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateIndex` resolves the collection, resolves the index, and writes `index.index_schema` with no check at all on the collection's `CollectionStatus`. Every other write path in this crate that reaches a `CollectionRecord` refuses one that is not usable:

* `CreateIndex` — the sibling that puts an index into the same `collection.indexes` map — refuses any collection whose status is not `ACTIVE`, with the stated reason that "An index lives in the collection's data plane, which exists once the collection is 'ready to use' ... so a collection still being created, updated or deleted cannot take one yet."
* `TagResource` and `UntagResource` refuse a collection whose deletion has been accepted (`!record.is_deleting()`), with the stated reason that "A tag belongs to a LIVE resource: a collection whose deletion has been accepted is on its way out".
* `UpdateCollection` refuses any collection that is not `ACTIVE`.

The reachable consequence is a write that is silently thrown away: `DeleteCollection` moves the collection to `DELETING` and sets `purge_at`, which `crate::state` documents as "the instant from which it stops being a collection any read reports at all". `UpdateIndex` against that collection answers 200 as if the schema had been stored, and the next call's `settle_due_transitions()` purges the collection together with the index and the schema just written. A caller that got 200 can never read back what it wrote — the handler turned a refusal into a success.

`UpdateIndex` declares no `ConflictException`, but that is not a reason to skip the check: this crate already has the pattern for exactly that situation — `UpdateCollection`'s own comment says that because the operation "models no not-found error ... an id that names no collection is refused as invalid input rather than with an error this operation does not declare", i.e. the modelled `ValidationException`. The same mapping is available here.

## Evidence

The skipped check — `services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/update_index.rs:183-217` goes from the lock straight to the write with no status branch:
```
183:        let mut store = state.lock();
184:        store.settle_due_transitions();
185:
186:        let Some(collection) = store.collections.get_mut(&input.id) else {
...
198:        let Some(index) = collection.indexes.get_mut(&input.index_name) else {
...
215:        if let Some(schema) = input.index_schema {
216:            index.index_schema = Some(schema);
217:        }
```

The sibling that DOES check, writing to the same map — `src/ops/create_index.rs:197-211`:
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

The other sibling writers that refuse a dying collection — `src/ops/untag_resource.rs:175-180` (identically `src/ops/tag_resource.rs:178-183`):
```
        // A tag belongs to a LIVE resource: a collection whose deletion has been accepted
        // is on its way out and its tags are no longer addressable, so it answers the same
        // not-found refusal as one that was never there.
        let taggable = store
            .collection_by_arn(&input.resource_arn)
            .is_some_and(|record| !record.is_deleting());
```

Why the write is LOST — `src/state.rs:688-691`:
```
    /// When this collection's accepted deletion is due to be PURGED — the instant from
    /// which it stops being a collection any read reports at all. Set by
    /// [`Self::begin_deletion`], which is also what moves it to `DELETING`.
    pub purge_at: Option<Instant>,
```
and `src/state.rs:2541-2551`, where `settle_due_transitions` calls `self.remove_collection(&id)` for every record whose purge is due — taking `indexes` with it (`src/state.rs:639-643`: "a purged collection takes its indexes with it").

The in-crate pattern for refusing a state the operation models no error for — `src/ops/update_collection.rs:212-222`:
```
        // `UpdateCollection` models no not-found error (its errors are `ConflictException`,
        // `InternalServerException` and `ValidationException`), so an id that names no
        // collection is refused as invalid input rather than with an error this operation
        // does not declare.
```

Test coverage: `tests/all/index_schema_update.rs` exercises `UpdateIndex` only against collections it first drives to `ACTIVE` (`an_update_replaces_the_stored_schema_and_the_read_reports_it`, `successive_updates_each_replace_what_the_previous_one_wrote`, `an_update_that_addresses_no_index_is_refused_and_writes_nothing`); no test asserts that an update against a `DELETING` or `UPDATING` collection succeeds, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateSecurityPolicy(type="encryption", name="enc", policy='{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}') -> 200.
2. CreateCollection(name="mycoll") -> 200; poll BatchGetCollection until status is ACTIVE.
3. CreateIndex(id=<collection id>, indexName="logs", indexSchema={"a":1}) -> 200; GetIndex reports {"a":1}.
4. DeleteCollection(id=<collection id>) -> 200 with deleteCollectionDetail.status == "DELETING" (src/ops/delete_collection.rs:226).
5. Immediately call UpdateIndex(id=<collection id>, indexName="logs", indexSchema={"a":2}) -> 200, although CreateIndex on the same collection in the same status answers 409 ConflictException and TagResource/UntagResource on it answer 404 ResourceNotFoundException.
6. Wait for the purge to fall due and call GetIndex(id=<collection id>, indexName="logs") -> 404: the schema the 200 claimed to have stored is gone, and so is every trace of the accepted write.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# CreateCollection stores a collectionGroupName without checking the group exists, so a collection reports membership in a group no read can find

_Filed: 2026-10-28T16:00:15Z_
_Operation: CreateCollection | Class: validation_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCollection` copies `collectionGroupName` straight onto the stored `CollectionRecord` without ever probing the collection-group store, even though the crate already has the helper for exactly that probe (`collection_group_by_name`) and already uses the sibling helper (`collection_by_name`) two statements earlier to refuse a duplicate collection name.

The result is state the rest of this crate contradicts:

* `BatchGetCollection` reports `collectionGroupName: "ghost"` on the new collection — a member the model documents as "The name of the collection group that contains this collection" — while `BatchGetCollectionGroup(names=["ghost"])` answers that same name with the modelled `NOT_FOUND` `collectionGroupErrorDetails` entry. One read says the collection is contained in a group; the other says the group does not exist.
* `ListCollections(collectionFilters.collectionGroupName="ghost")` returns the collection, i.e. the listing filters on a group that is not a resource of the account.
* `CollectionGroupRecord`'s capacity limits, `standbyReplicas` and `generation` — everything a group is for — never apply to the collection, because there is no group record behind the name.

The containment relation is NOT something this crate is indifferent to: `DeleteCollectionGroup` goes out of its way to preserve it, refusing with `ConflictException` while any collection still names the group, and `BatchGetCollectionGroup` derives `numberOfCollections` by counting collections by that name. Only the create path leaves the relation unchecked, so the one operation that establishes it is the one operation that can establish it against nothing. `CreateCollection` models `ValidationException` ("Thrown when the HTTP request contains invalid input or is missing required input"), which is the refusal the handler already uses for the missing-encryption-setting case in the same function.

## Evidence

Smithy, `CollectionDetail$collectionGroupName` (and the identically documented `CreateCollectionDetail$collectionGroupName` / `CollectionSummary$collectionGroupName`): "The name of the collection group that contains this collection." — so the member this handler fills in asserts containment.

The handler probes its OWN name index but not the group's:

    // A collection name identifies one collection in the account, so a name already
    // taken is the conflict `ConflictException` describes [...]
    if store.collection_by_name(&input.name).is_some() {

— src/ops/create_collection.rs:283, and then stores the group name unchecked:

    collection_group_name: input.collection_group_name,

— src/ops/create_collection.rs:347. No call to `collection_group_by_name` appears anywhere in the file.

The helper the path skips already exists and is used by two siblings:

    /// The collection group `name` resolves to, if one is stored under it.
    pub fn collection_group_by_name(&self, name: &str) -> Option<&CollectionGroupRecord> {

— src/state.rs:2343; called at src/ops/create_collection_group.rs:250 (`if store.collection_group_by_name(&input.name).is_some()`) and at src/ops/batch_get_collection_group.rs:192 (`match store.collection_group_by_name(&name)`).

The other half of the same relation IS enforced, with its own quoted documentation:

    // From this operation's own documentation: "You can only delete empty collection
    // groups that contain no collections." A group that still holds collections is
    // refused with `ConflictException` [...]
    let group_name = record.name.clone();
    let held = store.collections_in_group(&group_name);
    if held > 0 {

— src/ops/delete_collection_group.rs:183-189.

Test coverage: every test that creates a collection inside a group creates the group FIRST — tests/all/collection_group_lifecycle.rs:297-328 (`create_group(...)` for "empty-group"/"single-group"/"double-group" before the `CreateCollection` calls at line 319), tests/all/collection_group_delete.rs:408, tests/all/client_error_status.rs:130-149 ("stored-group" created at line 133, used at line 145). The only tests that pass an unknown group name assert the `@pattern` refusal of a malformed name, not acceptance of an absent group (tests/all/encryption_indexes_and_tagging.rs:444-445, tests/all/reported_pattern.rs:137-138, tests/all/client_error_status.rs:427-428). Nothing pins the current acceptance.

## Steps to Reproduce

1. Boot the emulator and point an aoss client at it.
2. Do NOT create any collection group.
3. CreateCollection(name="orphan-collection", collectionGroupName="ghost-group", encryptionConfig={"aWSOwnedKey": true}) → 200, and `createCollectionDetail.collectionGroupName` is "ghost-group".
4. BatchGetCollection(ids=[<new id>]) → `collectionDetails[0].collectionGroupName` is "ghost-group", i.e. the collection reports the group that "contains this collection".
5. BatchGetCollectionGroup(names=["ghost-group"]) → `collectionGroupErrorDetails[0]` with errorCode NOT_FOUND: the containing group does not exist.
6. ListCollections(collectionFilters={"collectionGroupName": "ghost-group"}) → the orphan collection is returned, so the listing filters on a group that is not a resource of this account.

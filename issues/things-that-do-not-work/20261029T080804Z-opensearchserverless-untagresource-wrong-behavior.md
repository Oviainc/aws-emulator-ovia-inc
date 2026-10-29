Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4c62cef672c36ef491d018c691144f86baa80e9d

# Tag writes rewrite the collection record without stamping lastModifiedDate, the stamp every other write in this crate applies

_Filed: 2026-10-29T08:08:04Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4c62cef672c36ef491d018c691144f86baa80e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` (and its sibling `TagResource`) mutate the stored `CollectionRecord` — one removes entries from `record.tags`, the other replaces the whole tag list — but neither touches `record.last_modified_date`. Every other write in this crate stamps it, and the crate states that as its own rule: `CollectionGroupRecord::apply_update`'s doc ends "Every write stamps `lastModifiedDate`.", `CollectionRecord::begin_update` stamps it, `CollectionRecord::begin_deletion` stamps it (so even an accepted DELETE moves the field), `PolicyWrite::apply` stamps it and `SecurityConfigRecord::apply` stamps it.

The consequence a client sees: after a successful `UntagResource`/`TagResource`, a `BatchGetCollection` (or `ListCollections`→`UpdateCollection`) reports a `lastModifiedDate` that predates the change the tagging call actually made to the stored record, so the field no longer answers what the model says it answers (`CollectionDetail$lastModifiedDate`: "The date and time when the collection was last modified."). No comment anywhere in the crate exempts a tag write from the stamp rule — the only comment about tags on the record (state.rs:646-648) is about which detail shape reports them, not about the timestamp.

Both copies of the gap are in the tagging plane; this issue is filed on `UntagResource` because the `TagResource`/wrong-behavior pair is already taken by a different open issue (the ARN-resolution one), and the fix is the same one-line stamp in both handlers.

## Evidence

The crate's own rule, verbatim — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs:921-940:
```
    /// `capacityLimits` is replaced WHOLE rather than merged member by member: the request
    /// ... exactly as a create does. Every write stamps `lastModifiedDate`.
    pub fn apply_update(
        &mut self,
        description: Option<String>,
        capacity_limits: Option<CollectionGroupCapacityLimits>,
        written_at: i64,
    ) {
        ...
        self.last_modified_date = written_at;
    }
```

The same stamp on every other writer of a collection — state.rs:687-694 (`begin_deletion`): `self.last_modified_date = deleted_at;`; state.rs:713-716 (`begin_update`): `self.last_modified_date = updated_at;`; state.rs:1128-1129 (`PolicyWrite::apply`): `*last_modified_date = self.written_at;`; state.rs:1560-1561 (`SecurityConfigRecord::apply`): `self.last_modified_date = write.written_at;`.

The two writers that skip it — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/untag_resource.rs:202-206:
```
        // Removing a key the resource does not carry removes nothing: the request asks for
        // a state (these keys absent) rather than for an event, so it is not a failure.
        record
            .tags
            .retain(|tag| !input.tag_keys.contains(&tag.key));
```
and services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/tag_resource.rs:228-238:
```
        let Some(record) = store.collection_by_arn_mut(&input.resource_arn) else {
            ... ResourceNotFoundException ...
        };
        record.tags = tags;
```
Neither handler reads or assigns `last_modified_date`, and neither calls `crate::state::now_epoch_millis()` at all (grep for `now_epoch_millis` matches create_collection.rs, update_collection.rs, delete_collection.rs, create_collection_group.rs, update_collection_group.rs, the policy writers and update_account_settings.rs — not the two tagging handlers).

What the field is modelled to mean — `smithy_query(query="shape", shape="CollectionDetail")`: `"lastModifiedDate": {"target": "Long", "doc": "<p>The date and time when the collection was last modified.</p>"}`.

Test coverage: tests/all/encryption_indexes_and_tagging.rs:880-945 is the tagging table ("One row per shape of tag write: adding keys, replacing a key's value, a no-op list, removing a subset, removing a key the resource never carried, and removing them all"); every row asserts only `listed_tags(&router, &arn)`, so nothing pins the timestamp across a tag write. The only `lastModifiedDate` assertion in that file is for `UpdateCollection` (encryption_indexes_and_tagging.rs:1188-1189: "lastModifiedDate is the write instant"), which is exactly the rule the tag writers skip.

## Steps to Reproduce

1. CreateSecurityPolicy(type="encryption", name="enc", policy='{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}') -> 200.
2. CreateCollection(name="logs-prod", tags=[{key:"team", value:"search"}]) -> 200; keep `createCollectionDetail.arn` and `id`.
3. Poll BatchGetCollection(ids=[id]) until status is ACTIVE; record `collectionDetails[0].lastModifiedDate` as T0.
4. Wait a moment, then call TagResource(resourceArn=<arn>, tags=[{key:"env", value:"prod"}]) -> 200, or UntagResource(resourceArn=<arn>, tagKeys=["team"]) -> 200.
5. ListTagsForResource(resourceArn=<arn>) -> the stored tag set has changed, so the record was written.
6. BatchGetCollection(ids=[id]) -> `lastModifiedDate` is still T0, unchanged by the write, although every other write in this crate (including an accepted DeleteCollection) advances it.

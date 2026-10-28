Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f08062a3928279f71031748dc453b175f9086c5b

# A create stores its `tags` list verbatim, so a repeated tag key persists a tag set the crate's own tagging plane can neither report nor update as the map it treats it as

_Filed: 2026-10-28T22:52:09Z_
_Operation: CreateCollection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f08062a3928279f71031748dc453b175f9086c5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCollection` (and `CreateCollectionGroup` on the same line of code) stores the request's `tags` list verbatim, with no key normalisation. The model permits a `tags` list that repeats a key — `Tags` carries no `@uniqueItems`, `Tag$key` carries no uniqueness constraint, and unlike `TagResourceRequest$tags` the create's own member documentation states no uniqueness rule — so `[{"key":"env","value":"a"},{"key":"env","value":"b"}]` reaches the handler and is persisted as two entries under one key.

That contradicts the rule the SAME crate's tagging plane states and implements. `TagResource` says in so many words that a stored tag set is a map keyed by `key`, and implements the merge as a first-match update (`tags.iter_mut().find(...)`). Against a create-time duplicate that merge is wrong in a way no later call can repair:

- `ListTagsForResource` answers the stored list straight through, so the resource reports the key `env` TWICE — which `CreateCollectionGroupDetail$tags` / `CollectionGroupDetail$tags` describe as "A map of key-value pairs", something that cannot hold one key twice.
- `TagResource {key:"env", value:"c"}` updates only the FIRST occurrence, so the resource afterwards reports `env=c` beside the stale `env=b`: the write the handler's own comment says "takes the new value" visibly does not take effect for that key, and the value a reader sees depends on which entry it looks at.
- The quota check in `TagResource` counts the duplicate twice, so a resource created with duplicated keys reaches `MAX_TAGS_PER_RESOURCE` on fewer distinct keys than one tagged through `TagResource`.

The fix belongs on the write that admits the state: the create should normalise (or refuse) a `tags` list that repeats a key, the way `TagResource` already normalises its merge, so every stored tag set really is the map the tagging plane and the response shapes treat it as.

## Evidence

The create stores the list unnormalised — `services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_collection.rs:354`:
```
            tags: input.tags.unwrap_or_default(),
```
and the identical line in the sibling create — `src/ops/create_collection_group.rs:292`:
```
            tags: input.tags.unwrap_or_default(),
```

The tagging plane in the SAME crate states the invariant the create breaks, and implements a first-match update — `src/ops/tag_resource.rs:179-191`:
```
        // The resulting tag set, computed before anything is stored: a key the resource
        // already carries takes the new value (a tag set is a map keyed by `key`), and a
        // key it does not is added.
        let mut tags = store
            .collection_by_arn(&input.resource_arn)
            .map(|record| record.tags.clone())
            .unwrap_or_default();
        for tag in input.tags {
            match tags.iter_mut().find(|stored| stored.key == tag.key) {
                Some(stored) => stored.value = tag.value,
                None => tags.push(tag),
            }
        }
```
(`find` stops at the first match, so a second entry under the same key is left untouched — and the quota check two lines later, `tag_resource.rs:192` `if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE`, counts it.)

The read hands the duplicated list back unchanged — `src/ops/list_tags_for_resource.rs:171-175`:
```
        // Answered from the STORED record: the tags a read reports are the ones the create
        // and the tag writes left on it.
        Ok(ListTagsForResourceOutput {
            tags: Some(record.tags.clone()),
        })
```

The response member the group create answers from is modelled as a MAP — `smithy_query(query="doc", shape="CreateCollectionGroupDetail", member="tags")`: "A map of key-value pairs associated with the collection group." — and `src/state.rs:848` renders it from the same stored list (`tags: Some(self.tags.clone())`).

The create's own member doc states no uniqueness rule, unlike the tag write's: `CreateCollectionRequest$tags` = "An arbitrary set of tags (key-value pairs) to associate with the OpenSearch Serverless collection." vs `TagResourceRequest$tags` = "A list of tags (key-value pairs) to add to the resource. All tag keys in the request must be unique." — so the duplicate cannot be refused at the boundary and must be handled by the handler.

Test coverage: the tag table in `tests/all/encryption_indexes_and_tagging.rs:882-925` covers adding keys, "replacing the value of a key already carried", an empty list, an empty value and removals — every row starts from a create whose `tags` carry ONE key (`tests/all/encryption_indexes_and_tagging.rs:867`: `"tags": [{ "key": "team", "value": "search" }]`). No test creates a resource with a repeated tag key, so nothing in the crate justifies storing one.

## Steps to Reproduce

1. CreateCollection {"name":"dup-tagged","encryptionConfig":{"aWSOwnedKey":true},"tags":[{"key":"env","value":"a"},{"key":"env","value":"b"}]} -> 200 (the boundary accepts it: Tags has no @uniqueItems and the member doc states no uniqueness rule). Note the returned arn, and poll until status ACTIVE.
2. ListTagsForResource {"resourceArn":"<arn>"} -> 200 with tags = [{env,a},{env,b}]: one key reported twice.
3. TagResource {"resourceArn":"<arn>","tags":[{"key":"env","value":"c"}]} -> 200.
4. ListTagsForResource {"resourceArn":"<arn>"} again.
5. Expected per tag_resource.rs:179-181 ("a key the resource already carries takes the new value (a tag set is a map keyed by `key`)"): the resource carries env exactly once, with value c.
6. Actual: tags = [{env,c},{env,b}] — the stale duplicate survives, so the key's value is ambiguous and the tag write did not take effect for it. The same duplication is visible directly in CreateCollectionGroup's own 200 body: CreateCollectionGroup {"name":"dup-group","standbyReplicas":"ENABLED","tags":[{"key":"env","value":"a"},{"key":"env","value":"b"}]} answers createCollectionGroupDetail.tags with env twice, although that member is modelled as "A map of key-value pairs associated with the collection group".

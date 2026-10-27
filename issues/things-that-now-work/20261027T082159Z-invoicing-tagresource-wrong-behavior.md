Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# TagResource counts a key repeated inside one ResourceTags array twice against the per-resource tag quota

_Filed: 2026-10-27T08:21:59Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource`'s quota check counts ARRAY ENTRIES that are not already on the resource, not DISTINCT new keys. A `ResourceTags` array that names the same key twice (which the model permits — `ResourceTagList` carries no `@uniqueItems`) therefore consumes two units of the per-resource quota, while the write that follows inserts the key once into the `BTreeMap`.

The effect is a refusal for a request that would have left the resource exactly at the quota: with a resource holding 49 tags and an array of two entries sharing one new key, `held + added == 51 > MAX_RESOURCE_TAGS` and the call answers 402 `ServiceQuotaExceededException`, even though applying it would store 50 tags — the same array applied as two separate calls succeeds.

The create path for the same quota does it the other way round: `crate::state::resource_tags_for_new_resource` collects the array into a `BTreeMap` first and compares the number of DISTINCT keys, so a create carrying the duplicate is accepted while the identical tag write is refused. `TagResource`'s own comment states the rule the code breaks: the quota is "on the RESOURCE … not on this call's array", yet duplicate entries in the call's array are what tip it over.

## Evidence

services/service-com-amazonaws-invoicing-invoicing/src/ops/tag_resource.rs:163-196 — the comment and the code disagree:
```
        // What the write would leave the resource holding, decided BEFORE anything is
        // written: a key already set is REPLACED rather than added, so re-tagging an
        // existing key is never a quota step. The quota is on the RESOURCE
        // (`MAX_RESOURCE_TAGS`), not on this call's array, …
        let added = input
            .resource_tags
            .iter()
            .filter(|tag| !already_set(&tag.key))
            .count();
        if held + added > crate::state::MAX_RESOURCE_TAGS {
            return Err(TagResourceErrorModeled::ServiceQuotaExceededException(
```
The write immediately below (:198-204) de-duplicates, proving the count is not what the write stores:
```
        for tag in input.resource_tags {
            tags.insert(tag.key, tag.value);
        }
```

The sibling path for the SAME quota counts distinct keys — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:1003-1006 and :1017-1025:
```
/// One call's `ResourceTags` array as the tag plane stores it for a resource THIS CALL
/// CREATES — one entry per key, a key repeated inside the array keeping the last value
/// it carried — or `Err(count)` with the number of distinct keys when storing them would
/// leave the new resource holding more than [`MAX_RESOURCE_TAGS`].
…
        .collect::<BTreeMap<_, _>>();
    if tags.len() > MAX_RESOURCE_TAGS {
        return Err(tags.len());
```
and its own doc at state.rs:1010-1011 says `TagResource` "counts only the keys the resource does not already hold" — keys, not entries.

Model: `smithy_query(query="shape", shape="ResourceTagList")` → `{"type": "list", "length": {"min": 0, "max": 200}, "member": "ResourceTag"}` — no `@uniqueItems`, so a repeated key is input the model allows.

Existing coverage exercises only distinct keys: tests/all/create_path_tag_quota.rs:463-479 ("One more DISTINCT key is past the quota") and :449-461 (re-writing an existing key is not a quota step); no test sends a repeated key inside one `ResourceTags` array.

## Steps to Reproduce

1. Create an invoice unit (or a procurement portal preference) and note its ARN.
2. TagResource with 49 distinct keys (`key-1` … `key-49`), which succeeds.
3. TagResource with `ResourceTags: [{"Key":"key-50","Value":"a"},{"Key":"key-50","Value":"b"}]`.
4. Observe HTTP 402 `ServiceQuotaExceededException` (`held=49 + added=2 > 50`), although the write would have left the resource holding 50 tags.
5. Send the same two entries as two separate TagResource calls: both succeed and `ListTagsForResource` reports 50 tags — the refusal depended on the shape of the call's array, not on the resource's tag count.

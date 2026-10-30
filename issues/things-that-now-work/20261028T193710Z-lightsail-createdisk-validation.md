Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 160eac1be171dea85ae8441ad5f20b2dc0c6ebbb

# Create-time tags are stored verbatim, so a resource can be created holding one tag key twice — the rule TagResource implements once

_Filed: 2026-10-28T19:37:10Z_
_Operation: CreateDisk | Class: validation_

> _Note: this issue was identified against the code base at commit `160eac1be171dea85ae8441ad5f20b2dc0c6ebbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every create in this crate that accepts `tags` bounds only the LIST LENGTH and then stores the list verbatim on the record. No create applies the per-resource key-uniqueness rule, which the crate implements exactly once in `crate::state::tags_merged_with` and which the model states for a resource ("Tag keys must be unique per resource"). A create carrying `tags: [{key: env, value: a}, {key: env, value: b}]` is answered 200 and the record is stored — and reported by GetDisk/GetDisks — with the key `env` twice.

The resulting state is one no later call can repair: `tags_merged_with` finds the FIRST entry carrying the key and overwrites only that one, so a subsequent `TagResource` for `env` leaves the second, stale `env` tag in place forever (while a single `UntagResource` for `env` drops both). The same duplicate also inflates the merged-set count that TagResource bounds, so a resource created with duplicated keys is refused tags earlier than the rule allows.

The same omission is in every sibling create that takes tags (CreateInstances, CreateBucket, CreateDiskSnapshot, CreateInstanceSnapshot, CreateRelationalDatabase, CreateCertificate, CreateLoadBalancer, CreateContactMethod, …); CreateDisk is named here as the first one.

## Evidence

The model states the rule on a RESOURCE — smithy_query(query="doc", shape="com.amazonaws.lightsail#TagResource"):
```
Adds one or more tags to the specified Amazon Lightsail resource. Each resource can have a
maximum of 50 tags. Each tag consists of a key and an optional value. Tag keys must be unique
per resource.
```

The crate implements it once — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:15569-15588:
```
/// Merging is BY KEY, the other half of [`tags_without`]'s rule: "tag keys must
/// be unique per resource" (the modelled `TagResource` documentation), so a key
/// the resource already carries keeps its place in the set and takes the new
/// value ...
pub fn tags_merged_with(record: &dyn TaggedRecord, added: &[Tag]) -> Vec<Tag> {
    let mut merged = record.tags().to_vec();
    for tag in added {
        match merged.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => merged.push(tag.clone()),
```
and applies it on the TagResource path — src/ops/tag_resource.rs:225-240:
```
// The RESULTING set is what the exact bound is on — "each
// resource can have a maximum of 50 tags" ...
let merged = crate::state::tags_merged_with(record, &added);
...
record.set_tags(merged);
```

The create skips it — src/ops/create_disk.rs:205-212 (length only) and src/ops/create_disk.rs:250-256 (stored verbatim):
```
let tags = input.tags.clone().unwrap_or_default();
if tags.len() > crate::state::MAX_TAGS {
    return Err(invalid(format!(
        "tags carries {} entries, more than the {} this service accepts",
...
store.insert_disk(crate::state::StoredDisk {
    name: name.clone(),
    ...
    tags,
```
Identical in src/ops/create_instances.rs:255-261 and :319 (`tags: tags.clone(),`) and src/ops/create_bucket.rs:199-206 and :285.

Test coverage: tests/all/resource_tagging.rs:556-565 pins the rule on the TagResource path ("one key named twice in one request: the last value wins") and tests/all/resource_tagging.rs:578 states it ("RULE: the merge is by tag key — \"tag keys must be unique per resource\""). No test creates a resource with a duplicated tag key.

## Steps to Reproduce

1. CreateDisk with diskName "dup-tags", availabilityZone "us-east-2a", sizeInGb 32 and tags [{"key":"env","value":"a"},{"key":"env","value":"b"}] — answered 200.
2. GetDisk with diskName "dup-tags": the reported tags carry the key "env" twice, a state the model says cannot exist ("Tag keys must be unique per resource").
3. TagResource with resourceName "dup-tags" and tags [{"key":"env","value":"c"}], then GetDisk again: only the first "env" entry took the new value — the resource still reports a second, stale "env" tag that no further TagResource call can reach.
4. UntagResource with resourceName "dup-tags" and tagKeys ["env"]: both entries disappear, so the only way out of the state is losing the key entirely.

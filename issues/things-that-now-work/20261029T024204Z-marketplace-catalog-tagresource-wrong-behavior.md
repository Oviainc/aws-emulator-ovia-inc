Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# A change set tagged with a repeated ChangeSetTags key is only half-updated by TagResource, breaking the handler's own one-value-per-key rule

_Filed: 2026-10-29T02:42:04Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TagResource states and implements the rule that a tag key carries exactly ONE value, but the rule is only ever applied to the keys a TagResource request carries, and its merge stops at the FIRST stored entry with that key. StartChangeSet, by contrast, persists `ChangeSetTags` verbatim: the only thing it checks on those tags is the reserved `aws:` prefix, and `TagList` carries no `@uniqueItems`, so a request may name the same key twice and the change-set record is stored holding two entries for one key.

The consequences, all on the same stored tag list that TagResource owns:

1. `ListTagsForResource` on that change set's ARN reports the same key twice — a tag set that is not a key->value map at all.
2. A later `TagResource` naming that key replaces only the FIRST copy (`merged.iter_mut().find(..)` returns one entry), so after a successful 200 the resource still carries the OLD value for the key the caller just wrote, and `UntagResource` (which uses `retain`) is the only way to get rid of it.
3. The per-resource tag accounting diverges: TagResource compares `merged.len()` (list entries) against `MAX_RESOURCE_TAGS`, while `judge_change_set` counts DISTINCT keys in a `HashSet` for the entity side of the same bound.

The entity path in the same crate does normalise: `apply_changes` removes any held entry with the key before pushing, so an entity record can never hold a key twice. Only the change-set tag list, written by StartChangeSet, escapes the rule.

## Evidence

The rule, stated and implemented in the handler — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/tag_resource.rs:189`:
```
        // A tag is a key with ONE value, so tagging with a key the resource
        // already carries replaces that value instead of adding a second entry;
        // every other key is appended.
        let mut merged = held.clone();
        for tag in input.tags {
            match merged.iter_mut().find(|existing| existing.key == tag.key) {
                Some(existing) => existing.value = tag.value,
                None => merged.push(tag),
            }
        }
        if merged.len() > crate::state::MAX_RESOURCE_TAGS {
```
(`find` yields the first match only; `merged.len()` counts entries, not keys.)

The path that stores a list the rule was never applied to — `src/ops/start_change_set.rs:667`:
```
            tags: input.change_set_tags.unwrap_or_default(),
```
its only check on those tags, `src/ops/start_change_set.rs:219`:
```
        if let Some(tags) = input.change_set_tags.as_deref() {
            if let Some(reserved) = crate::state::first_reserved_tag(tags) {
```

The sibling write that DOES normalise, `src/state.rs:1787` (`apply_changes`):
```
                for tag in change.entity_tags {
                    entry.tags.retain(|held| held.key != tag.key);
                    entry.tags.push(tag);
                }
```

The same list is what the tagging surface reads and writes — `src/state.rs:1602` (`resource_tags`): `CatalogResource::ChangeSet { id } => { self.change_sets.get(id).map(|held| held.tags.as_slice())` — reported verbatim by `src/ops/list_tags_for_resource.rs:191`: `tags: (!tags.is_empty()).then(|| tags.to_vec()),`.

The distinct-key accounting on the entity side, `src/state.rs:1700`:
```
        let mut tag_keys: HashMap<String, HashSet<String>> = HashMap::new();
```

Test coverage: `tests/all/resource_policy_and_tagging.rs:465` pins the rule for TagResource — "// Re-tagging a key the resource already carries replaces its value rather than / // adding a second entry: a tag is one value per key." — and `tests/all/reserved_tag_keys.rs:483` (`("ChangeSetTags", Some(tags.clone()), None)`) is the only case that exercises `ChangeSetTags` at all. No test sends a repeated key in `ChangeSetTags`, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /StartChangeSet with {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"p\"}"}],"ChangeSetTags":[{"Key":"env","Value":"a"},{"Key":"env","Value":"b"}]} — answered 200; note the ChangeSetId
2. GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> to read the ChangeSetArn
3. POST /ListTagsForResource {"ResourceArn":"<ChangeSetArn>"} — Tags carries TWO entries for the one key `env` (`env=a` and `env=b`)
4. POST /TagResource {"ResourceArn":"<ChangeSetArn>","Tags":[{"Key":"env","Value":"c"}]} — answered 200
5. POST /ListTagsForResource {"ResourceArn":"<ChangeSetArn>"} again — Tags now reports `env=c` AND the stale `env=b`: the write replaced only the first copy, so the key the caller just set still reports an old value

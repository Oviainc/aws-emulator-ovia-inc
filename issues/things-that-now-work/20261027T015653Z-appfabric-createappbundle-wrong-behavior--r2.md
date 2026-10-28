Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b4c888ef8ded8aa913bb7b3b9f51a492e4dac1e5

# Create operations persist the tag list verbatim, so a repeated tag key is stored twice while TagResource collapses the same input

_Filed: 2026-10-27T01:56:53Z_
_Operation: CreateAppBundle | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b4c888ef8ded8aa913bb7b3b9f51a492e4dac1e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The four create handlers store `input.tags` straight into the record without passing it through the crate's own tag-set normaliser. `TagList` carries no `@uniqueItems`, so a request may legitimately carry the same key twice; TagResource handles that case by merging through `crate::state::merge_tags`, which states that a tag list is a map of key-value pairs and collapses a repeated key onto one entry. The create paths do not, so the SAME tag list produces two different stored tag sets depending on which operation received it.

The resulting record violates the map invariant the rest of the crate relies on: ListTagsForResource then reports one key twice (its own test harness asserts that can never happen), and a later TagResource for that key rewrites only the FIRST copy through `merge_tags`, leaving the resource holding two contradictory values for one key that no sequence of TagResource calls can repair (only UntagResource, which drops both). The fix is for the create paths to use the normaliser the crate already has.

## Evidence

The rule, stated in this crate — src/state.rs:1039-1046: "/// Merge `incoming` into `tags` the way `TagResource` does: a tag list is a map /// of key-value pairs, so a key already present takes the new value in place /// (keeping the position it was first assigned at) and a new key is appended." `pub fn merge_tags(tags: &[Tag], incoming: &[Tag]) -> Vec<Tag>`

The path that applies it — src/ops/tag_resource.rs:154-170: "// Merged first, so a merge past the modelled bound is refused with the // resource's tags left exactly as they were." `let merged = crate::state::merge_tags(held, &input.tags); ... *held = merged;`

The paths that skip it (identical omission in all four creates):
- src/ops/create_app_bundle.rs:225 `tags: input.tags.unwrap_or_default(),`
- src/ops/create_app_authorization.rs:381 `tags: input.tags.unwrap_or_default(),`
- src/ops/create_ingestion.rs:354 `tags: input.tags.unwrap_or_default(),`
- src/ops/create_ingestion_destination.rs:346 `tags: input.tags.unwrap_or_default(),`
(grep for `merge_tags` in src/ shows exactly one caller: tag_resource.rs:156.)

What the stored set is then reported as — src/ops/list_tags_for_resource.rs:155-159: "// Always a list, empty for a resource that holds no tags" `tags: Some(held.clone())`.

The invariant this breaks, asserted by this crate's own tests — tests/all/resource_tags.rs:95-99: `let previous = mapped.insert(key.to_string(), value.to_string()); assert!(previous.is_none(), "case {case:?}: key {key:?} reported twice in {tags}");` and tests/all/resource_tags.rs:7-11: "a tag set is a map of key-value pairs held on the resource its ARN names". No existing test creates a resource with a repeated tag key, which is why the suite is green today.

The model permits the input — smithy_query(shape="TagList"): `{"type": "list", "length": {"min": 0, "max": 50}, "member": "Tag"}` with no uniqueItems, and Tag's doc: "The key or keys of the key-value pairs for the tag or tags assigned to a resource."

## Steps to Reproduce

1. POST /appbundles with body {"tags":[{"key":"a","value":"1"},{"key":"a","value":"2"}]} -> 201, note appBundle.arn
2. GET /tags/{arn} -> the tags list contains key "a" TWICE ({a:1} and {a:2}); feeding this answer to tests/all/resource_tags.rs `reported_tags` trips its "key reported twice" assertion
3. Create a second bundle (in another Region) with no tags and POST /tags/{arn2} with the same list -> GET /tags/{arn2} reports ONE entry a=2, because TagResource merges through merge_tags: the same input, two different stored sets
4. POST /tags/{arn} (the bundle from step 1) with {"tags":[{"key":"a","value":"3"}]} -> 200
5. GET /tags/{arn} -> the resource now reports a=3 AND a=2: the second copy is unreachable to TagResource, so no tag write can repair the set

Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 465e7599899ea277984bee5e564f3564a11f5f81

# The resultant-tag-set bound is implemented twice (TagResource handler and state::register_provider)

_Filed: 2026-10-26T05:30:45Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `465e7599899ea277984bee5e564f3564a11f5f81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "merge the requested pairs into the resource's stored tag set, and refuse when the RESULTANT set would exceed `MAX_TAGS_PER_RESOURCE`" is implemented twice in this crate, in two different ways, with no shared helper: once in the `TagResource` handler (count the keys the write would add to the stored map, compare, then insert) and once in `state::register_provider`'s idempotent-repeat path (clone, extend, compare the merged length, then store). Both end in the same wire refusal (`crate::scaffolded::Scaffolded::tag_limit_exceeded()` / `RegisterRefusal::TagLimitExceeded`, which the handler maps to the same constructor), so the two copies are the same rule and can drift — e.g. a change to how the bound counts overwrites versus additions would have to be made in both places, and only one of them is reached by each test suite.

A single helper on the state module (merge a tag map into a stored set, refusing when the resultant set would pass `MAX_TAGS_PER_RESOURCE`) would serve both callers and leave every response and every existing test unchanged: both paths already refuse with the identical status, code and message, and both already leave the stored set untouched on refusal.

## Evidence

Copy 1 — src/ops/tag_resource.rs:171-188:
```
// The resultant set is what the resource would carry, so it is the set the tag
// bound applies to: count the keys the write would ADD to what is stored.
let resultant = tags.len()
    + input
        .tags
        .keys()
        .filter(|key| !tags.contains_key(*key))
        .count();
if resultant > crate::state::MAX_TAGS_PER_RESOURCE {
    return Err(TagResourceError::tag_limit_exceeded());
}
...
for (key, value) in input.tags {
    tags.insert(key, value);
}
```
Copy 2 — src/state.rs:348-354 (`register_provider`, the `@idempotent` repeat path):
```
let mut merged = record.tags.clone();
merged.extend(tags);
if merged.len() > MAX_TAGS_PER_RESOURCE {
    return Err(RegisterRefusal::TagLimitExceeded);
}
record.tags = merged;
```
Both refuse with the same unmodeled refusal: src/state.rs:483-488 documents `RegisterRefusal::TagLimitExceeded` as "the same condition reached from a handler must not acquire a second code", and src/ops/register_subscription_provider.rs:232-234 maps it to `crate::scaffolded::Scaffolded::tag_limit_exceeded()` — the very constructor `tag_resource.rs:179` calls.

Tests that pin both copies and must stay unchanged: `tests/all/resource_tags.rs:550-597` (`a_write_past_the_tag_maximum_is_refused_and_stores_nothing`, rows `(MAX_TAGS - 1, 1, true)`, `(MAX_TAGS, 1, false)`, `(MAX_TAGS - 1, 2, false)`) and `tests/all/subscription_provider_registration.rs:498-557` (`a_repeat_past_the_tag_maximum_is_refused`, rows `(MAX_TAGS - 10, 10, true)`, `(MAX_TAGS, 1, false)`), plus `tests/all/error_taxonomy.rs:220-226` which pins the shared message "The resultant tag set must not have more than 50 user tags."

## Steps to Reproduce

1. Read src/ops/tag_resource.rs:171-180 — the resultant-set bound computed by counting the keys the write adds.
2. Read src/state.rs:348-354 — the same bound computed by cloning and extending the stored map.
3. Note both refusals land on `Scaffolded::tag_limit_exceeded()` (src/ops/tag_resource.rs:179 and src/ops/register_subscription_provider.rs:232-234), so the two copies implement one rule.
4. Change how the bound counts in one copy only (e.g. the treatment of a key that is overwritten rather than added) and observe the two paths disagree while each suite still passes its own half.

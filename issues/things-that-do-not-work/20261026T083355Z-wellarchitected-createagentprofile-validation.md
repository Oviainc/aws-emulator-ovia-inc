Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: bd074c72652bf1c498424f058cd9d0b07e5b999b

# CreateAgentProfile writes its `tags` with no tag-count bound, so an agent profile can be created holding more tags than the crate's own rule allows every resource

_Filed: 2026-10-26T08:33:55Z_
_Operation: CreateAgentProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `bd074c72652bf1c498424f058cd9d0b07e5b999b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has exactly one rule for how many tags a stored resource may hold — `crate::state::MAX_TAGS_PER_RESOURCE` (50) — and `crate::state::merged_tag_count` documents itself as "Shared by every operation that writes tags to a resource". Two tag writers honour it: `TagResource` refuses a request that would push the resource past the ceiling, and `ImportLens` does the same on its update path. `CreateAgentProfile` is the third tag writer in the crate (its `tags` land in `AgentProfileRecord::tags`, which is the very map `state::resource_tags_mut` hands back for `TaggedResource::AgentProfile`, so `TagResource`/`UntagResource`/`ListTagsForResource` all act on it), and it applies no bound at all: it collects `input.tags` straight into the stored `BTreeMap`.

Unlike every other tag-carrying create in this service, the agent-profile input is NOT bounded by the model either: `CreateWorkload`, `CreateProfile`, `CreateReviewTemplate` and `ImportLens` all take a `TagMap`, which carries `@length(min: 1, max: 50)` and is therefore held at 50 by the request boundary before any handler runs, whereas `CreateAgentProfile$tags` targets `Tags`, a plain `list<Tag>` with no `@length`. So nothing — model or handler — stops a create from storing 51, 500 or 5000 tags on an agent profile.

The consequences are observable: `ListTagsForResource` on that profile reports a tag map the crate's own rule says cannot exist, and every subsequent `TagResource` call against it is permanently refused with 400 `ValidationException` (because `merged_tag_count(stored, requested) > 50` from the first stored tag onward), so the resource is wedged out of the tagging plane it is otherwise a full member of. The fix is the one check the two sibling writers already make, against the same shared helpers, before anything is stored.

## Evidence

The rule, single-homed and declared universal — `services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/state.rs:1736-1739`:
```
/// The most tags one resource carries: `TagMap` is `@length(min: 1, max: 50)` wherever a
/// request supplies one (`smithy_query(query="operation", operation="ImportLens")`), so
/// 50 is the most a stored resource can hold too.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;
```
and `state.rs:1750-1751`: "Shared by every operation that writes tags to a resource, so two tag writers cannot drift on what they count."

Sibling writer 1 — `src/ops/tag_resource.rs:160-178`:
```
// A resource holds at most `TagMap`'s own maximum of tags ... Counted and worded by
// the shared helpers beside `MAX_TAGS_PER_RESOURCE`, so this and `ImportLens`'s tag
// write cannot drift, and checked BEFORE anything is written ...
if crate::state::merged_tag_count(stored, &tags) > crate::state::MAX_TAGS_PER_RESOURCE {
    return Err(crate::scaffolded::Scaffolded::validation(
        crate::state::constraint_sentence(
            crate::state::TAGS_MEMBER,
            &crate::state::tag_ceiling_constraint("the resource"),
```

Sibling writer 2 — `src/ops/import_lens.rs:258-265`:
```
if crate::state::merged_tag_count(&record.tags, &tags)
    > crate::state::MAX_TAGS_PER_RESOURCE
{
    return Err(field_validation(
        crate::state::TAGS_MEMBER,
        &crate::state::tag_ceiling_constraint("the lens"),
    ));
}
```

The violating path — `src/ops/create_agent_profile.rs:214-219`, the whole of what it does with `tags` (no `merged_tag_count`, no `MAX_TAGS_PER_RESOURCE`, no refusal anywhere in the handler; `grep MAX_TAGS_PER_RESOURCE src/ops` matches only `tag_resource.rs` and `import_lens.rs`):
```
let tags: std::collections::BTreeMap<String, String> = input
    .tags
    .unwrap_or_default()
    .into_iter()
    .map(|tag| (tag.key, tag.value))
    .collect();
```
and it is stored verbatim at `create_agent_profile.rs:305` (`tags,` inside `crate::state::AgentProfileRecord { .. }`).

That stored map IS the tag map of a taggable resource — `state.rs:6278-6281`:
```
TaggedResource::AgentProfile => self
    .agent_profiles
    .get(resource_arn)
    .map(|record| &record.tags),
```
and `state.rs:6306-6309` for `resource_tags_mut`, with `state.rs:6254-6258` naming "the agent profiles" among "every taggable resource type of the service".

The model shows the boundary does not bound this member, unlike every other tag-carrying create: `smithy_query(query="shape", shape="Tags,TagMap")` returns `"Tags": {"type": "list", "member": "Tag"}` (no `length`) against `"TagMap": {"type": "map", "length": {"min": 1, "max": 50"}, ...}`; `CreateAgentProfile`'s input member is `"tags": {"target": "Tags", "type": "list"}`.

Test coverage that pins the rule for agent profiles — but only through `TagResource`, never through the create: `tests/all/resource_tagging.rs:562-617`, `async fn a_request_that_would_push_the_resource_past_the_tag_maximum_is_refused()` loops `for kind in KINDS`, and `KINDS` includes `Kind::AgentProfile` (`tests/all/resource_tags.rs:84`). The create-side table tops out AT the bound and never past it — `tests/all/resource_tags.rs:187-193`: `case: "the most tags a TagMap carries", tags: (0..state::MAX_TAGS_PER_RESOURCE)`, and the agent-profile create path that sends them is `resource_tags.rs:293-295` (`body["tags"] = tag_list(tags);`). So the gap sits exactly in the untested row.

## Steps to Reproduce

1. Read services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/state.rs:1736-1739 and :1745-1761 — MAX_TAGS_PER_RESOURCE = 50 and merged_tag_count, documented as shared by every operation that writes tags to a resource
2. Read src/ops/tag_resource.rs:160-178 and src/ops/import_lens.rs:258-265 — both tag writers call merged_tag_count against MAX_TAGS_PER_RESOURCE before writing, and refuse with 400 ValidationException naming Tags
3. Read src/ops/create_agent_profile.rs:214-219 and :305 — input.tags is collected into a BTreeMap and stored with no count check; grep MAX_TAGS_PER_RESOURCE over src/ops shows only tag_resource.rs and import_lens.rs
4. Confirm via smithy_query(query="shape", shape="Tags,TagMap") that CreateAgentProfile$tags targets Tags (list<Tag>, no @length) while every other tag-carrying create targets TagMap (@length(min: 1, max: 50)), so the request boundary bounds the others and not this one
5. POST /api/v1/agent-profiles with a valid name/pillars/executionRoleArn/aggregationConfiguration and a `tags` list of 60 distinct key/value pairs — the create is accepted 200 and the response echoes all 60
6. GET /tags/{the minted agent-profile ARN} (ListTagsForResource) — it reports 60 tags, a count state.rs:1736-1739 says a stored resource cannot hold
7. POST /tags/{that ARN} (TagResource) with a single new key — it is refused 400 ValidationException by merged_tag_count, so the profile can never be tagged again, while tests/all/resource_tagging.rs:562-617 asserts that exact ceiling holds for Kind::AgentProfile

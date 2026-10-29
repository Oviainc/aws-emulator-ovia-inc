Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# The two tag-map rules are applied in opposite order on the tag write paths, so one bad map is refused two different ways

_Filed: 2026-10-29T12:53:50Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers in this crate write a resource's tags, and all three hold the same two rules: a map may not name a key in the AWS-reserved namespace, and a resource may not end up holding more than `state::MAX_TAGS_PER_RESOURCE` tags. The two creates decide the COUNT first and the reserved NAMESPACE second; `TagResource` decides them the other way round.

So a single request that breaks both rules is classified by which path it was sent on, not by what is wrong with it:

* `CreateApprovalTeam` / `CreateIdentitySource` with a `Tags` map that is both over the ceiling and names `aws:owner` answer the ceiling refusal ("Tags names N tags, more than the maximum of 50 tags allowed per resource.").
* `TagResource` with the same map answers the reserved-namespace refusal ("Tags names a key in the 'aws:' namespace, which is reserved for Amazon Web Services...") — and, because the ceiling refusal on this path is a different modelled error, the two paths differ in error SHAPE too (`ValidationException` 400 on the creates, `TooManyTagsException` 400 on `TagResource`).

Nothing in the crate states which of the two rules wins. The crate does state the opposite intent — that these are one shared set of tag rules that every writing path applies the same way — and its tests assert cross-path agreement for the ceiling ("One rule, one sentence"), but no comment, doc or test fixes the relative order, so the divergence is unexplained and either copy can be changed without the other noticing.

## Evidence

Creates: ceiling first, reserved namespace second.
src/ops/create_approval_team.rs:295-317:
```
        // One resource carries at most `MAX_TAGS_PER_RESOURCE` tags, the ceiling
        // `TagResource` reports with `TooManyTagsException`. ... The
        // rule itself lives once, beside the other tag rules in `crate::state`.
        if let Some(refusal) = crate::state::tags_past_resource_ceiling(input.tags.as_ref()) {
            return Err(CreateApprovalTeamErrorModeled::ValidationException(refusal).into());
        }
        ...
        if let Some(tags) = input.tags.as_ref() {
            if crate::state::names_reserved_tag_key(tags.keys().map(String::as_str)) {
                return Err(CreateApprovalTeamErrorModeled::ValidationException(
                    crate::state::reserved_tag_key(),
```
src/ops/create_identity_source.rs:297-318 is the same order (`tags_past_resource_ceiling` at :303, `names_reserved_tag_key` at :312).

TagResource: reserved namespace first, ceiling second.
src/ops/tag_resource.rs:204-231:
```
        // A tag key in the namespace reserved for Amazon Web Services belongs to AWS,
        // not to the caller, so this call cannot create or replace one ...
        if crate::state::names_reserved_tag_key(input.tags.keys().map(String::as_str)) {
            return Err(
                TagResourceErrorModeled::ValidationException(crate::state::reserved_tag_key())
                    .into(),
            );
        }

        // Whether the merged set fits is decided before anything is written ...
        let merged = crate::state::merged_tag_count(tags, input.tags.keys().map(String::as_str));
        if merged > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::TooManyTagsException(TooManyTagsException {
```
The rule the crate states for itself — src/state.rs:192-193: "[`reserved_tag_key`] — the one message every path that WRITES tags refuses a key in the [`RESERVED_TAG_KEY_PREFIX`] namespace with" — and src/ops/create_approval_team.rs:300: "The rule itself lives once, beside the other tag rules in `crate::state`."

The crate already asserts cross-path agreement for the ceiling, but only path-against-path for one rule at a time — tests/all/create_tag_ceiling.rs:218-227: "One rule, one sentence: for every refused count both paths answered, each path's message is the other's." No test in tests/all/create_tag_ceiling.rs, tests/all/reserved_tag_namespace.rs or tests/all/write_path_input_validation.rs sends a map that breaks BOTH rules, so the order is unpinned on every path.

## Steps to Reproduce

1. Build a `Tags` map with 51 entries, one of whose keys is `aws:owner`.
2. Send it as `CreateApprovalTeam` (POST /approval-teams) with an otherwise valid body: the answer is `ValidationException` naming 51 tags and the maximum of 50 (src/ops/create_approval_team.rs:301 runs first).
3. Send the same map as `TagResource` (PUT /tags/{ResourceArn}) against an existing approval team: the answer is `ValidationException` naming the reserved `aws:` namespace (src/ops/tag_resource.rs:209 runs first), never the `TooManyTagsException` that path raises for the ceiling.
4. grep -n 'tags_past_resource_ceiling\|names_reserved_tag_key\|merged_tag_count' services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops — the two creates evaluate the count before the namespace, TagResource the namespace before the count, and no comment or test states which order is intended.

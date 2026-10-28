Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8668e0a390f031d37f864cf13043544a41d29fed

# The create-time per-resource tag-ceiling refusal is written out twice, once per create, although the sibling tag rules live in state.rs

_Filed: 2026-10-28T15:06:52Z_
_Operation: CreateApprovalTeam | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8668e0a390f031d37f864cf13043544a41d29fed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate centralizes every other tag rule in `crate::state`: the reserved-namespace predicate and its refusal (`names_reserved_tag_key` / `reserved_tag_key`) and the merge arithmetic `TagResource` uses (`merged_tag_count`), so all three tag-writing paths raise one and the same sentence. The create-time ceiling rule is the exception: `CreateApprovalTeam` and `CreateIdentitySource` each spell out the whole rule themselves — count `input.tags`, compare with `MAX_TAGS_PER_RESOURCE`, and build a `ValidationException` from a byte-identical format string. Two copies of one rule can drift (and only one of them has a test), while a single `crate::state` constructor beside `reserved_tag_key()` would keep both responses exactly as they are today.

## Evidence

Copy 1 — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/create_approval_team.rs:294-307:
```
        let tag_count = input
            .tags
            .as_ref()
            .map_or(0, ::std::collections::HashMap::len);
        if tag_count > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(CreateApprovalTeamErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "Tags names {tag_count} tags, more than the maximum of {} tags allowed \
                     per resource.",
                    crate::state::MAX_TAGS_PER_RESOURCE
                ),
            })
            .into());
        }
```
Copy 2 — src/ops/create_identity_source.rs:302-317, the same condition and the same sentence:
```
        let tag_count = input
            .tags
            .as_ref()
            .map_or(0, ::std::collections::HashMap::len);
        if tag_count > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(CreateIdentitySourceErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "Tags names {tag_count} tags, more than the maximum of {} tags allowed \
                         per resource.",
                        crate::state::MAX_TAGS_PER_RESOURCE
                    ),
                },
            )
            .into());
        }
```
The sibling tag rule this crate already states once — state.rs:788-800:
```
/// The refusal a request that would write a tag key in the reserved namespace earns.
...
pub fn reserved_tag_key() -> ValidationException {
```
used by all three writing paths (create_approval_team.rs:315-319, create_identity_source.rs:324-328, tag_resource.rs:209-213), and state.rs:749 `merged_tag_count` for `TagResource`'s arithmetic.
Tests that pin the behaviour a fold must not change: tests/all/resource_tags.rs:472-510 `create_refuses_more_tags_than_a_resource_may_hold` (rows "exactly the maximum" / "one past the maximum", asserting 400 `ValidationException` and that nothing was stored) and tests/all/identity_sources.rs:94-96, which creates an identity source carrying exactly `state::MAX_TAGS_PER_RESOURCE` tags.

## Steps to Reproduce

1. Read src/ops/create_approval_team.rs:294-307 and src/ops/create_identity_source.rs:302-317: the same condition (`tag_count > state::MAX_TAGS_PER_RESOURCE`) and the same message are written out in full in both handlers.
2. Read state.rs:788-800 and state.rs:749: the two neighbouring tag rules (reserved namespace, merged count) each live once in `crate::state` and are called from every path that needs them.
3. Note that only the `CreateApprovalTeam` copy is covered by a refusal test (tests/all/resource_tags.rs:472), so an edit to one copy's sentence or comparison would not be caught in the other.
4. Fold the rule into one `crate::state` constructor (e.g. beside `reserved_tag_key`) and have both creates lift it into their own `ValidationException`; every response byte and every existing test stays unchanged.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# TagResource stores reserved aws:-prefixed tag keys instead of refusing them

_Filed: 2026-10-24T08:25:29Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` performs no validation of the tag keys it merges into the rule: it checks only the revision and the 200-tag per-resource bound. The modeled `TagKey` pattern `^[\w\s\.\-\:\/\=\+\@]+$` admits a colon, so a key such as `aws:createdBy` passes the boundary validator and is persisted.

Expected: AWS reserves the `aws:` prefix for AWS-managed tags and refuses a caller-supplied tag key using it — here with the operation's `InvalidParameterValueException` (HTTP 400) — leaving the rule's tag set unchanged. Actual: HTTP 200, and the reserved key becomes part of the rule's tags, visible from `ListTagsForResource` and `GetAutomationRule`, and removable by `UntagResource` (`crate::state::tag_key_is_valid`, used by `UntagResource`, also permits the prefix).

## Evidence

Spec: `TagResource` declares `InvalidParameterValueException` (HTTP 400); `Tag$key` targets `TagKey` (`@length(1..128)`, pattern `^[\w\s\.\-\:\/\=\+\@]+$`), which does not exclude the `aws:` prefix, so the prefix rule is a server-side check. Reserving `aws:` for AWS-managed tags and rejecting caller-supplied keys that use it is established AWS-wide tagging behaviour. Handler: `src/ops/tag_resource.rs` goes from the revision check straight to `merge_tags(&rule.tags, &input.tags)` and the 200-tag bound — there is no key inspection; `src/state.rs::tag_key_is_valid` (the only tag-key validator in the crate, used by `UntagResource`) checks length and character class only. Test coverage: `tests/all/resource_tagging.rs` / `resource_tags.rs` contain no reserved-prefix case.

## Steps to Reproduce

1. Enroll with `UpdateEnrollmentConfiguration {"status":"Active"}` and create a rule; note `ruleArn` and `ruleRevision`.
2. Call `TagResource` with `{"resourceArn":<ruleArn>,"ruleRevision":<rev>,"tags":[{"key":"aws:createdBy","value":"me"}]}` — expect `InvalidParameterValueException` (400) and no change to the rule's tags.
3. Observe HTTP 200, then call `ListTagsForResource` with the same `resourceArn` — observe the reserved `aws:createdBy` tag present on the rule.

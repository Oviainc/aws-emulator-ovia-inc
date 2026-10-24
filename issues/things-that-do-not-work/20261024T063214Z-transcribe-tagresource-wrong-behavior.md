Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Tagging operations refuse Call Analytics category ARNs, so category tags are unreachable

_Filed: 2026-10-24T06:32:14Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three tagging operations resolve an ARN through `ComAmazonawsTranscribeTranscribeState::tags` / `tags_mut`, which match only four resource types: `transcription-job`, `vocabulary`, `medical-vocabulary` and `vocabulary-filter`. Every other Transcribe resource type falls into the `_ => None` arm, which each handler renders as a not-found `BadRequestException`.

Call Analytics categories ARE a taggable resource family: `CreateCallAnalyticsCategory` accepts `Tags` (list<Tag>, len 1..200), the handler stores them on `CallAnalyticsCategoryRecord.tags`, and `CategoryProperties.Tags` reports them. But no tagging operation can reach that set: `ListTagsForResource` on a category ARN answers 400 instead of 200 with the stored tags, `TagResource` cannot add a tag to an existing category, and `UntagResource` cannot remove one. So tags supplied at create time are write-once and invisible to the standard tagging API, and the documented tag-based workflow (create → list tags → tag → untag) is impossible for this family even though the create accepted tags.

Expected: an ARN of the form `arn:aws:transcribe:<region>:<account>:<type>/<name>` naming a stored Call Analytics category resolves to that category's tag set — `ListTagsForResource` answers 200 with `ResourceArn` and `Tags`, `TagResource` merges into the stored set (observable through a later `GetCallAnalyticsCategory`, whose `CategoryProperties.Tags` must change), and `UntagResource` removes the named keys. Actual: HTTP 400 `BadRequestException` with a generic "resource couldn't be found" message for an ARN whose category exists. (The same unmapped-type hole will apply to the language-model family once `CreateLanguageModel` is implemented — it too accepts `Tags`.)

## Evidence

Spec: `smithy_query(query="operation", operation="TagResource,ListTagsForResource,UntagResource")` — `ResourceArn` targets `TranscribeArn` with pattern `^arn:aws(-[^:]+)?:transcribe:[a-zA-Z0-9-]*:[0-9]{12}:[a-zA-Z-]*/[0-9a-zA-Z._-]+$`, i.e. ANY Transcribe resource type segment is a legal input, not just the four this emulator maps; `CreateCallAnalyticsCategory` takes `Tags` (so the resource is taggable by construction) and `CategoryProperties$Tags` reports them. Handler/state: `src/state.rs` `tags` (lines ~406-426) and `tags_mut` (lines ~438-458) match only `RESOURCE_TYPE_TRANSCRIPTION_JOB`/`VOCABULARY`/`MEDICAL_VOCABULARY`/`VOCABULARY_FILTER` and return `None` otherwise — the doc comment admits "a Call Analytics category … answers `None`, which the caller renders as the modelled not-found error"; `resource_kind` (lines ~244-252) falls back to `KIND_RESOURCE` for it. Call sites: `src/ops/tag_resource.rs` lines 177-180 + `not_found(..)`, `src/ops/untag_resource.rs` lines 185-196, `src/ops/list_tags_for_resource.rs` lines 184-198. Tests: `tests/all/vocabulary_file_uri.rs` and `tests/all/vocabulary_delete_and_tag_sets.rs` exercise tagging only for `vocabulary` ARNs; `tests/all/call_analytics_categories.rs` never calls a tagging operation, so nothing asserts the correct behavior.

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.CreateCallAnalyticsCategory` and body {"CategoryName":"tagged-cat","Rules":[{"NonTalkTimeFilter":{"Threshold":10000}}],"Tags":[{"Key":"team","Value":"speech"}]} — 200, and `GetCallAnalyticsCategory` shows CategoryProperties.Tags with that tag
2. Call `Transcribe.ListTagsForResource` with {"ResourceArn":"arn:aws:transcribe:us-east-1:<emulator-account>:analytics-category/tagged-cat"} (the ARN type segment AWS publishes for a Call Analytics category)
3. Emulator answers HTTP 400 BadRequestException ("the requested resource couldn't be found") because the type segment is unmapped in state.rs; AWS answers 200 with ResourceArn and the category's Tags
4. Call `Transcribe.TagResource` on the same ARN with {"Tags":[{"Key":"env","Value":"test"}]} — emulator 400, AWS 200; afterwards `GetCallAnalyticsCategory` should show both tags but the emulator's set is unchanged
5. Call `Transcribe.UntagResource` on the same ARN with {"TagKeys":["team"]} — emulator 400, AWS 200 and the key is gone from CategoryProperties.Tags

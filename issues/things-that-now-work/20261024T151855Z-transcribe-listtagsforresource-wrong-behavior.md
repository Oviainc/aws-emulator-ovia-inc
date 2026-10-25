Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# Tagging operations cannot reach a Call Analytics job's tag set, so job tags are write-only

_Filed: 2026-10-24T15:18:55Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A Call Analytics job accepts tags at start time (`StartCallAnalyticsJob` input `Tags`) and reports them back in `CallAnalyticsJob$Tags`, but the standard tagging API cannot see or modify that tag set: an ARN whose resource-type segment names a Call Analytics job (`arn:aws:transcribe:<region>:<account>:analytics-job/<job-name>`) resolves to no family at all in the emulator.

Expected (AWS): `ListTagsForResource` on the ARN of an existing Call Analytics job answers 200 with the tags the job carries; `TagResource` adds/replaces keys on that job and `UntagResource` removes them, exactly as for every other taggable Transcribe resource family (transcription jobs, medical transcription jobs, vocabularies, medical vocabularies, vocabulary filters, language models, analytics categories, medical scribe jobs). The Call Analytics job IS a taggable Transcribe resource — the start operation models `Tags` and the job shape reports `Tags` — so the tags must be reachable through the tagging API.

Actual (emulator): all three tagging operations refuse such an ARN with `BadRequestException` ("The requested resource couldn't be found. Check the resource name and try your request again.") whether or not the job exists, because `crate::state::ResourceFamily::from_resource_type` has no variant for the Call Analytics job family and `tags()` / `tags_mut()` therefore return `None`. The tags a `StartCallAnalyticsJob` request carried are stored on the record and echoed by `StartCallAnalyticsJob` / `GetCallAnalyticsJob`, but they can never be listed, added to, or removed afterwards — a write-only tag set.

The same gap was filed and fixed for Call Analytics CATEGORY ARNs (issue 20261024T063214Z-transcribe-tagresource-wrong-behavior, now fixed, which added the `CallAnalyticsCategory` family); the Call Analytics JOB family was left out, and `src/state.rs` records that omission explicitly ("like the Medical Scribe jobs it is absent from Self::tags / Self::tags_mut because this emulator has no recorded spelling for the ARN `<type>` segment of a Call Analytics job") even though the Medical Scribe family has since been wired in under `medical-scribe-job`.

## Evidence

Spec: `smithy_query(query="operation", operation="StartCallAnalyticsJob")` shows input member `Tags` (TagList, len 1..200); `smithy_query(query="shape", shape="CallAnalyticsJob")` shows output member `Tags` ("The tags, each in the form of a key:value pair, assigned to the specified call analytics job") — a Transcribe resource that carries tags, and the service's tagging API (`TagResource`/`UntagResource`/`ListTagsForResource`, ARN form `arn:partition:transcribe:region:account:resource-type/resource-id`) is the only way to read or change them after the start. AWS's own resource-type spelling for this family is `analytics-job`, the same hyphenation family as the already-supported `analytics-category` / `medical-scribe-job`.

Handler/state: `src/state.rs` — `ResourceFamily` (lines ~417-467) enumerates eight families and has none for Call Analytics jobs; `ComAmazonawsTranscribeTranscribeState::tags` / `tags_mut` (lines ~807-889) match on that enum only, so any Call Analytics job ARN yields `None`. `src/ops/list_tags_for_resource.rs` lines 185-198, `src/ops/tag_resource.rs` lines 178-180 and `src/ops/untag_resource.rs` lines 186-196 all render that `None` as `BadRequestException` + `not_found_message(resource_kind(..))`. Tags ARE stored: `src/ops/start_call_analytics_job.rs` line 368 (`tags: input.tags.unwrap_or_default()`) and reported by `CallAnalyticsJobRecord::to_job` (`src/state.rs` line 2074).

Tests: `tests/all/tagging_resource_families.rs` line 587 pushes the spelling `"call-analytics-job"` into the list of resource types that "name no resource family of this service at all" and asserts all three operations REFUSE it; no test exercises the AWS spelling `analytics-job` against an existing job, so no test asserts the correct behavior (the existing test encodes the divergence and would need updating with the fix).

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.StartCallAnalyticsJob` and body {"CallAnalyticsJobName":"ca-1","Media":{"MediaFileUri":"s3://DOC-EXAMPLE-BUCKET/call.wav"},"DataAccessRoleArn":"arn:aws:iam::123456789012:role/Admin","Tags":[{"Key":"team","Value":"speech"}]} — 200, and the response echoes Tags from the stored record.
2. POST / with `X-Amz-Target: Transcribe.ListTagsForResource` and body {"ResourceArn":"arn:aws:transcribe:us-east-1:<emulator-account>:analytics-job/ca-1"} — AWS answers 200 with Tags [{team, speech}]; the emulator answers a client error with Message "The requested resource couldn't be found. Check the resource name and try your request again."
3. POST / with `X-Amz-Target: Transcribe.TagResource` and body {"ResourceArn":"arn:aws:transcribe:us-east-1:<emulator-account>:analytics-job/ca-1","Tags":[{"Key":"env","Value":"prod"}]} — AWS answers 200 and the tag is then visible via ListTagsForResource and GetCallAnalyticsJob; the emulator refuses with the same not-found client error and stores nothing.
4. POST / with `X-Amz-Target: Transcribe.UntagResource` and body {"ResourceArn":"arn:aws:transcribe:us-east-1:<emulator-account>:analytics-job/ca-1","TagKeys":["team"]} — AWS answers 200 and `GetCallAnalyticsJob` no longer reports the key; the emulator refuses and the tag set is unchanged.

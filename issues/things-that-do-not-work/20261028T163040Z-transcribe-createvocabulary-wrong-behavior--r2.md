Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ad2e1ef07e763a71f3a6bfc5f25b7c81e14ebd76

# Creates and starts store a Tags list with repeated keys verbatim, breaking the one-value-per-key rule TagResource enforces

_Filed: 2026-10-28T16:30:40Z_
_Operation: CreateVocabulary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ad2e1ef07e763a71f3a6bfc5f25b7c81e14ebd76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every create/start handler of this crate stores the request's `Tags` list into the record exactly as it arrived, with no per-key normalisation. `TagResource` — the other writer of the very same tag sets, reached through the same `state::tags_mut` lookup — states and implements the opposite rule: one value per key, where a key already on the resource has its value REPLACED rather than appended beside.

So a create carrying two entries with the same key stores both, and `ListTagsForResource` then reports one key twice — a state `TagResource` says is not expressible. Worse, the two writers then disagree about that record: a later `TagResource` for that key updates only the FIRST stored copy (`merged.iter_mut().find(..)` stops at the first match), so the resource keeps reporting the stale second copy beside the new value, and a client that reads the tag set back cannot tell which value is in force. The same unnormalised store happens in all nine handlers that accept `Tags` (the five creates and the four starts), so every family's tag set can be corrupted this way.

The fix is for the creates/starts to apply the same per-key merge `TagResource` already implements (or a shared helper) before storing `input.tags`; `UntagResource` already removes every copy of a key (`tags.retain(..)`), so only the write side is inconsistent.

## Evidence

The rule this crate states and enforces on one path — services/service-com-amazonaws-transcribe-transcribe/src/ops/tag_resource.rs:193-202:
```rust
        // "a key:value pair": one value per key, so a tag whose key is already on the
        // resource replaces that key's value rather than being added beside it. The
        // merge is computed first and committed below.
        let mut merged = tags.clone();
        for tag in input.tags {
            match merged.iter_mut().find(|stored| stored.key == tag.key) {
                Some(stored) => stored.value = tag.value,
                None => merged.push(tag),
            }
        }
```

The same tag set written WITHOUT that rule — services/service-com-amazonaws-transcribe-transcribe/src/ops/create_vocabulary.rs:388:
```rust
                tags: input.tags.unwrap_or_default(),
```
and identically at create_vocabulary_filter.rs:370, create_call_analytics_category.rs:286, create_medical_vocabulary.rs:329, create_language_model.rs:295, start_transcription_job.rs:645, start_medical_transcription_job.rs:490, start_medical_scribe_job.rs:471, start_call_analytics_job.rs:490.

Both copies then surface on the read path — services/service-com-amazonaws-transcribe-transcribe/src/ops/list_tags_for_resource.rs:218: `tags: Some(tags.clone()),`.

The existing test pins the rule for the TagResource path only, so nothing pins the create path — services/service-com-amazonaws-transcribe-transcribe/tests/all/tagging_resource_families.rs:452-453:
```rust
        // A second merge of the same keys and values is idempotent: one value per key,
        // so nothing is added beside what is already there.
```

## Steps to Reproduce

1. CreateVocabulary with VocabularyName="v1", LanguageCode="en-US", Phrases=["a"] and Tags=[{"Key":"env","Value":"a"},{"Key":"env","Value":"b"}]
2. ListTagsForResource with ResourceArn=arn:aws:transcribe:<region>:<account>:vocabulary/v1 — the answer carries the key "env" TWICE, which tag_resource.rs:193-195 says cannot happen
3. TagResource on that ARN with Tags=[{"Key":"env","Value":"c"}]
4. ListTagsForResource again — the answer now carries env=c AND the stale env=b, two values for one key

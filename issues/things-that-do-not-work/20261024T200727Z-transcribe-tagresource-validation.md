Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# TagResource accepts tag keys in the reserved aws: namespace instead of refusing them

_Filed: 2026-10-24T20:07:27Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler validates the ResourceArn and the merged tag count, then stores whatever keys the request carried. Nothing checks the reserved `aws:` prefix.

Expected (AWS): `aws:` is reserved for AWS-generated/system tags. A TagResource request carrying a key such as `aws:owner` is refused with a client error (Transcribe reports request-validation failures as BadRequestException) and the resource's tag set is left untouched.

Actual (emulator): the call answers HTTP 200 and the reserved key is merged into the stored tag set, after which `ListTagsForResource` reports `aws:owner` as a user tag on the resource — a tag state AWS cannot be driven into through this API. Because `TagResource` is also how the merge/overwrite path works, a reserved key can even overwrite an existing value, and `UntagResource` is then the only way to get rid of it.

The same hole exists on the `Tags` member of the create/start operations (CreateVocabulary, StartTranscriptionJob, …), which all persist `input.tags` verbatim; the observable fix point is the shared tag-key rule.

## Evidence

Spec: `TagKey` is modelled as a bare string with `length {min 1, max 128}` and NO pattern (smithy_query shape=TagKey/Tag), so the boundary validator cannot refuse a reserved key — the handler is the only place it can be refused. Well-established AWS behavior: the `aws:` tag-key prefix is reserved for AWS use and user tagging calls that supply it are rejected service-wide; TagResource models BadRequestException ("Your request didn't pass one or more validation tests") for exactly this kind of refusal. Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/tag_resource.rs lines 177-205 — ARN lookup, then `for tag in input.tags { ... merged.push(tag) }` with only a `MAX_TAGS_PER_RESOURCE` ceiling; no key inspection anywhere. Tests: tests/all/tagging_resource_families.rs exercises tag/untag/list across every family and tests/all/vocabulary_delete_and_tag_sets.rs covers tag-set merging, but neither file contains an `aws:`-prefixed key, so no test asserts the refusal.

## Steps to Reproduce

1. POST / X-Amz-Target: Transcribe.CreateVocabulary {"VocabularyName":"tagme","LanguageCode":"en-US","Phrases":["Aardvark"]}
2. POST / X-Amz-Target: Transcribe.TagResource {"ResourceArn":"arn:aws:transcribe:us-east-1:<emulator-account>:vocabulary/tagme","Tags":[{"Key":"aws:owner","Value":"me"}]} — the emulator answers 200; AWS refuses the request because the key uses the reserved aws: prefix
3. POST / X-Amz-Target: Transcribe.ListTagsForResource with the same ARN — the emulator reports the `aws:owner` tag as a user tag on the vocabulary

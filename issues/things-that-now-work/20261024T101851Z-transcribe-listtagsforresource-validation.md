Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: c677c98df9a3a2556a69e92857be9faa7b7124d4

# Tagging ARNs resolve regardless of their region and account segments

_Filed: 2026-10-24T10:18:51Z_
_Operation: ListTagsForResource | Class: validation_

> _Note: this issue was identified against the code base at commit `c677c98df9a3a2556a69e92857be9faa7b7124d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::parse_resource_arn` destructures a Transcribe ARN and then discards the partition, region and account fields, keying the lookup on `<type>/<name>` alone. Any ARN whose type/name pair matches a stored record is served, no matter which region or account the ARN claims.

Expected (AWS): an ARN is region- and account-qualified. `ListTagsForResource` with an ARN naming a different Region than the one the resource lives in, or a different account id, does not resolve to that resource — the call is refused (not-found / bad-request), not answered with the local resource's tag set.

Actual (emulator): `ListTagsForResource` with `arn:aws:transcribe:eu-west-1:000000000000:vocabulary/foo` returns the tags of the `foo` vocabulary created under us-east-1, and `arn:aws:transcribe:us-east-1:999999999999:vocabulary/foo` does the same for an account that holds nothing. Because `TagResource` and `UntagResource` share the same helper, the write paths mutate the local record through a foreign ARN as well, so a caller can tag/untag a resource with an ARN AWS would reject.

## Evidence

Spec: `TranscribeArn`'s own pattern pins the region and 12-digit account segments (^arn:aws(-[^:]+)?:transcribe:[a-zA-Z0-9-]*:[0-9]{12}:...$ — smithy_query operation=ListTagsForResource/TagResource), and the resource records already store the region they were created in (`VocabularyRecord.region`, used to build the resource's own ARN via `crate::state::resource_arn`), so the ARN the service hands out is region-specific. Handler/state: services/.../src/state.rs `parse_resource_arn` binds `_partition`, `_region` and `_account` and ignores them; `tags()` / `tags_mut()` look up only `resource_type` + `name`; services/.../src/ops/list_tags_for_resource.rs lines 169-198, tag_resource.rs lines 168-180 and untag_resource.rs lines 170-196 all go through that helper. Tests: tests/all/tagging_resource_families.rs and vocabulary_file_uri.rs build ARNs with the emulator's own region and account only — no row exercises a mismatched region or account, so nothing asserts the refusal.

## Steps to Reproduce

1. Call Transcribe.CreateVocabulary {"VocabularyName":"foo","LanguageCode":"en-US","Phrases":["Aardvark"]} against the default region (us-east-1) and tag it with TagResource using its own ARN.
2. Call Transcribe.ListTagsForResource with {"ResourceArn":"arn:aws:transcribe:eu-west-1:<emulator-account>:vocabulary/foo"} — the emulator answers 200 with the tag set of the us-east-1 vocabulary; AWS does not resolve a resource in another Region.
3. Call Transcribe.ListTagsForResource with {"ResourceArn":"arn:aws:transcribe:us-east-1:999999999999:vocabulary/foo"} — the emulator answers 200 with the same tags for an account that holds nothing.
4. Call Transcribe.TagResource / UntagResource with either foreign ARN — the emulator mutates the local record's tag set.

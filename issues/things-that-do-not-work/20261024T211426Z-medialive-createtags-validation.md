Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# CreateTags accepts tag keys in the reserved `aws:` namespace

_Filed: 2026-10-24T21:14:26Z_
_Operation: CreateTags | Class: validation_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS reserves the `aws:` tag-key prefix for system tags: a caller may not create or overwrite a tag whose key begins with `aws:`, and MediaLive refuses such a request with `BadRequestException` (400). The emulator's tag writer validates only the tag-count ceiling, the key length and the value length, so `CreateTags` with `{"aws:cloudformation:stack-name": "x"}` is accepted with 204 and the key then shows up in `ListTagsForResource` and in the resource's own describe output as if the caller owned it. `DeleteTags` likewise removes an `aws:`-prefixed key rather than refusing.

Expected: 400 BadRequestException for any key beginning with `aws:` (case-insensitively), with the resource's stored tags left untouched. Actual: 204 and the reserved key is stored.

## Evidence

`ops/create_tags.rs` lines 194-231: the only refusals mapped from `insert_resource_tags` are `NoSuchResource`, `TooManyTags`, `KeyTooLong` and `ValueTooLong` — there is no reserved-prefix branch. A `grep` for `aws:`/`reserved` across `services/service-com-amazonaws-medialive-medialive/src` finds only documentation and ARN-example text, never a tag-key check. `Tags` is a modelled map member of `CreateTagsRequest` and the operation models `BadRequestException` (400). Grounded in the AWS-wide reserved tag-namespace rule. No test asserts the refusal.

## Steps to Reproduce

1. CreateChannel (or CreateInput) and note the resource ARN
2. POST /prod/tags/{resourceArn} with body {"tags": {"aws:foo": "bar"}}
3. Observe 204 instead of 400 BadRequestException
4. GET /prod/tags/{resourceArn} and observe the reserved key `aws:foo` reported among the resource's tags

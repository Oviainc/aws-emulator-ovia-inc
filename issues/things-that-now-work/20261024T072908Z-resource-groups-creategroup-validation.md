Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5bfe48ddd26ea9415acafc303bdb706a28d090a8

# CreateGroup stores tag keys with the reserved aws: prefix instead of refusing them

_Filed: 2026-10-24T07:29:08Z_
_Operation: CreateGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `5bfe48ddd26ea9415acafc303bdb706a28d090a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `aws:` tag-key prefix is reserved for AWS system tags across every AWS tagging surface, and Resource Groups refuses a caller-supplied key that uses it with BadRequestException (HTTP 400) — the same class of refusal the handler already produces for a reserved group NAME prefix. CreateGroup here copies `input.tags` straight into `GroupRecord::tags` with no key inspection, so a group can be created carrying `aws:...` (or `awsApplication`, the key Resource Groups itself manages for application groups) tags, and those tags are then echoed back by CreateGroup and persisted for later readers. Expected: 400 BadRequestException for a tag key beginning with `aws:` (any capitalization), and nothing stored. Actual: 200, the tag stored and reported.

## Evidence

Spec/well-established behaviour: AWS reserves the `aws:` tag-key prefix for system tags and tagging APIs reject user-supplied keys using it; the model's own `ApplicationTagKey` (`^awsApplication$`) shows that Resource Groups owns `awsApplication` as a service-managed tag, and `CreateGroupInput$Tags` is documented only as 'The tags to add to the group'. Handler: `src/ops/create_group.rs` line ~301 `tags: input.tags.unwrap_or_default().into_iter().collect()` — no key validation anywhere in the handler (contrast the reserved-prefix check it does apply to `Name`, lines 227-241). Tests: `tests/all/resource_group_lifecycle.rs` line 156 only round-trips ordinary tags; no test covers reserved tag keys.

## Steps to Reproduce

1. POST /groups with {"Name":"reserved-tags","Tags":{"aws:createdBy":"me"}}.
2. Observe 200 with the tag echoed in the response instead of 400 BadRequestException.
3. GET /resources/<group arn>/tags (or re-create and inspect state) and observe the reserved key persisted on the group.

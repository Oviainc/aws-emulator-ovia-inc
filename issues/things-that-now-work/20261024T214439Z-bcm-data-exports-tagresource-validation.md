Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 4af528f360b03ab2b05e9ad5a75be6ebefdc29e9

# TagResource accepts reserved `aws:`-prefixed tag keys instead of refusing them

_Filed: 2026-10-24T21:44:39Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `4af528f360b03ab2b05e9ad5a75be6ebefdc29e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (real service): AWS reserves the `aws:` tag-key prefix for its own use across every taggable service — a caller-supplied tag whose `Key` begins with `aws:` is refused with the modelled `ValidationException` (HTTP 400) and no tag is stored. The same rule applies to the `ResourceTags` a `CreateExport` carries.

Actual (emulator): the key is stored verbatim and reported back. `TagResource` with `ResourceTags: [{"Key": "aws:cloudformation:stack-name", "Value": "x"}]` answers 200, and a following `ListTagsForResource` reports that tag as one of the export's tags, so a client that relies on the reserved namespace being rejected (and on the service-managed tags it implies) sees state AWS would never produce. `UntagResource` likewise removes such a key, where AWS refuses to let a caller edit or delete an `aws:`-prefixed tag.

The tag-reading path validates only key UNIQUENESS (duplicate key → 400) and the total count against the 200-tag bound; the key's own content is never examined beyond the model's `@length(1,128)`.

## Evidence

Spec/semantics: `ResourceTag$Key` targets `ResourceTagKey` with `@length(min 1, max 128)` and NO `@pattern` (smithy_query shape `ResourceTag`), so the model cannot express the restriction and it has to be enforced in the handler. The restriction itself is well-established AWS tagging behaviour — the `aws:` prefix is reserved for AWS and a user cannot create, edit or delete tags that use it; services that model `TagResource`/`UntagResource` answer the attempt with their own validation error (here `ValidationException`, @httpError 400, which `TagResource`, `UntagResource` and `CreateExport` all declare).

Handler: `src/ops/tag_resource.rs` lines 168-212 — the only checks between deserialization and `stored.tags.extend(submitted)` are `crate::state::tag_map` (duplicate keys only, `src/state.rs` `tag_map`/`duplicate_tag_key`), `validate_taggable_resource_arn` (ARN account) and the `MAX_RESOURCE_TAGS` count. `src/ops/untag_resource.rs` lines 191-196 removes whatever keys the request names with no prefix check. `src/ops/create_export.rs` line 195 reuses the same `tag_map`, so a create can seed a reserved key too.

Tests: `grep -n 'aws:[a-z]|reserved'` over the crate's `tests/` matches only ARN literals — `tests/all/tagging.rs`, `tests/all/resource_tagging.rs` and `tests/all/tagging_arn_validation.rs` cover duplicate keys, the 200-tag bound, ARN account validation and not-found, and no test asserts anything about a reserved tag key, so this is uncovered rather than deliberately pinned.

Not a duplicate: the open backlog for this service holds CreateExport (validation/wrong-behavior/event-propagation), GetExecution (wrong-behavior) and UntagResource (validation, ARN account) entries; none of them mentions the reserved tag-key prefix, and no (TagResource, validation) issue exists.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBillingAndCostManagementDataExports.CreateExport` and a valid export definition; keep the returned `ExportArn`.
2. POST / with `X-Amz-Target: AWSBillingAndCostManagementDataExports.TagResource` and body {"ResourceArn": "<ExportArn>", "ResourceTags": [{"Key": "aws:billing:owner", "Value": "finance"}]}.
3. Observe HTTP 200 with an empty body where AWS answers HTTP 400 `ValidationException` for the reserved `aws:` key and stores nothing.
4. POST / with `X-Amz-Target: AWSBillingAndCostManagementDataExports.ListTagsForResource` and {"ResourceArn": "<ExportArn>"} — the reserved key is reported among `ResourceTags`.
5. Repeat step 2 against `CreateExport` by putting the same `aws:`-prefixed pair in the create's `ResourceTags`: the export is created with it.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 14fef70bd15ca29a08f279a21ebf0b06fea39359

# TagResource accepts reserved `aws:`-prefixed tag keys instead of refusing them

_Filed: 2026-10-24T00:32:02Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `14fef70bd15ca29a08f279a21ebf0b06fea39359`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the `aws:` prefix on a tag key is reserved for AWS system tags; a caller may not create or modify such a tag, and a TagResource call carrying a key like `aws:cloudformation:stack-name` is refused (4xx, no tag applied). This is an AWS-wide tagging rule enforced by every taggable service's tagging surface.

Actual: the handler applies the request's map wholesale after only the modelled key/value length+pattern checks and the 50-tag capacity check. The modelled TagKey pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$ admits ':' , so `aws:anything` passes the centralized walk, is inserted into the group's tag map, and is then reported by ListTagsForResource as if it were a user tag. The same hole exists on CreateInvestigationGroup's `tags` map (same shared tag write path) and on UntagResource, which will happily delete an `aws:`-prefixed key.

## Evidence

Spec: Tags -> TagKey has length 1..128 and pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$ (smithy_query shape=Tags), so a reserved-prefix key is NOT excluded by any modelled constraint and must be refused by the operation. Well-established behavior: AWS reserves the `aws:` tag-key prefix for system tags — callers cannot create, edit or delete them — and tagging APIs reject attempts with a 400-class error. Handler: services/service-com-amazonaws-aiops-aiops/src/ops/tag_resource.rs lines ~176-186 (`for (key, value) in input.tags { group.tags.insert(key, value); }`) applies every key with no prefix check; crate::state::tag_capacity_rejection is the only extra rule. Test coverage: grepping tests/all for an `aws:`-prefixed tag key finds none (resource_tags.rs covers per-key merge/removal, the 50-tag cap and the frontend 404 only), so nothing asserts this today.

## Steps to Reproduce

1. Create an investigation group and note its ARN
2. POST /tags/<url-encoded group ARN> with {"tags":{"aws:cloudformation:stack-name":"mine"}} -> emulator answers 200
3. GET /tags/<url-encoded group ARN> -> the reserved key is reported back as a user tag; real AWS refuses the write with a 400-class error and the tag never appears
4. DELETE /tags/<arn>?tagKeys=aws:cloudformation:stack-name -> emulator answers 200 and removes it, another write AWS does not permit

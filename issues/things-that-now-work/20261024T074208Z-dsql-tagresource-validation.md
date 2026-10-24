Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# TagResource accepts reserved `aws:`-prefixed tag keys instead of rejecting them

_Filed: 2026-10-24T07:42:08Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: tag keys beginning with the reserved `aws:` prefix are owned by AWS and cannot be created or modified by a caller; every AWS tagging API refuses such a request with a 4xx validation-class error (for this service, `ValidationException`), and the reserved tags do not count against the per-resource tag limit. A `TagResource` carrying {"aws:cloudformation:stack-name": "x"} must be refused in full and must leave the resource's tags unchanged.

Actual: `tag_resource.rs` merges whatever `tags` the request carries straight into the resource's tag map after only two checks — that the `resourceArn` parses as a DSQL ARN and that the merge would not exceed `MAX_TAGS_PER_RESOURCE` — so `aws:`-prefixed keys are accepted with 200 and are then reported by `ListTagsForResource` and `GetCluster` as ordinary customer tags (and can be removed again by `UntagResource`, which AWS also forbids for reserved keys). `TagKey`'s modeled pattern `^[a-zA-Z0-9_.:/=+\-@ ]*$` admits `:`, so the boundary validator passes these keys through and nothing behind it looks at the prefix. The same unchecked path is taken by the `tags` member of `CreateCluster` and `CreateStream`, which store the map verbatim.

## Evidence

Spec: `smithy_query(query="shape", shape="TagMap,TagKey")` — TagMap len 0..200, TagKey len 1..128 pattern `^[a-zA-Z0-9_.:/=+\-@ ]*$` (the `:` needed for an `aws:` prefix is allowed by the pattern, so the model cannot express this rule and the service must enforce it). Well-established AWS tagging semantics: the `aws:` prefix is reserved for AWS use — user attempts to create, edit or delete such a tag are rejected, and reserved tags are excluded from the tags-per-resource quota.

Handler: `src/ops/tag_resource.rs` lines 191-257 — ARN parse, resource lookup, quota arithmetic over `input.tags.keys()`, then `for (key, value) in input.tags { held.insert(key, value); }`. No key-prefix inspection anywhere in the crate (no occurrence of a reserved-prefix constant in `src/`).

Tests: `tests/all/tag_resource.rs`, `tests/all/resource_tags.rs` and `tests/all/error_contract.rs` cover the ARN classes, the 200-tag ceiling (`BadRequestException` on the input map, `ServiceQuotaExceededException` on the merge) and the merge/remove semantics; a repo-wide grep of the test tree for `aws:`-prefixed tag keys finds none, so no test pins either acceptance or refusal.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false} -> note `identifier` and `arn`.
2. POST /tags/{arn} with {"tags": {"aws:cloudformation:stack-name": "mine"}}.
3. Emulator answers 200 and persists the key; expected is a 400 ValidationException stating that tag keys beginning with `aws:` are reserved, with the resource's tags unchanged.
4. GET /tags/{arn} -> the emulator reports {"aws:cloudformation:stack-name": "mine"} as a customer tag (and GetCluster reports it under `tags`).
5. DELETE /tags/{arn}?tagKeys=aws:cloudformation:stack-name -> 200, the reserved key is removed, which AWS also refuses.

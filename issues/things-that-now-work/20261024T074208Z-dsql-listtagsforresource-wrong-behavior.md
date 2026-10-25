Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# A resourceArn naming another Region or account resolves to the local cluster, so tags of a resource this endpoint does not serve are read and written

_Filed: 2026-10-24T07:42:08Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: Aurora DSQL is a regional service and the tagging operations address a resource by ARN. An ARN whose Region segment is not the Region the request is signed for, or whose account segment is not the caller's account, names a resource this endpoint does not serve: AWS answers an error (a 404 `ResourceNotFoundException` / 400 `ValidationException` class refusal), never a successful read or write against a different resource.

Actual: `crate::state::parse_resource_arn` deliberately discards the partition and account fields and merely reports the Region without matching it, and `resource_tags` / `resource_tags_mut` then resolve purely on the resource type plus the resource id. So `arn:aws:dsql:eu-west-1:999988887777:cluster/<id of a cluster created in us-east-1>` answers 200 with that cluster's tags, and because `TagResource` and `UntagResource` share the same resolution, the same cross-Region / cross-account ARN MUTATES the local cluster's tags. The ARN the emulator itself mints for the cluster carries a specific Region and the emulator account, so an ARN differing in those fields denotes something else — yet it is treated as the same resource.

## Evidence

Spec: `smithy_query(query="operation", operation="ListTagsForResource,TagResource,UntagResource")` — `resourceArn` (shape `Arn`, len 1..1011, pattern `^arn:.+$`, @httpLabel, required), errors `ResourceNotFoundException` (404) / `ServiceQuotaExceededException`; `ClusterArn`'s pattern pins the Region and 12-digit account segments (`^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}$`), so those fields are part of the resource's identity. Well-established AWS behavior: a regional endpoint does not act on another Region's or another account's resource ARN.

Handler/state: `src/state.rs` `parse_resource_arn` (around lines 742-764) with its own comment "The partition and account fields take no part in the lookup ... The region is reported rather than matched on"; `resource_tags` / `resource_tags_mut` key only on `(resource_type, resource_id)`. Readers: `src/ops/list_tags_for_resource.rs` lines 163-191, `src/ops/tag_resource.rs` lines 195-253, `src/ops/untag_resource.rs` lines 191-220.

Tests: `tests/all/resource_tags.rs` lines 292-353 pin the 404 classes (unknown cluster id, retired cluster, unknown DSQL resource type, colon-form ARN) and lines 384-417 the 400 classes (another service's ARN, uppercase `DSQL`, truncated ARNs). No case varies the Region or the account segment, so this behavior is unpinned.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false, "tags": {"env": "prod"}} signed for us-east-1 -> note `identifier`; the minted ARN is arn:aws:dsql:us-east-1:<emulator-account>:cluster/<identifier>.
2. GET /tags/arn:aws:dsql:eu-west-1:<emulator-account>:cluster/<identifier> -> emulator answers 200 {"tags":{"env":"prod"}}; expected is an error, since no cluster with that id exists in eu-west-1.
3. GET /tags/arn:aws:dsql:us-east-1:999988887777:cluster/<identifier> -> emulator answers 200 with the same tags, for a foreign account id.
4. POST /tags/arn:aws:dsql:eu-west-1:999988887777:cluster/<identifier> with {"tags": {"owner": "someone-else"}} -> 200, and GET /tags/<the real us-east-1 ARN> now reports `owner`, i.e. a cross-Region/cross-account ARN mutated the local resource.
5. DELETE /tags/arn:aws:dsql:eu-west-1:999988887777:cluster/<identifier>?tagKeys=env -> 200, removing a tag from the local cluster.

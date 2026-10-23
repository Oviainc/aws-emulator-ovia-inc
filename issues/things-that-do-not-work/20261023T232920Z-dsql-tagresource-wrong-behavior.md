Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# TagResource cannot tag a stream: a stream ARN answers 404 while ListTagsForResource/UntagResource resolve the same ARN

_Filed: 2026-10-23T23:29:20Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A CDC stream is a tagged resource of this service: CreateStream accepts `tags`, GetStream reports them, and the stream ARN this service mints (`arn:aws:dsql:{region}:{account}:cluster/{cluster}/stream/{stream}`) is handed back to the caller. `ListTagsForResource` and `UntagResource` both resolve that ARN through the shared `ComAmazonawsDsqlDsqlState::resource_tags{,_mut}` helper, which walks `cluster/{cluster}/stream/{stream}` into the stream's own tag map.

Expected: `POST /tags/{streamArn}` merges the supplied tags into that stream's tag map, and a following ListTagsForResource on the same ARN reports them.

Actual: TagResource does NOT use the shared resolver. It matches only `resource_type == "cluster"` and then looks the whole remainder up as a cluster id (`state.clusters.get_mut(&resource.resource_id)`), so for a stream ARN the id is the compound `{cluster}/stream/{stream}` and no cluster matches → HTTP 404 ResourceNotFoundException. The emulator is therefore self-inconsistent: tags set at CreateStream time can be listed and removed, but no tag can ever be added to an existing stream. The stale comment in the handler (lines 206-213) claims ListTagsForResource cannot resolve a stream either, which is no longer true.

## Evidence

Spec: TagResource input `resourceArn` is only constrained `^arn:.+$` and the model gives streams their own tags (`CreateStream.tags`, `GetStream.tags`, `StreamArn` nested under the cluster ARN). Handler: `src/ops/tag_resource.rs` lines 201-225 (cluster-only match, 404 otherwise) versus `src/ops/untag_resource.rs` lines 204-213 and `src/ops/list_tags_for_resource.rs` lines 175-184, which both call `resource_tags{,_mut}` → `addressed_resource` in `src/state.rs` lines 588-640 (explicitly handles the stream sub-path). Tests: `tests/all/resource_tags.rs::a_stream_arn_addresses_the_streams_own_tags` exercises list+untag on a stream ARN but never TagResource; `tests/all/tag_resource.rs` has no stream case.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false}; wait for ACTIVE.
2. POST /stream/{clusterId} with a Kinesis targetDefinition, ordering UNORDERED, format JSON, tags {"env":"test"}; record the returned stream `arn`.
3. GET /tags/{streamArn} → 200 with {"env":"test"} (the stream ARN resolves).
4. POST /tags/{streamArn} with {"tags": {"owner": "team-a"}} → emulator answers 404 ResourceNotFoundException. Expected 200 and the tag persisted.
5. GET /tags/{streamArn} → still only {"env":"test"}.

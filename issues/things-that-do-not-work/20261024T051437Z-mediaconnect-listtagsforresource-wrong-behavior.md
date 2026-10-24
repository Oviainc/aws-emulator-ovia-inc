Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# Tag operations resolve only flow ARNs: a flow's output/source/entitlement ARN is answered 404

_Filed: 2026-10-24T05:14:37Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tag operations take any MediaConnect `ResourceArn` ("Associates the specified tags to a resource with the specified `resourceArn` in the current region"), and MediaConnect's own request shapes prove that a flow's outputs, sources and entitlements are taggable resources with their own ARNs (`AddOutputRequest$OutputTags`, `SetSourceRequest$SourceTags`, `GrantEntitlementRequest$EntitlementTags`; `Output$OutputArn`, `Source$SourceArn`, `Entitlement$EntitlementArn`).

Expected: `GET /tags/{ResourceArn}` for an existing output/source/entitlement ARN answers 200 with that sub-resource's tag map (empty when it has none), and `POST`/`DELETE` on the same ARN add and remove its tags.

Actual: the resolver behind all three operations looks the ARN up in the FLOW map only (`tags_of`/`tags_of_mut` return `self.flows.get(resource_arn)`), so any non-flow ARN — including an output/source/entitlement ARN this very emulator minted and returned — is reported as a resource whose tags do not exist: 404 NotFoundException for the read, and for TagResource/UntagResource as well. There is nowhere to store such tags either, which is the write-side half of the same gap.

## Evidence

Spec: `smithy_query(query="doc",shape="TagResource")` — "Associates the specified tags to a resource with the specified `resourceArn` in the current region"; the sub-resource tag members and ARNs listed above (`smithy_query(query="shape",shape="AddOutputRequest")`, `...="Output"`).

Handler/state: `src/ops/list_tags_for_resource.rs:165-178` and `src/ops/tag_resource.rs:208-211`, `src/ops/untag_resource.rs:182-185` all go through `store.tags_of(...)`/`tags_of_mut(...)`, defined at `src/state.rs:2180-2188` as `self.flows.get(resource_arn)` with the comment "A flow is the only resource whose TAGS this emulator persists".

Test coverage: `tests/all/resource_tags.rs` asserts only the flow-ARN behaviour plus the 404 for an unknown ARN; no case uses an output/source/entitlement ARN, so nothing pins the current refusal as correct.

## Steps to Reproduce

1. POST /v1/flows with one source and one output, and note `flow.outputs[0].outputArn` from the 201 response
2. GET /tags/{outputArn} — observe 404 NotFoundException, where AWS answers 200 with the output's tag map
3. POST /tags/{outputArn} with {"tags":{"env":"prod"}} — observe 404 NotFoundException, where AWS answers 204 and the tag is then readable
4. Repeat with the flow's sourceArn and an entitlementArn from GrantFlowEntitlements — same 404

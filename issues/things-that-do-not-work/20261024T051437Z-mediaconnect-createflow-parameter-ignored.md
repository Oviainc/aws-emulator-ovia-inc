Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# Sub-resource tag maps (OutputTags, SourceTags, EntitlementTags) are accepted and silently discarded

_Filed: 2026-10-24T05:14:37Z_
_Operation: CreateFlow | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateFlow (and the sibling add/grant operations that reuse the same request shapes) accepts a tag map for each sub-resource it creates: `AddOutputRequest$OutputTags` ("the key-value pairs that can be used to tag and organize the output"), `SetSourceRequest$SourceTags` ("... the source") and `GrantEntitlementRequest$EntitlementTags` ("... the entitlement"). Each of those sub-resources has its own ARN (`Output$OutputArn`, `Source$SourceArn`, `Entitlement$EntitlementArn`), which is what `ListTagsForResource`/`TagResource`/`UntagResource` address a resource by.

Expected: the pairs are persisted with the sub-resource they were supplied for, so `GET /tags/{outputArn}` (and the source / entitlement equivalents) answers 200 with exactly those tags, and `UntagResource` on that ARN removes them.

Actual: the three maps are parsed off the wire and then dropped on the floor — `StoredOutput`, `StoredSource` and the minted `Entitlement` have no tag field, and `src/state.rs` states the choice explicitly ("`SetSourceRequest$SourceTags` is not stored", "`GrantEntitlementRequest$EntitlementTags` is not stored"). The create answers 201 as if the tags had been applied, and every later read reports no tags at all for the ARN they were attached to (the emulator's tag resolver only knows flow ARNs, so such a call is answered 404 NotFoundException). Note the inconsistency inside the crate: `AddMediaStreamRequest$MediaStreamTags` IS stored (`src/state.rs:1238-1243`) even though a media stream has no ARN to address it by, while the three ARN-bearing sub-resources' tags are thrown away.

The same dropped members reach the store through AddFlowOutputs, AddFlowSources and GrantFlowEntitlements, which materialise the identical request shapes through the same `crate::state` helpers, so one fix covers all four entry points.

## Evidence

Spec: `smithy_query(query="doc",shape="AddOutputRequest",member="OutputTags")` — "The key-value pairs that can be used to tag and organize the output"; `smithy_query(query="shape",shape="SetSourceRequest")` member `SourceTags`; `smithy_query(query="shape",shape="GrantEntitlementRequest")` member `EntitlementTags`; `TagResource` doc — "Associates the specified tags to a resource with the specified `resourceArn` in the current region", and the sub-resources each carry an ARN (`Output$OutputArn`, `Source$SourceArn`, `Entitlement$EntitlementArn`).

Handler/state: `src/ops/create_flow.rs:345-387` materialises outputs via `StoredOutput::from_add_request` and entitlements via `crate::state::mint_entitlement`; `src/state.rs:766-768` ("`SetSourceRequest$SourceTags` is not stored"), `src/state.rs:941-1002` (`StoredOutput` has no tag field and `from_add_request` ignores `output_tags`), `src/state.rs:2460-2487` ("`GrantEntitlementRequest$EntitlementTags` is not stored"). The wire types do carry the members (`src/ops/types/shared_types.rs:235,305,392`), so they are deserialized and then dropped.

Test coverage: none — `tests/all/resource_tags.rs` covers only flow-level `FlowTags`/TagResource/UntagResource; no test sends `outputTags`, `sourceTags` or `entitlementTags`.

## Steps to Reproduce

1. POST /v1/flows with {"name":"f-tags","source":{"protocol":"rtp","whitelistCidr":"10.0.0.0/16","sourceTags":{"team":"video"}},"outputs":[{"name":"o1","protocol":"rtp","destination":"10.1.1.1","port":5000,"outputTags":{"env":"prod"}}],"entitlements":[{"name":"e1","subscribers":["111122223333"],"entitlementTags":{"cost":"shared"}}]} — observe 201
2. GET /tags/{outputArn} using the `outputArn` from the response — observe 404 NotFoundException with no trace of {"env":"prod"}, where AWS answers 200 with that tag
3. Repeat for the returned sourceArn and entitlementArn — same result
4. POST /v1/flows/{FlowArn}/outputs with an output carrying "outputTags" and observe the same loss through the add path

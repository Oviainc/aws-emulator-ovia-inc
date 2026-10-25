Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0831f46a299ca00fdb6555dd0d1d0166298aa2f3

# Tagging operations resolve a resourceArn without the request Region or the DELETED state

_Filed: 2026-10-24T16:32:16Z_
_Operation: ListTagsForResource | Class: validation_

> _Note: this issue was identified against the code base at commit `0831f46a299ca00fdb6555dd0d1d0166298aa2f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The ARN-addressed tagging surface (ListTagsForResource, TagResource, UntagResource) resolves `resourceArn` by scanning EVERY stored environment for an exact ARN match, with no regard for the Region the request was signed for or the state the environment is in.

Expected (two symptoms of the same acceptance):
1. An environment is a regional resource: an ARN naming `arn:aws:evs:us-east-1:...:environment/env-x` is not addressable at the eu-central-1 endpoint. Real EVS refuses such a request (the ARN names no resource of the Region the request was signed for) — not with HTTP 200 and the tags of a resource that lives at another endpoint. Every other operation of this crate already enforces exactly that: `environment_in_region` filters by the request Region and `tests/all/connectors.rs` (lines 695-717) asserts that an environment of one Region is "not addressable from another Region" through `call_in_region`.
2. An environment that has reached `DELETED` is gone for addressing purposes: "The resource might not be specified correctly, or it may have a `state` of `DELETED`" (ResourceNotFoundException doc). `CreateEnvironmentConnector`, `CreateEntitlement`, `CreateEnvironmentHost`, `GetDepotUrl`, `AssociateEipToVlan` and `DisassociateEipFromVlan` all apply that reading with `.filter(|environment| environment.state != STATE_DELETED)`; the tagging surface does not, so a deleted environment can still be tagged and untagged and still reports tags.

Actual: `ListTagsForResource` answers 200 with the tags, and `TagResource` / `UntagResource` answer 200 and MUTATE the stored tag map (and re-stamp `modifiedAt`) of an environment that belongs to another Region or that is already DELETED.

## Evidence

Spec: an EVS environment is regional (its ARN carries the Region, and `ListEnvironments` "Lists the Amazon EVS environments in your account in the specified Region"); `ResourceNotFoundException` doc sanctions DELETED as not-found. Handler: src/ops/list_tags_for_resource.rs lines 159-170, src/ops/tag_resource.rs lines 179-191, src/ops/untag_resource.rs lines 160-172 — all call `state.environment_by_arn[_mut]`, which is src/state.rs lines 1917-1928: `self.environments.values().find(|e| e.environment_arn == resource_arn)` with no Region comparison and no state filter (the doc comment there argues "the ARN carries the Region, so matching it is already Region-scoped", which identifies the resource but never compares the ARN's Region with the endpoint's). None of the three handlers reads `crate::state::request_region(ctx.parts())` at all, unlike every id-addressed handler in the crate. Tests: tests/all/tagging_not_found.rs and tests/all/environment_lifecycle.rs (lines 960-1007, 1179-1217) only cover an ARN that names no environment at all and a happy-path tag/read-back; grep shows no tagging test using `call_in_region` and none tagging a DELETED environment, so nothing asserts either rule.

## Steps to Reproduce

1. Register a VPC and service access subnet in the shared EC2 store and call CreateEnvironment at the us-east-1 endpoint (termsAccepted=true, ten non-overlapping initialVlans); note the returned environmentId and build its ARN arn:aws:evs:us-east-1:<account>:environment/<environmentId>.
2. Call TagResource at a DIFFERENT endpoint (sign the request for eu-central-1) with that us-east-1 ARN and tags={"team":"platform"} — observe HTTP 200 and the tag applied, instead of a not-found refusal for an ARN that names no resource of eu-central-1.
3. Call ListTagsForResource at the eu-central-1 endpoint with the same ARN — observe HTTP 200 and the tags of the us-east-1 environment.
4. Back at us-east-1, call DeleteEnvironment and wait for the environment to reach DELETED, then call TagResource/UntagResource with its ARN — observe HTTP 200 and a mutated tag map on a DELETED environment, instead of the tagging not-found refusal the other operations answer for a DELETED environment.

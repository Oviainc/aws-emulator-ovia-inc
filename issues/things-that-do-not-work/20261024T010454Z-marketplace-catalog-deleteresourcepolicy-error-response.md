Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# DeleteResourcePolicy answers 200 for an ARN naming a resource the catalog does not hold; ResourceNotFoundException is unreachable

_Filed: 2026-10-24T01:04:54Z_
_Operation: DeleteResourcePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteResourcePolicy` models `ResourceNotFoundException` (HTTP 404), but the handler can never return it: after parsing the ARN it unconditionally removes the (possibly absent) policy-store entry and answers 200 with an empty body — even when the ARN names an entity or change set the store has no record of. Expected: 404 `ResourceNotFoundException` for an ARN naming a resource this account/catalog does not hold, consistent with `PutResourcePolicy`/`TagResource`/`ListTagsForResource` on the same ARN.

## Evidence

Spec: `DeleteResourcePolicy` declares `ResourceNotFoundException` (httpError 404); its documentation is "Deletes a resource-based policy on an entity that is identified by its resource ARN" — the entity is part of the addressed resource.
Handler: `src/ops/delete_resource_policy.rs` lines ~160-176 call `store.settle(now)` then `store.resource_policies.remove(&resource.policy_key())` and return `Ok(DeleteResourcePolicyOutput {})`; there is no `holds_resource` check and no `ResourceNotFoundException` in the file.
Tests: `tests/all/resource_policy_and_tagging.rs` lines 733-742 assert 200 for an ARN naming an entity that was never created, so no test asserts the 404.

## Steps to Reproduce

1. Start the emulator with an empty store
2. DELETE /DeleteResourcePolicy?resourceArn=arn:aws:aws-marketplace:us-east-1:000000000000:AWSMarketplace/SaaSProduct/prod-c3d4e5f6g7h8i
3. Observe 200 with an empty body; AWS answers 404 ResourceNotFoundException for an ARN whose entity does not exist (as PutResourcePolicy already does in this emulator)

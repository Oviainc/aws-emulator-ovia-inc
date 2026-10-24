Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# GetResourcePolicy answers 200 with a fabricated "{}" policy for a resource that does not exist; ResourceNotFoundException is unreachable

_Filed: 2026-10-24T01:04:54Z_
_Operation: GetResourcePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetResourcePolicy` is documented as "Gets a resource-based policy of an entity that is identified by its resource ARN" and models `ResourceNotFoundException` (HTTP 404). The handler never returns it: for any ARN that merely parses as a catalog resource ARN it answers 200 with `Policy` = `{}` — including ARNs naming entities/change sets the store has no record of, and resources that have never had a policy attached. `PutResourcePolicy`, `TagResource`, `UntagResource` and `ListTagsForResource` all answer 404 for those same ARNs, so the read path contradicts the write path. Expected: 404 `ResourceNotFoundException` for an ARN naming a resource this account/catalog does not hold (and for a resource with no policy attached, rather than a synthesised empty document).

## Evidence

Spec: `GetResourcePolicy` declares `ResourceNotFoundException` (httpError 404) and its output `Policy` targets `ResourcePolicyJson` (@length min 1), i.e. the model has no representation for "no policy".
Handler: `src/ops/get_resource_policy.rs` lines ~171-188 — `store.resource_policies.get(&resource.policy_key()).cloned().unwrap_or_else(|| crate::state::EMPTY_RESOURCE_POLICY.to_string())`, where `EMPTY_RESOURCE_POLICY = "{}"` (`src/state.rs`). No `holds_resource` check and no `ResourceNotFoundException` construction anywhere in the file.
Tests: `tests/all/resource_policy_and_tagging.rs` lines 681-753 assert 200 + "{}" for ARNs naming entities no change set ever created, i.e. the behavior is pinned by test rather than fixed.

## Steps to Reproduce

1. Start the emulator with an empty store
2. GET /GetResourcePolicy?resourceArn=arn:aws:aws-marketplace:us-east-1:000000000000:AWSMarketplace/AmiProduct/prod-b2c3d4e5f6g7h (an entity no change set created)
3. Observe 200 with {"Policy":"{}"}
4. POST /PutResourcePolicy for the same ARN and observe 404 ResourceNotFoundException — the two operations disagree about whether the resource exists

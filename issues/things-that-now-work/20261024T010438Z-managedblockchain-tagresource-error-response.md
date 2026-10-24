Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Tagging operations never raise ResourceNotReadyException for a resource that is still CREATING

_Filed: 2026-10-24T01:04:38Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TagResource, UntagResource and ListTagsForResource are the three operations in this model that declare `ResourceNotReadyException` (HTTP 409, "The requested resource exists but isn't in a status that can complete the operation"), which is how the service refuses a tag operation against a network/member/node that has not finished provisioning.

Expected: TagResource against a resource whose Status is CREATING → 409 ResourceNotReadyException with the tag set unchanged. Actual: the handler resolves the ARN to a stored tag map and writes it regardless of the owning record's status, so a tag operation issued inside the emulator's CREATING window (crate::state::TRANSITION_SPAN, 500 ms after CreateNetwork/CreateMember/CreateNode) answers 200 and mutates the resource. `ResourceNotReadyException` is unreachable from all three tagging handlers, so no client can ever observe or test that modeled failure mode — even though every other handler in this crate (CreateMember, CreateNode, CreateProposal, DeleteNode) does enforce the AVAILABLE precondition it models.

## Evidence

Spec: OPERATIONS.md / Smithy model — TagResource, UntagResource and ListTagsForResource each declare `ResourceNotReadyException (HTTP 409)`; the shape's doc is "The requested resource exists but isn't in a status that can complete the operation." The records carry the status the refusal keys on (`StoredNetwork/StoredMember/StoredNode.status` with CREATING → AVAILABLE settled by `settle()`).
Handlers: src/ops/tag_resource.rs, src/ops/untag_resource.rs, src/ops/list_tags_for_resource.rs — each resolves the ARN via `guard.tags_for_resource[_mut]` and reads/writes the tag map; grep for ResourceNotReadyException in these three files finds only the generated error-enum variant, never a constructed one.
Tests: tests/all/resource_tags.rs asserts the ARN-shape refusal, the not-found path and the 50-tag cap; no test covers tagging a resource that is still CREATING.

## Steps to Reproduce

1. Call create_network(...) and immediately (within the emulator's CREATING window) read get_network to confirm Status == 'CREATING'.
2. Build the new network's ARN (arn:aws:managedblockchain:<region>:<account>:networks/<NetworkId>) and, still inside that window, call tag_resource(ResourceArn=<arn>, Tags={'k': 'v'}).
3. Observe HTTP 200 and that list_tags_for_resource reports the tag; expected 409 ResourceNotReadyException while the network is CREATING, with the tag set unchanged.
4. Repeat with untag_resource and list_tags_for_resource against the CREATING resource: both also answer 200.

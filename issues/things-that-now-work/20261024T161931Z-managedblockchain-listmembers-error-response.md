Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# ListMembers answers an unmodeled 404 ResourceNotFoundException for an unknown NetworkId instead of 400 InvalidRequestException

_Filed: 2026-10-24T16:19:31Z_
_Operation: ListMembers | Class: error-response_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: ListMembers' modeled error set is exactly AccessDeniedException (403), InternalServiceErrorException (500), InvalidRequestException (400) and ThrottlingException (429) — ResourceNotFoundException is NOT one of its errors. For the sibling listing scoped by the same `/networks/{NetworkId}` label, ListNodes, the recorded AWS behavior (see the already-fixed control-plane issue, trace `managedblockchain.list-nodes-nonexistent-member-rejected`) is that an unresolvable network/member is refused with HTTP 400 InvalidRequestException and a request-derived Message (e.g. 'Member with Id m-… not found in network n-….'). ListMembers, whose error list omits ResourceNotFoundException for the same reason, must likewise refuse an unknown NetworkId with 400 InvalidRequestException naming the network.

Actual: the handler returns `ResourceNotFoundException` with HTTP 404 for a NetworkId that names no stored network. To make that compile, the operation's error enum is deliberately widened past the model with `#[smithy_macros::operation_error(..., extra_errors = ["ResourceNotFoundException"])]`. A generated SDK's error union for ListMembers has no ResourceNotFoundException variant, so a typed catch never fires and the failure surfaces as an unmodeled/unknown error; retry and branching logic that treats 404 as 'resource gone' versus 400 as 'bad request' takes the wrong branch. The crate is internally inconsistent about this: `list_nodes.rs` and `list_proposal_votes.rs` both explicitly refuse to emit a 404 their models do not declare (list_proposal_votes.rs: "never a 404 the model does not declare") and answer InvalidRequestException instead, while only ListMembers forces one in.

## Evidence

Spec: smithy_query(operation="ListMembers") — errors = AccessDeniedException(403), InternalServiceErrorException(500), InvalidRequestException(400), ThrottlingException(429); ResourceNotFoundException absent (contrast ListProposals, which does declare it, and which the emulator correctly 404s).
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/list_members.rs lines ~99-117 add `extra_errors = ["ResourceNotFoundException"]` to the generated error enum, and lines ~203-208 of the same file return `ListMembersErrorModeled::ResourceNotFoundException(crate::state::not_found(RESOURCE_NETWORK, &input.network_id))`.
Precedent for the AWS answer: the fixed issue 20261022T023011Z-managedblockchain-control-plane-… Cause 3 records that AWS answers 400 InvalidRequestException (not 404) for ListNodes on an unresolvable network/member precisely because ResourceNotFoundException is not in that operation's modeled error set; src/ops/list_nodes.rs now implements that, and src/ops/list_proposal_votes.rs does the same for proposals.
Tests: tests/all/list_members.rs::a_network_id_that_names_nothing_stored_is_a_404 (lines ~528-560) asserts the current 404/ResourceNotFoundException, so the divergent behavior is pinned by a test that would need updating together with the handler.

## Steps to Reproduce

1. Point a botocore managedblockchain client at the emulator (region us-east-1).
2. Call list_members(NetworkId='n-' + '0'*30) for a network that was never created.
3. Observe HTTP 404 with error code ResourceNotFoundException (message 'network n-0000… not found'), an error code that is not in ListMembers' modeled error union.
4. Expected: HTTP 400 with error code InvalidRequestException and a Message derived from the request naming the unresolvable network, matching how the emulator already answers the sibling call list_nodes(NetworkId='n-'+'0'*30, MemberId='m-'+'0'*30).

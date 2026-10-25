Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: managedblockchain.list-members-nonexistent-network-not-found managedblockchain.list-members-invalid-status-filter-rejected
Issue-Shape-Id: com.amazonaws.managedblockchain#TaigaWebService
Issue-Key: managedblockchain.list-members-nonexistent-network-not-found|run#7|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: managedblockchain.list-members-invalid-status-filter-rejected|run#4|AssertionError|assert control_code == "ResourceNotFoundException", control_code
Issue-Commit: fa5ed22240e89dcfd43a22a078f364ee68f69599

# ListMembers misclassifies an unresolvable NetworkId as InvalidRequestException (400) instead of ResourceNotFoundException (404), and never validates the Status enum filter

> _Note: this issue was identified against the code base at commit `fa5ed22240e89dcfd43a22a078f364ee68f69599`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMembers

## Description
Two root causes in the same handler (services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/list_members.rs). Both must be closed before submitting.

Cause 1 of 2: a well-formed but nonexistent NetworkId is refused as a request-validation error instead of a not-found error.
  AWS: recorded behavior only — the ListMembers operation shape in this service's Smithy model does not declare ResourceNotFoundException (its modeled errors are AccessDeniedException 403, InternalServiceErrorException 500, InvalidRequestException 400, ThrottlingException 429), but the recordings show real AMB answering ListMembers on a well-formed NetworkId that names no network with error code ResourceNotFoundException (the service's own ResourceNotFoundException shape, com.amazonaws.managedblockchain#ResourceNotFoundException, httpError 404), message of the form "Network <NetworkId> not found." — the same not-found sentence GetNetwork (which DOES model ResourceNotFoundException 404) produces for the same id. The recordings also establish the ORDERING: a well-formed request with a valid Status filter reaches the network-existence check and fails there, i.e. existence is reported as not-found and not folded into request validation.
  Emulator: the handler deliberately converts the unresolved-network case into its own InvalidRequestException via the local `network_not_found()` helper (`ListMembersErrorModeled::InvalidRequestException(crate::state::not_found_invalid_request(RESOURCE_NETWORK, network_id))`), on the stated rationale that the operation models no 404. Replay shows code InvalidRequestException (HTTP 400) where ResourceNotFoundException (HTTP 404) is expected.
  Why: a client that branches on the not-found condition — a provisioning poller treating ResourceNotFoundException as "network gone/not yet visible" and retrying, or an idempotent teardown that swallows the 404 — instead sees a 400 request-validation error, which SDK retry/error-classification logic treats as a permanent caller bug. The condition becomes indistinguishable from a genuinely malformed request (see Cause 2), so callers cannot tell "your request was wrong" from "that network does not exist".
  Fix: the ListMembers handler's network-resolution branch — answer the unresolvable scope with the service's ResourceNotFoundException (404) carrying the not-found sentence, rather than remapping it to InvalidRequestException. The operation's error channel (`ListMembersErrorModeled`, generated from the model's declared error list) has to be able to carry that shape, so the error enum / scaffolded error channel for this operation is part of the same fix. The same rationale comment is replicated for the sibling list operations, so check ListNodes / ListProposalVotes for the identical remap while fixing this.
  Traces: managedblockchain.list-members-nonexistent-network-not-found (asserts code ResourceNotFoundException, got InvalidRequestException); managedblockchain.list-members-invalid-status-filter-rejected (its positive control, ListMembers(NetworkId=<nonexistent>, Status=AVAILABLE), fails at the same point with the same substitution).

Cause 2 of 2: the Status filter is not validated against the MemberStatus enum.
  AWS: Smithy: ListMembersInput$Status targets com.amazonaws.managedblockchain#MemberStatus, an enum whose values are CREATING | AVAILABLE | CREATE_FAILED | UPDATING | DELETING | DELETED | INACCESSIBLE_ENCRYPTION_KEY, bound as httpQuery "status"; ListMembers models InvalidRequestException (400). Recorded behavior: a Status value outside that enum is rejected by REQUEST validation with code InvalidRequestException, message of the form "Status <value> is not valid." (example from the recording: "Status NOT_A_REAL_STATUS is not valid."), and that rejection happens BEFORE the network-existence check — the recording distinguishes it from the not-found path of Cause 1 using the very same NetworkId.
  Emulator: the handler performs no enum membership check at all. It treats the filter as an opaque string (`input.status.as_deref().is_none_or(|status| member.status == status)`), so an out-of-enum value simply matches no stored member. Against an existing network that yields a 200 with an empty Members page; against a missing one it falls through to Cause 1's 400. Either way the invalid enum value is never reported.
  Why: a caller that typos or passes a stale/renamed status value gets a successful empty listing instead of a validation error, and concludes the network has no members in that state. Combined with Cause 1, request-shape errors and resource-existence errors are conflated in both directions, so no client can classify ListMembers failures.
  Fix: the ListMembers handler's input validation, ahead of the network lookup — reject a Status not in MemberStatus with InvalidRequestException (400) naming the offending value, before any state resolution. (Message rule: the status 400 and code InvalidRequestException are the requirement; the message text is built from the request member and its enum constraint, so "Status <value> is not valid." is the shape of the requirement, with the recorded string as the example.)
  Traces: managedblockchain.list-members-invalid-status-filter-rejected (the second half of the check, reached once Cause 1 is fixed).

No open issue covers either cause: the filed managedblockchain issues concern DeleteMember/DeleteNode/UpdateNode/UpdateMember lifecycle states, CreateNode's MemberId handling, and managedblockchain-query listings.

## Evidence
Check managedblockchain.list-members-nonexistent-network-not-found: AssertionError: expected ResourceNotFoundException for ListMembers on nonexistent network n-000000000000000000000000000000; got InvalidRequestException.
Check managedblockchain.list-members-invalid-status-filter-rejected: AssertionError: InvalidRequestException — the positive control asserted control_code == "ResourceNotFoundException" for list_members(NetworkId='n-'+'0'*30, Status='AVAILABLE') and the emulator returned InvalidRequestException; the check never reached its out-of-enum assertion (expected code InvalidRequestException with message of the form "Status <value> is not valid.").
Expected (recorded AWS): HTTP 404 / code ResourceNotFoundException, message "Network n-0000... not found." for an unresolvable NetworkId; HTTP 400 / code InvalidRequestException naming the invalid Status value for an out-of-enum filter.
Actual (emulator): HTTP 400 / code InvalidRequestException for the unresolvable NetworkId; no enum validation of Status at all.
Model: smithy_query(operation=ListMembers) — errors AccessDeniedException(403), InternalServiceErrorException(500), InvalidRequestException(400), ThrottlingException(429); NO ResourceNotFoundException declared, while GetNetwork on the same NetworkId does declare ResourceNotFoundException(404). smithy_query(shape=ResourceNotFoundException) — client error, httpError 404, members Message/ResourceName. smithy_query(shape=MemberStatus) — CREATING, AVAILABLE, CREATE_FAILED, UPDATING, DELETING, DELETED, INACCESSIBLE_ENCRYPTION_KEY.
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/list_members.rs lines 127-141 (the `network_not_found()` helper and the comment stating the deliberate remap to InvalidRequestException because the op models no 404), line 216-218 (the unresolved-network branch returning it), lines 225-238 (the Status filter applied as a raw string compare with no enum membership check).

## Steps to Reproduce
1. Point a botocore managedblockchain client at the emulator, region us-east-1 (AMB Access has no us-west-2 endpoint).
2. Call list_members(NetworkId='n-' + '0'*30, Status='AVAILABLE') — a well-formed, valid-enum request against a NetworkId that names no network.
3. Observe HTTP 400 with Error.Code 'InvalidRequestException'; expected HTTP 404 with Error.Code 'ResourceNotFoundException' and a message of the form 'Network n-000000000000000000000000000000 not found.'
4. Call list_members(NetworkId='n-' + '0'*30) with no filters — same substitution (400 InvalidRequestException instead of 404 ResourceNotFoundException).
5. Create a real Hyperledger Fabric network + owner member (create_network with FrameworkConfiguration.Fabric.Edition='STARTER' and a MemberConfiguration), wait for AVAILABLE, then call list_members(NetworkId=<real id>, Status='NOT_A_REAL_STATUS').
6. Observe HTTP 200 with an empty 'Members' list; expected HTTP 400 with Error.Code 'InvalidRequestException' and a message naming the invalid value, e.g. 'Status NOT_A_REAL_STATUS is not valid.' The same call against the nonexistent NetworkId must also report that request-validation error rather than the not-found path.

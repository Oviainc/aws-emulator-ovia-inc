Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: managedblockchain.get-member-not-found managedblockchain.get-accessor-invalid-id-rejected managedblockchain.reject-invitation-not-found
Issue-Shape-Id: com.amazonaws.managedblockchain#TaigaWebService
Issue-Key: managedblockchain.get-member-not-found|run#6|AssertionError|assert message == f"Network {fake_network_id} not found.", message
Issue-Key: managedblockchain.get-accessor-invalid-id-rejected|run#12|AssertionError|assert message == f"AccessorId parameter {fake_accessor_id} is malformed", message
Issue-Key: managedblockchain.reject-invitation-not-found|run#6|AssertionError|assert malformed_message == (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# managedblockchain not-found/invalid-id error semantics diverge: GetMember reports the wrong missing resource, GetAccessor rejects by raw length instead of accessor-id space, RejectInvitation's not-found text omits the member name

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMember,GetAccessor,RejectInvitation

## Description
Three distinct root causes in the error-semantics path of the Amazon Managed Blockchain (AMB) control-plane emulator (crate services/service-com-amazonaws-managedblockchain-taigawebservice). All three operations return the right HTTP status and the right error CODE; what diverges is WHICH resource/validation rule the emulator applies and reports, which is observable to clients. Three causes, all in this one service.

Cause 1 of 3: GetMember on a nonexistent network reports the member as missing instead of the network (lookup/validation order).
AWS: Smithy GetMember — GET /networks/{NetworkId}/members/{MemberId}, input members NetworkId and MemberId are both @required httpLabel ResourceIdString with @length min 1 max 32; modelled errors include ResourceNotFoundException (httpError 404) and InvalidRequestException (400). The model does not state resolution order; recorded behavior only: real AWS resolves the NetworkId path segment FIRST and, when that network does not exist, fails the call reporting the NETWORK as not found (recorded example message: "Network n-000000000000000000000000000000 not found."), never mentioning the member, because the member is scoped inside a network that does not exist. The error code is ResourceNotFoundException and the status 404 in both cases — the requirement here is which resource the 404 is attributed to, built from the request's NetworkId member.
Emulator: returns ResourceNotFoundException (correct code) but attributes it to the member — observed message "Member m-000000000000000000000000000000 not found." — i.e. it goes straight to the member lookup and never checks that the parent network exists.
Why: a client (or a CLI/console user, or any retry/branching logic that keys off which resource is missing) that passes a stale or mistyped NetworkId is told its MEMBER is gone while the network is silently accepted. Diagnostics point at the wrong resource, and code that creates-on-missing-member would try to recreate a member inside a nonexistent network instead of recreating/选择 the network.
Fix: the GetMember handler's resource resolution in this service crate — resolve and validate the NetworkId path label against network state before performing any member lookup, and raise the not-found for the parent network when it is absent.
Traces: managedblockchain.get-member-not-found.

Cause 2 of 3: GetAccessor validates AccessorId only against the raw @length bound, not against the accessor-id space, so the rejection it reports is a generic constraint violation rather than AWS's malformed-id rejection.
AWS: Smithy GetAccessor — GET /accessors/{AccessorId}, AccessorId is @required httpLabel ResourceIdString @length min 1 max 32; modelled errors InvalidRequestException (400) and ResourceNotFoundException (404). Recorded behavior only: real AWS validates the supplied id against the accessor-id space and rejects any id it does not recognize as ever-issued with InvalidRequestException (400) — the recorded check establishes this for BOTH a bare 32-hex-digit id (which satisfies the @length bound) and a 36-character UUID, with a request-derived message naming the member and the supplied value (recorded example: "AccessorId parameter <id> is malformed"). So the requirement is: unissued/ill-formed AccessorId -> 400 InvalidRequestException attributed to the accessorId member, independent of whether the value happens to fit the length bound.
Emulator: returns 400 InvalidRequestException only as a side effect of the generic length check — observed message "AccessorId <uuid> is not valid. Value '<uuid>' at 'accessorId' failed to satisfy constraint: Member must have length less than or equal to 32". The rejection is produced purely because the UUID is 36 characters. Hypothesis: an id inside the 32-character bound that was never issued therefore does not take this path at all and will fall through to a lookup/ResourceNotFoundException instead of the InvalidRequestException AWS returns.
Why: clients that distinguish "you handed me a bad id" (InvalidRequestException, non-retryable, fix the input) from "that resource is gone" (ResourceNotFoundException, re-create/ignore) classify unissued 32-character accessor ids into the wrong branch, and error text that cites a length constraint instead of a malformed id misdirects the fix.
Fix: GetAccessor's input validation in this service crate — validate AccessorId against the accessor-id space/format (not only the @length bound) and raise InvalidRequestException naming the accessorId member for any id the emulator never issued, before the state lookup.
Traces: managedblockchain.get-accessor-invalid-id-rejected.

Cause 3 of 3: RejectInvitation's ResourceNotFoundException message does not name the InvitationId member.
AWS: Smithy RejectInvitation — DELETE /invitations/{InvitationId}, InvitationId is @required httpLabel ResourceIdString @length min 1 max 32; modelled errors include ResourceNotFoundException (404), InvalidRequestException (400), IllegalActionException (400). Recorded behavior only: real AWS does NOT format-validate InvitationId server-side — both a plausible id ("in-" + 29 zeros) and an implausible one ("not-an-invitation-id") come back ResourceNotFoundException, never InvalidRequestException — and the message is built from the request naming the member and the value (recorded example: "InvitationId not-an-invitation-id not found."). The request-derived rule (404 ResourceNotFoundException attributed to the InvitationId member, same shape for any non-existent id) is the requirement; the exact recorded wording is an example.
Emulator: returns the correct 404 / ResourceNotFoundException for both id shapes, but attributes it to a bare resource noun rather than the request member — observed "Invitation not-an-invitation-id not found." (no "Id" / member name).
Why: AMB's not-found text is the only place the service echoes back WHICH input member it failed to resolve; tooling and operators that parse or match on the member-qualified form see an unqualified noun, and the emulator's message is indistinguishable from a failure to resolve some other invitation-shaped entity.
Fix: the RejectInvitation handler's not-found error construction in this service crate — build the ResourceNotFoundException message from the InvitationId input member (member-qualified), the same way the other AMB resource lookups report their id member.
Traces: managedblockchain.reject-invitation-not-found.

## Evidence
Suite: simparity-behavioral-aws-v1, managedblockchain 10/23 passed (43%), 13 failed (10 of them already filed as unimplemented-scaffold blockers for ListAccessors/DeleteAccessor/ListTagsForResource/VoteOnProposal). The three mismatches below are the net-new causes.

Cause 1 — managedblockchain.get-member-not-found [mismatch]
  call: GetMember(NetworkId="n-"+"0"*30, MemberId="m-"+"0"*30)
  expected: ResourceNotFoundException (404) attributed to the NETWORK; recorded example message "Network n-000000000000000000000000000000 not found."
  actual:   ResourceNotFoundException (404) attributed to the MEMBER — "Member m-000000000000000000000000000000 not found."
  (the code assertion passed; the failure is the attributed resource)

Cause 2 — managedblockchain.get-accessor-invalid-id-rejected [mismatch]
  setup/positive control passed: CreateAccessor then GetAccessor on the issued id returns the accessor.
  call: GetAccessor(AccessorId=<random uuid4>)
  expected: InvalidRequestException (400) rejecting the id as not in the accessor-id space; recorded example message "AccessorId parameter <uuid> is malformed"
  actual:   InvalidRequestException (400) from the generic length bound — "AccessorId 58f96631-35ad-4946-b2be-1e06e85c8536 is not valid. Value '58f96631-35ad-4946-b2be-1e06e85c8536' at 'accessorId' failed to satisfy constraint: Member must have length less than or equal to 32"
  Smithy: GetAccessor.AccessorId -> ResourceIdString @length(min 1, max 32), httpLabel, required; errors InvalidRequestException(400), ResourceNotFoundException(404).

Cause 3 — managedblockchain.reject-invitation-not-found [mismatch]
  connectivity control passed: ListInvitations() succeeds.
  call: RejectInvitation(InvitationId="not-an-invitation-id")
  expected: ResourceNotFoundException (404) naming the member; recorded example message "InvitationId not-an-invitation-id not found."
  actual:   ResourceNotFoundException (404) with "Invitation not-an-invitation-id not found."
  Smithy: RejectInvitation.InvitationId -> ResourceIdString @length(min 1, max 32), httpLabel, required; errors ResourceNotFoundException(404), InvalidRequestException(400), IllegalActionException(400).

Related but NOT covering these: open issues 20261022T132335Z-managedblockchain-query-error-responses-are-not-aws-shaped.md and 20261022T132335Z-managedblockchain-query-reads-return-no-data-for-real.md are both for the separate managedblockchain-query service, not this control-plane service.

## Steps to Reproduce
Point a botocore client at the emulator (the checks pin region us-east-1 because AMB Access has no us-west-2 endpoint; the emulator serves all services on one endpoint, so the pin is irrelevant to the result).

Cause 1:
  mbc = boto3.client("managedblockchain", region_name="us-east-1", endpoint_url=<emulator>)
  mbc.get_member(NetworkId="n-"+"0"*30, MemberId="m-"+"0"*30)
  -> observe the ResourceNotFoundException names the member; AWS names the network (the nonexistent parent must be resolved first).

Cause 2:
  created = mbc.create_accessor(ClientRequestToken=<unique<=64>, AccessorType="BILLING_TOKEN", NetworkType="ETHEREUM_MAINNET")
  mbc.get_accessor(AccessorId=created["AccessorId"])            # positive control, succeeds
  mbc.get_accessor(AccessorId=str(uuid.uuid4()))
  -> code is InvalidRequestException but the rejection comes from the @length(max 32) bound, not from accessor-id-space validation; re-run with a never-issued bare 32-hex-digit id to see the length path not fire at all.

Cause 3:
  mbc.list_invitations()                                         # connectivity control, succeeds
  mbc.reject_invitation(InvitationId="not-an-invitation-id")
  mbc.reject_invitation(InvitationId="in-"+"0"*29)
  -> both return ResourceNotFoundException (correct) with a message that omits the InvitationId member name.

Or simply replay the three check ids in trace_ids via the behavioral suite.

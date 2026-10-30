Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rtbfabric.link-reject-terminal-state-blocks-accept-rejected
Issue-Shape-Id: com.amazonaws.rtbfabric#RTBFabric
Issue-Key: rtbfabric.link-reject-terminal-state-blocks-accept-rejected|run#15|AssertionError|assert code == "ConflictException", f"expected ConflictException, got {code}"
Issue-Commit: 05dc617f403125f4713489b7a7f640ccc8d0a41f

# rtbfabric AcceptLink is an unimplemented scaffold stub: accepting a terminally REJECTED link answers 501 NotImplemented instead of 409 ConflictException

> _Note: this issue was identified against the code base at commit `05dc617f403125f4713489b7a7f640ccc8d0a41f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptLink

## Description
One net-new root cause in the rtbfabric emulator, found by the behavioral suite (rtbfabric 2/10 checks passed).

=== Cause 1 of 1: AcceptLink answers 501 NotImplemented, so neither the terminal-state refusal nor the happy-path accept exists ===

AWS: Smithy (services/service-com-amazonaws-rtbfabric-rtbfabric/smithy-model/com-amazonaws-rtbfabric-rtbfabric.json) models AcceptLink as POST /gateway/{gatewayId}/link/{linkId}/accept, @idempotent, success 200, input gatewayId (httpLabel, required, @length 8..32, @pattern ^rtb-gw-[a-z0-9-]{1,25}$), linkId (httpLabel, required, @length 6..30, @pattern ^link-[a-z0-9-]{1,25}$), logSettings (required), optional attributes and timeoutInMillis (@range 100..5000); output requires gatewayId, peerGatewayId, linkId, status (LinkStatus: ACCEPTED | ACTIVE | DELETED | FAILED | ISOLATED | PENDING_CREATION | PENDING_DELETION | PENDING_ISOLATION | PENDING_REQUEST | PENDING_RESTORATION | PENDING_UPDATE | REJECTED | REQUESTED), createdAt, updatedAt. Modeled errors: AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). ConflictException is documented as "The request could not be completed because of a conflict in the current state of the resource." REJECTED is a TERMINAL link state: recorded behavior (the behavioral check, measured against real AWS) is that AcceptLink on a link whose status is REJECTED is refused with HTTP 409, error code `ConflictException` — the recording asserts the code and status only, so the message text is recorded behavior only and is not asserted here; it must be AWS's wording for the state-conflict class for this service (fixed conflict text, with the link/gateway identifiers taken from the request and the state, never emulator-authored prose). The same operation on a link in REQUESTED state must SUCCEED with 200 and drive the link to ACCEPTED and then ACTIVE.

Emulator: the handler is still the generated scaffold stub. services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/accept_link.rs carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle()` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("AcceptLink"))`, so every AcceptLink call — valid or not — is refused with error code `NotImplemented` and HTTP 501. The link's current status is never consulted. The replay reached this point with CreateLink/GetLink/RejectLink working: the link was created, polled to REQUESTED, rejected, and observed REJECTED, and only the AcceptLink call diverged.

Why: a client that drives the rtbfabric link handshake cannot complete it at all. 501 NotImplemented is not a modeled error for AcceptLink, so SDK error handling for ConflictException never runs and a caller retrying a conflict as "service unavailable" will loop; worse, the responder side of the handshake is unreachable, so no link can ever reach ACTIVE against the emulator and the terminal-state guarantee for REJECTED links is unverifiable. Harnesses that assert the state machine (REQUESTED -> ACCEPTED/ACTIVE on accept; REJECTED is terminal) all fail on an unmodeled status code.

Fix: the AcceptLink handler — services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/accept_link.rs, `AcceptLink::handle` (lines 175-193, currently returning `Scaffolded::not_implemented`). Implement the operation against the stored link state: look the link up for the responder gatewayId (404 ResourceNotFoundException when absent), refuse a link in a terminal/ineligible status (REJECTED, DELETED, FAILED) with the modeled 409 ConflictException, and on a REQUESTED link transition it to ACCEPTED/ACTIVE and serialize the full modeled output. Delete the SCAFFOLDED marker line once implemented.

Note on scope: no filed issue names AcceptLink. The open issue 20261029T191824Z-rtbfabric-5-net-new-causes-associatecertificate-getlinkroutingrule-listtagsforresource.md asserts this check is covered by a stub issue for `RejectLink`; that is stale — RejectLink now answers, no such stub issue is open, and the suite reports this check as neither ALREADY FILED nor OWNED. The open stub issues cover ListRequesterGateways and ListTagsForResource only.

Traces: rtbfabric.link-reject-terminal-state-blocks-accept-rejected.

## Evidence
Behavioral suite: rtbfabric 2/10 (20%), 8 failed. This check is the only failure not already filed or owned.

Cause 1 — rtbfabric.link-reject-terminal-state-blocks-accept-rejected [mismatch], check_rtbfabric_link_reject_terminal_state_blocks_accept_rejected.py line 50:
  assert code == "ConflictException", f"expected ConflictException, got {code}"
  AssertionError: expected ConflictException, got NotImplemented

  expected: HTTP 409, error code `ConflictException` (modeled on AcceptLink) for AcceptLink against a link whose status is REJECTED; the link stays REJECTED.
  actual:   HTTP 501, error code `NotImplemented` from the scaffold stub; no state is consulted and none is changed.

All steps before the diverging call succeeded, which localizes the defect to AcceptLink alone:
  - CreateLink(gatewayId=<requester>, peerGatewayId=<responder>, logSettings=...) returned a linkId
  - GetLink polled to status REQUESTED
  - RejectLink(gatewayId=<responder>, linkId=<link>) succeeded
  - GetLink polled to status REJECTED (asserted, passed)
  - AcceptLink(gatewayId=<responder>, linkId=<link>, logSettings=...) -> NotImplemented  <-- diverges

Source: services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/accept_link.rs
  line 4: "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`"
  line 192: Err(crate::scaffolded::Scaffolded::not_implemented("AcceptLink"))

Also unverified behind this stub (asserted later in the same check, never reached): the positive control that a fresh REQUESTED link accepted via AcceptLink settles in ACTIVE (not FAILED), and that the blocked accept leaves the rejected link's status at REJECTED. Both must hold once AcceptLink is implemented.

## Steps to Reproduce
Boot the emulator and point a botocore client for service 'rtbfabric' at its endpoint (the check also uses ec2 for the VPC/subnet/security-group scaffolding and iam for the service-linked role).

1. Create a VPC, a subnet in the first availability zone, and a security group.
2. Create a requester gateway and a responder gateway on that network; poll GetRequesterGateway / GetResponderGateway until both report status ACTIVE.
3. CreateLink(gatewayId=<requester gateway id>, peerGatewayId=<responder gateway id>, logSettings=<link log settings>); poll GetLink(gatewayId=<requester>, linkId=<new link>) until status == "REQUESTED".
4. RejectLink(gatewayId=<responder>, linkId=<link>); poll GetLink(gatewayId=<responder>, linkId=<link>) until status == "REJECTED". (These all pass today.)
5. Call AcceptLink(gatewayId=<responder>, linkId=<link>, logSettings=<link log settings>). EXPECT a ClientError with HTTP 409 and error code ConflictException, because REJECTED is terminal. The emulator instead answers HTTP 501 with error code NotImplemented.
6. Then GetLink(gatewayId=<responder>, linkId=<link>) must still report status == "REJECTED" (no effect from the refused accept).
7. Positive control: CreateLink a second link, poll it to REQUESTED, call AcceptLink on it from the responder gateway — it must return 200 and the link must poll to status ACTIVE (not FAILED).

Replay to confirm: rtbfabric.link-reject-terminal-state-blocks-accept-rejected

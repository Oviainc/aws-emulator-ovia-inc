Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# CreateProposal accepts an InviteAction Principal that is not an AWS account ID

_Filed: 2026-10-24T01:04:38Z_
_Operation: CreateProposal | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InviteAction$Principal` is documented as "The Amazon Web Services account ID to invite". Real AWS validates that the value is an account ID (12 digits) and refuses anything else with InvalidRequestException (400) at CreateProposal time, because no such account could ever create a member from the resulting invitation.

Expected: CreateProposal(Actions={'Invitations': [{'Principal': 'not-an-account'}]}) → 400 InvalidRequestException and no proposal created. Actual: the handler validates only that Invitations and Removals are not both present and not both empty, then stores the actions verbatim; when the proposal is later approved, `carry_out_proposal_actions` mints a StoredInvitation whose `principal` is the garbage string. That invitation is invisible to ListInvitations (which filters `principal == emulator_account_id()`) and unusable by CreateMember, so the APPROVED proposal silently grants nothing — AWS would have refused the create instead.

## Evidence

Spec: `com.amazonaws.managedblockchain#InviteAction$Principal` is @required with doc "The Amazon Web Services account ID to invite"; its target `PrincipalString` carries no length/pattern constraint, so the account-ID rule is service behavior the handler must enforce. CreateProposal models InvalidRequestException (400).
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/create_proposal.rs lines ~189-213 (only the mutually-exclusive/non-empty Actions checks; `actions: input.actions` is then stored as-is) and src/state.rs `record_invitation` / `carry_out_proposal_actions` (~1350-1390), which build the invitation from whatever `invite.principal` holds. src/ops/list_invitations.rs filters on `invitation.principal == engine::emulator_account_id()`, so such an invitation is never reportable.
Tests: tests/all/proposal_lifecycle.rs and tests/all/invitations.rs only use the emulated account id as Principal; no test asserts a refusal for a malformed Principal.

## Steps to Reproduce

1. Create a Hyperledger Fabric network and wait for it (and its first member) to report AVAILABLE.
2. Call client.create_proposal(ClientRequestToken='p1', NetworkId=<nid>, MemberId=<mid>, Actions={'Invitations': [{'Principal': 'not-an-account'}]}). Observe HTTP 200 with a ProposalId; expected 400 InvalidRequestException because Principal is not an AWS account ID.
3. Vote YES with enough members to approve the proposal (vote_on_proposal), then call list_invitations(). Observe no invitation at all, while get_proposal reports Status APPROVED — the approved action granted nothing.

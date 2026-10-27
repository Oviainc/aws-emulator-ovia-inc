Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: f9b86d9948f04d96bbe5506e2764e861468ce8ee

# VoteOnProposal accepts a vote from a member whose removal has started, so a non-participating member decides the proposal

_Filed: 2026-10-27T04:44:51Z_
_Operation: VoteOnProposal | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f9b86d9948f04d96bbe5506e2764e861468ce8ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`VoteOnProposal` resolves `VoterMemberId` through `StoredNetwork::member()`, which applies no status filter, and then records the vote. Every sibling path in this crate treats a member whose removal has begun as no longer part of the membership, and the handler's own comment says so two lines above the lookup — yet the vote it records from such a member is counted in the YES/NO totals that `proposal_outcome` compares against a membership that EXCLUDES it.

Two concrete consequences:

1. Wrong approval arithmetic. `members` is computed from `participating_members(region)` (removal-begun members excluded) while `vote_count(YES)` counts every stored vote (removal-begun members included). On a two-member network with a 50% `GREATER_THAN` policy, begin removing member B: `members` drops to 1 so `required_yes_votes` drops to 1, and a YES vote cast *by B* (still resolvable, still `is_owned`) now single-handedly flips the proposal to `APPROVED` and carries out its `InviteAction`/`RemoveAction`. The same skew can make `yes + no` exceed `members`, which `to_proposal` has to clamp (`(members - cast).max(0)`).

2. Inconsistency with the sibling write on the same resource. `CreateProposal` refuses a member that is not `is_in_service` with `ResourceNotReadyException`; `VoteOnProposal` applies no such check at all, so the very member that may not *create* a proposal may still *decide* one.

The same gap also lets a member still `CREATING` cast a vote.

## Evidence

The handler states the rule it then does not enforce — services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/vote_on_proposal.rs:194-205:

```
// The facts the rules below read, copied out here so the write path can take the
// single mutable borrow it needs. `OutstandingVoteCount` is "the number of members
// minus the sum of YES votes and NO votes", so the membership at THIS instant is
// what the threshold is measured against — a member whose removal has started
// excluded, since it "can no longer participate on the network"
// (`Member$Status`).
let members = network.participating_members(&region).count();
...
let voter = network.member(&region, &input.voter_member_id).cloned();
```

`network.member()` has no status filter — state.rs:793-797:

```
pub fn member(&self, region: &str, member_id: &str) -> Option<&StoredMember> {
    self.members
        .get(member_id)
        .filter(|member| member.in_region(region))
}
```

and the only checks after it are existence, ownership, proposal openness and double-voting (vote_on_proposal.rs:209-260) — never the member's status.

The crate's own rule for a removal-begun record, state.rs:304-312:

```
/// Whether `status` is one a delete has already put the record in: a removal under way
/// ([`STATUS_DELETING`]) or finished ([`STATUS_DELETED`]).
///
/// A record in either "can no longer participate on the network" (`Member$Status` /
/// `Node$Status`), so it is readable and listable but counts as a member or node of the
/// network no longer — which is what the membership counts and name checks read.
pub fn deletion_begun(status: &str) -> bool {
```

The sibling write on the same resource DOES check — src/ops/create_proposal.rs:298-313:

```
// A member in service can create a proposal; one that is still creating or is on
// its way out cannot. A configuration change under way (`UPDATING`) leaves the
// member participating in the network, so it is not a refusal.
if !crate::state::is_in_service(&member.status) {
    return Err(CreateProposalErrorModeled::ResourceNotReadyException(
```

The skewed denominator is read at state.rs:2461-2471 (`proposal_outcome`) and state.rs:1319-1343 (`to_proposal` clamps the negative `OutstandingVoteCount` with `.max(0)`).

Test coverage: tests/all/vote_on_proposal.rs:677-811 (`a_vote_the_service_cannot_accept_is_refused`) tables every refusal `VoteOnProposal` makes — unknown network, unknown proposal, unknown member, double vote, rejected proposal, expired proposal — and no row votes as a `CREATING` or `DELETING` member, so nothing pins the current behaviour.

## Steps to Reproduce

1. `CreateNetwork` with a `VotingPolicy` of `ThresholdPercentage` 50 and `ThresholdComparator` `GREATER_THAN`; wait past `TRANSITION_SPAN` so the network and its founding member are `AVAILABLE`.
2. Add a second owned member to the network (`CreateProposal` + `VoteOnProposal` invite flow, or seed one as tests/all/vote_on_proposal.rs:132 does) and let it settle to `AVAILABLE`. The network now has 2 participating members, so approval needs 2 YES votes.
3. `CreateProposal` as the founding member with an `Invitations` action. `GetProposal` reports `IN_PROGRESS`, `OutstandingVoteCount` 2.
4. `DeleteMember` the SECOND member. It is now `DELETING`, so `participating_members` counts 1 and `required_yes_votes` falls to 1.
5. `VoteOnProposal` with `VoterMemberId` = the `DELETING` member, `Vote` = `YES`. Observe 200: the vote is recorded.
6. `GetProposal`: the proposal is `APPROVED` and its actions were carried out, decided solely by a member that "can no longer participate on the network". Expected: the vote is refused (the member is not in service, exactly as `CreateProposal` refuses it), leaving the proposal `IN_PROGRESS`.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# DeleteMember erases the member (and its network) instead of reporting DELETING then DELETED

_Filed: 2026-10-24T12:47:08Z_
_Operation: DeleteMember | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `DeleteMember` starts an asynchronous deletion and the member stays readable while it runs. `Member$Status` documents the two statuses that describe precisely the state this call produces: "DELETING - The member and all associated resources are in the process of being deleted. Either the Amazon Web Services account that owns the member deleted it, or the member is being deleted as the result of an APPROVED PROPOSAL to remove the member" and "DELETED - The member can no longer participate on the network and all associated resources are deleted". So after a successful DeleteMember, `GetMember` should answer 200 with `Status: DELETING` (then `DELETED`), and `ListMembers` should still report it — which is why `ListMembersInput$Status` accepts DELETING and DELETED. The same holds for the network that loses its last member: `NetworkStatus` declares DELETING and DELETED, so `GetNetwork` should report the network as DELETING/DELETED rather than vanishing.

Actual: the handler calls `guard.remove_member(..)`, which erases the member record and its nodes synchronously, and then `guard.networks.remove(&input.network_id)` when that was the last member. The immediately following `GetMember`/`GetNetwork` is a 404 `ResourceNotFoundException`, `ListMembers` no longer reports the member, and `MemberStatus::DELETING`, `MemberStatus::DELETED`, `NetworkStatus::DELETING` and `NetworkStatus::DELETED` are unreachable from every handler, so the listing filters for them can never match. The same `remove_member` path is also what carries out an approved `RemoveAction` (state.rs `carry_out_proposal_actions`), so a proposal-driven removal erases the member the same way instead of leaving it DELETING — the status doc explicitly names that second path.

## Evidence

Spec: Smithy model `com.amazonaws.managedblockchain#MemberStatus` = CREATING | AVAILABLE | CREATE_FAILED | UPDATING | DELETING | DELETED | INACCESSIBLE_ENCRYPTION_KEY, and `Member$Status`'s DELETING/DELETED prose (quoted above) describes the state after an owner-issued DeleteMember or an approved removal proposal — observable only if the record survives the call. `NetworkStatus` likewise declares DELETING | DELETED. `ListMembersInput$Status` exposes those values as filters.
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/delete_member.rs lines ~249-277: `guard.remove_member(&input.network_id, &input.member_id)`, then `guard.networks.remove(&input.network_id)` when the network is emptied, then `forget_dangling_tokens()`; nothing ever writes MemberStatus DELETING/DELETED or NetworkStatus DELETING/DELETED. `StoredMember` (state.rs ~619-639) carries `status`/`available_at` but no deletion-due instant, and `StoredNetwork::settle` settles only CREATING → AVAILABLE. Shared path: state.rs `carry_out_proposal_actions` (~1543-1547) calls the same `remove_member` for each `RemoveAction`.
Contrast in the same crate: accessors do implement a two-stage deletion (`StoredAccessor::begin_deletion`/`settle`, state.rs ~968-987).
Tests: tests/all/delete_member.rs lines ~176-184 and ~329-337 assert the current behavior (GetMember 404 right after the delete) and ~256-263 asserts GetNetwork 404 after the last member goes; no test asserts a DELETING/DELETED member or network.

## Steps to Reproduce

1. CreateNetwork(... HYPERLEDGER_FABRIC, MemberConfiguration=...) and wait until GetNetwork reports AVAILABLE and GetMember reports the founding member AVAILABLE.
2. Add a second member (so the network is not emptied) and wait for AVAILABLE.
3. Call DeleteMember(NetworkId=<net>, MemberId=<founder>) — observe 200.
4. Immediately call GetMember(NetworkId=<net>, MemberId=<founder>): observe 404 ResourceNotFoundException; expected 200 with Member.Status == 'DELETING' (settling to 'DELETED').
5. Call ListMembers(NetworkId=<net>, Status='DELETING') and Status='DELETED': both empty; expected the deleting member to be listed.
6. Delete the remaining member and call GetNetwork(<net>): observe 404; expected 200 with Network.Status == 'DELETING' (settling to 'DELETED').

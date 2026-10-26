Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# UpdateMember applies the configuration change without ever reporting the member as UPDATING

_Filed: 2026-10-24T16:19:31Z_
_Operation: UpdateMember | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: UpdateMember ("Updates a member configuration with new parameters") starts an asynchronous update that the member reports. `Member$Status` documents "UPDATING - The member is in the process of being updated", and `ListMembersInput$Status` accepts UPDATING as a filter value, so a caller that calls UpdateMember and then GetMember observes Status UPDATING until the update settles back to AVAILABLE, and ListMembers?status=UPDATING reports the member during that window.

Actual: the handler merges the requested `LogPublishingConfiguration` into the stored record and returns, leaving `member.status` exactly as it was (AVAILABLE). No handler in the crate ever writes MemberStatus UPDATING, so the value is unreachable: GetMember never reports an updating member and the ListMembers status=UPDATING filter can never match a real member (only a store seeded by a test). The record already carries everything needed — `StoredMember.status` plus an `available_at` due-instant materialised by `StoredNetwork::settle` — which is exactly how CREATING → AVAILABLE is already modeled, so the update window is simply not represented. Note this is the member-side twin of the already-filed UpdateNode gap (20261024T124708Z-managedblockchain-updatenode-wrong-behavior), which covers only the node.

## Evidence

Spec: smithy_query(doc, shape=Member, member=Status) — "UPDATING - The member is in the process of being updated"; `MemberStatus` enumerates UPDATING and ListMembers binds Status as an httpQuery filter over that enum. UpdateMember's own doc is "Updates a member configuration with new parameters."
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/update_member.rs lines ~183-195 — the only write is `member.log_publishing_configuration = Some(merged)`; `member.status` and `member.available_at` are untouched. grep for UPDATING across src/ finds no handler that sets it (src/state.rs defines only STATUS_CREATING/STATUS_AVAILABLE plus the accessor statuses).
Tests: tests/all/update_member_log_publishing.rs asserts only that GetMember reads the merged configuration back; the only UPDATING occurrences under tests/ are directly seeded member statuses in list_members.rs / delete_member.rs, so no test covers the member's status after an update.

## Steps to Reproduce

1. Create a Hyperledger Fabric network and wait until GetMember reports the first member Status AVAILABLE.
2. Call update_member(NetworkId=<net>, MemberId=<member>, LogPublishingConfiguration={'Fabric': {'CaLogs': {'Cloudwatch': {'Enabled': True}}}}) — observe 200.
3. Immediately call get_member: observe Member.Status == 'AVAILABLE'; expected 'UPDATING' until the update settles, then 'AVAILABLE'.
4. Immediately call list_members(NetworkId=<net>, Status='UPDATING'): observe an empty Members page; expected the member being updated to be reported.

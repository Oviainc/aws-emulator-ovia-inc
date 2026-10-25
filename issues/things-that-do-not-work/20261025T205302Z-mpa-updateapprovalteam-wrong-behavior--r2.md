Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 558d123cf10a98531c236bb7b81525d860a40117

# UpdateApprovalTeam mints (and keeps) approver ids before it validates MinApprovalsRequired, so a refused request mutates the store

_Filed: 2026-10-25T20:53:02Z_
_Operation: UpdateApprovalTeam | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `558d123cf10a98531c236bb7b81525d860a40117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateApprovalTeam` rebuilds the requested approver set — minting a fresh `ApproverId` out of the SHARED store for every approver the request adds — BEFORE it validates the resulting M-of-N relation, before the session quota, and before the approval-instant computation. All three of those checks return an error, but the guard held over the store is already dirty: `state.mint_approver_id()` increments the store's `next_approver_seq`, and dropping the guard on the `Err` path persists that increment. So a request this operation refuses with 400 `ValidationException` (or 402 `ServiceQuotaExceededException`, or the 500 for an uncomputable instant) permanently consumes approver ids: the next team or update that IS accepted mints `approver-N+k` instead of `approver-N+1`, and the `ApproverId` values a later `GetApprovalTeam` / `GetSession` reports skip values for no reason a caller can see.

The sibling create handler does the SAME two steps in the opposite (correct) order — it validates `repeated_identity` and `MinApprovalsRequired` first and only then mints ids — so the two write paths that build an approver set disagree about whether a refusal may touch the store. The crate states the rule it is breaking in three other places (`CreateApprovalTeam` "before any state is written", `TagResource` "decided before anything is written, so a refused call leaves the resource's tags exactly as they were", `StartActiveApprovalTeamDeletion` "computed BEFORE anything is mutated"), and `UpdateApprovalTeam` itself states it for the instant check nine lines below the code that already wrote.

This is distinct from the fixed "applies the change in place instead of creating a pending update" issue: the pending-update handshake is implemented correctly; the defect is that the mint happens on the wrong side of the validation boundary.

No test pins the minted id values — tests/all/approval_team_lifecycle.rs:532-539 asserts only that an id was minted and is unique, and tests/all/pending_team_update.rs:481 asserts only that a RETAINED approver keeps its id — so moving the mint after the checks leaves every response and every existing test unchanged.

## Evidence

Handler writes first, validates second — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/update_approval_team.rs:294-307:
```
                    match retained {
                        Some(stored) => rebuilt.push(stored.clone()),
                        None => {
                            let approver_id = state.mint_approver_id();
                            rebuilt.push(crate::state::Approver::invited(
```
…and only afterwards, src/ops/update_approval_team.rs:317-332:
```
        // M of N is checked against the set the update PRODUCES, not the one it replaces.
        ...
        if resulting_min > resulting_count {
            return Err(UpdateApprovalTeamErrorModeled::ValidationException(ValidationException {
```
The mint is a store write, not a local one — src/state.rs:2419-2423:
```
    /// A fresh, never-reused approver id.
    pub fn mint_approver_id(&mut self) -> String {
        self.next_approver_seq += 1;
        format!("approver-{}", self.next_approver_seq)
    }
```
Sibling handler, same crate, opposite order — src/ops/create_approval_team.rs:360-375:
```
        if approval_strategy.min_approvals_required() > approver_count {
            return Err(CreateApprovalTeamErrorModeled::ValidationException(ValidationException {
 ...
            .map(|requested| {
                let approver_id = state.mint_approver_id();
```
The rule the crate states for every other refusal — src/ops/create_approval_team.rs:11-12 ("earns `AccessDeniedException` (403) before any state is written"); src/ops/tag_resource.rs:216-219 ("Whether the merged set fits is decided before anything is written, so a refused call leaves the resource's tags exactly as they were."); src/ops/start_active_approval_team_deletion.rs:193-195 ("The whole window is computed BEFORE anything is mutated, with checked arithmetic throughout, so a request can neither panic this handler nor leave a team half-moved into DELETING."); and in this very handler, src/ops/update_approval_team.rs:356-358 ("Nothing is written until every instant this handshake records can be computed") — stated 58 lines after the mint that already wrote.

Same-order quota/instant checks also sit downstream of the mint: src/ops/update_approval_team.rs:358-368 (500 for an uncomputable approval instant) and :374-386 (`ServiceQuotaExceededException` for `MAX_SESSIONS`).

Test coverage that must stay green: tests/all/approval_team_lifecycle.rs:532-539 (`ApproverId` minted and unique), tests/all/pending_team_update.rs:139/481 (retained approvers keep their `ApproverId`) — neither asserts a concrete id, so the ordering fix is response-preserving.

## Steps to Reproduce

1. Record the caller's account as the organization management account, then POST /approval-teams to create team T with one approver and ApprovalStrategy MofN{MinApprovalsRequired: 1}; let the invitation response come due so T is ACTIVE.
2. GET /approval-teams/{T} and note the single approver's ApproverId (e.g. `approver-1`).
3. PATCH /approval-teams/{T} with body {"Approvers":[{"PrimaryIdentityId":"identity-9999","PrimaryIdentitySourceArn":"<any arn>"}],"ApprovalStrategy":{"MofN":{"MinApprovalsRequired":2}}} — MinApprovalsRequired (2) exceeds the resulting approver count (1).
4. Observe HTTP 400 ValidationException and that no pending update was recorded (GET /approval-teams/{T} reports no PendingUpdate).
5. Now PATCH /approval-teams/{T} with a VALID change that adds one approver (Approvers: the existing approver plus one new identity, MinApprovalsRequired 1).
6. GET /approval-teams/{T} and read PendingUpdate.Approvers: the added approver carries `approver-3`, not `approver-2` — the id minted during the refused call in step 3 was consumed and kept. Repeat step 3 N times to skip N ids.

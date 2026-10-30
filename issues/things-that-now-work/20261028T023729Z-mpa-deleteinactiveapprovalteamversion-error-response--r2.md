Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# The 409 for a version awaiting the team's approval reads "is PENDING and not INACTIVE or PENDING"

_Filed: 2026-10-28T02:37:29Z_
_Operation: DeleteInactiveApprovalTeamVersion | Class: error-response_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteInactiveApprovalTeamVersion` builds every one of its `ConflictException` refusals from one closure whose sentence states the rule "the version must be INACTIVE or PENDING". That sentence is correct for the two call sites that refuse a LIVE version (an ACTIVE team, a team inside its deletion window), but the handler also reuses the closure for the REQUESTED version a team's approvers are still deciding — and that version's status is always `PENDING` (`PendingTeamUpdate::requested` records `status: TeamStatus::Pending`, and `is_outstanding()` is exactly `status == TeamStatus::Pending`).

So the one refusal a caller gets for the outstanding requested version reads: "Approval team '<arn>' version '<id>' is PENDING and not INACTIVE or PENDING, so it cannot be deleted by DeleteInactiveApprovalTeamVersion; a version awaiting the team's approval is abandoned by cancelling its update session with CancelSession." The message both asserts and denies the same status, and it tells the caller a PENDING version is deletable while the branch directly above it refuses precisely that. The handler's own branches disagree: for the team's live version `TeamStatus::Pending | TeamStatus::Inactive => {}` is the accepted (deletable) arm, while for a requested version `PENDING` is the refused state and only `INACTIVE` is deletable. The status (409) and the state change (none) are right; only the sentence the caller reads is self-contradictory, so the remedy it offers is buried in a claim that cannot be true.

## Evidence

The one refusal sentence, which always names INACTIVE and PENDING as the deletable statuses — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/delete_inactive_approval_team_version.rs:206-224:
```rust
        // The refusal this operation answers a version it may not delete with: the
        // version's own status, and what the caller can do about it instead.
        let conflict = |version_status: &str, remedy: &str| {
            ...
                        message: crate::state::bounded_message(format!(
                            "Approval team '{}' version '{}' is {version_status} and not {} or \
                             {}, so it cannot be deleted by {}; {remedy}.",
                            input.arn,
                            input.version_id,
                            crate::state::TeamStatus::Inactive.as_wire(),
                            crate::state::TeamStatus::Pending.as_wire(),
                            Self::NAME,
                        )),
```
The call site that feeds it the requested version's status — same file, lines 230-241:
```rust
        if let Some(requested) = team
            .pending_update
            .as_ref()
            .filter(|update| update.version_id == input.version_id)
        {
            if requested.is_outstanding() {
                return Err(conflict(
                    requested.status.as_wire(),
                    "a version awaiting the team's approval is abandoned by cancelling its \
                     update session with CancelSession",
                ));
            }
```
That status is always `PENDING` — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:1051 and :1064-1066:
```rust
            status: TeamStatus::Pending,
            status_code: Some(TeamStatusCode::UpdatePendingApproval),
...
    pub fn is_outstanding(&self) -> bool {
        self.status == TeamStatus::Pending
    }
```
And the same handler treats PENDING as DELETABLE for the live version — delete_inactive_approval_team_version.rs:265-277:
```rust
        match status {
            // A version that has never been activated (the team draft) or one that is no
            // longer active is this operation's to delete.
            crate::state::TeamStatus::Pending | crate::state::TeamStatus::Inactive => {}
```
Test coverage: none pins this message. tests/all/inactive_team_version_deletion.rs spans the LIVE version's status only (`Stored::{PendingDraft, Inactive, Active, Deleting}`, rows at :163-229) and its only message assertion is `message.contains(&arn) && message.contains(&asked_for)` (:336-340) — the status words are never checked, and no row names the version a team's approvers are still deciding.

## Steps to Reproduce

1. Record the emulator account as the organization's management account (as tests/all/approval_team_lifecycle.rs:192 does) and create an approval team; let its approvers' invitations be materialised so the team is ACTIVE.
2. Call `PATCH /approval-teams/{Arn}` with `{"Description": "a change"}`. It answers 200 with the REQUESTED `VersionId` (say `2`) and records a pending update awaiting the team's approval.
3. Immediately — before `UPDATE_APPROVAL_DELAY` elapses — call `DELETE /approval-teams/{Arn}/2`.
4. Observe HTTP 409 `ConflictException` whose `Message` reads "... version '2' is PENDING and not INACTIVE or PENDING, so it cannot be deleted by DeleteInactiveApprovalTeamVersion; ..." — the sentence asserts and denies the same status.
5. Contrast with `DELETE /approval-teams/{Arn}/1` on a PENDING draft team (tests/all/inactive_team_version_deletion.rs row "a draft version, named by its own id"), which succeeds with 200 — PENDING is deletable there, exactly as the message claims, and not deletable above.

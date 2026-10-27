Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# An active team's deletion completes with no team approval, and the pending window is measured from the request instead of the approval

_Filed: 2026-10-27T04:33:25Z_
_Operation: StartActiveApprovalTeamDeletion | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states that deleting an ACTIVE approval team requires the team's approval, and it defines `PendingWindowDays` as the span between the team's APPROVAL of the delete request and the removal. The handler implements neither half:

1. It records no approval request at all. It flips the stored team to `DELETING` / `DELETE_PENDING_APPROVAL` and returns, without starting any session for the approvers — `crate::state::start_session` (the one path every session of this service is recorded through) is never called, and `crate::state::SessionKind` has only `Baseline` and `Update` variants. Nothing in the crate ever records an approval of a deletion, so `DELETE_PENDING_APPROVAL` names a pending approval that no record exists for and that nobody can ever give or refuse.
2. The completion instant is computed from `now` — the instant of the delete REQUEST — so `materialize_due` removes the team `PendingWindowDays` days after the request, whether or not the team ever approved it. Per the member's own documentation the window should start at the approval.

The net effect is that an unapproved deletion of an active team always succeeds: the team is destroyed (together with every session that names it, via `remove_team`) on a timer the team was never consulted about. The crate itself proves it knows the rule: `state::DEFAULT_PENDING_WINDOW_DAYS` quotes the member's sentence verbatim in its own doc comment, and the sibling `UpdateApprovalTeam` implements the IDENTICAL documented rule for updates ("Updates require team approval") by starting an update session through `state.start_session(SessionKind::Update, ..)` and letting the clock materialise the approval. `StartActiveApprovalTeamDeletion` skips the helper its sibling uses for the same rule.

## Evidence

Model (kind A) — `smithy_query(query="doc",shape="StartActiveApprovalTeamDeletion")`:
  "Starts the deletion process for an active approval team." + note: "**Deletions require team approval** / Requests to delete an active team must be approved by the team."
Model — `smithy_query(query="doc",shape="StartActiveApprovalTeamDeletionRequest",member="PendingWindowDays")`:
  "Number of days between when the team approves the delete request and when the team is deleted."
The same sentence is already pasted into the handler file:
  services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/start_active_approval_team_deletion.rs:65-66 — "//! Starts the deletion process for an active approval team. Deletions require team approval / //! Requests to delete an active team must be approved by the team."

The code that contradicts it:
  start_active_approval_team_deletion.rs:196-199 —
    `let days = input.pending_window_days.unwrap_or(crate::state::DEFAULT_PENDING_WINDOW_DAYS);`
    `let Some(completion) = crate::state::deletion_completion(now, days) else {`   <- window measured from `now`, the request instant
  start_active_approval_team_deletion.rs:227-232 —
    `crate::state::TeamStatus::Active => {`
    `    team.status = crate::state::TeamStatus::Deleting;`
    `    team.status_code = Some(crate::state::TeamStatusCode::DeletePendingApproval);`
    `    team.deletion_start_time = Some(now);`
    `    team.deletion_completion_time = Some(completion);`
    `}`   <- the whole handler body; no session is started, nothing records an approval
  state.rs:2086-2098 (materialize_due) —
    `// A team whose pending deletion window has elapsed is gone.`
    `.filter(|team| team.deletion_completion_time.is_some_and(|due| due <= now))` ... `self.remove_team(&arn);`   <- removal gated on the clock alone

The crate's own statement of the rule it does not implement:
  state.rs:278-284 —
    "/// Days between the delete request and the team's removal when `StartActiveApprovalTeamDeletion` omits `PendingWindowDays`.
     /// `PendingWindowDays` is documented as \"Number of days between when the team approves the delete request and when the team is deleted\""

Sibling that DOES implement the identical documented rule (kind B):
  update_approval_team.rs:7-13 — "//! \"Updates require team approval. Updates to an active team must be approved by the team\": a change to the configuration an ACTIVE team's approvers approved is therefore not applied where it is asked for. It is REQUESTED ..."
  update_approval_team.rs:387-393 — `let Some(session_arn) = state.start_session(crate::state::SessionKind::Update, &team_record, session_approvers, crate::state::request_region(ctx.parts()), now,)`
  state.rs:2199-2261 — the clock materialises that approval (`session.approve(due)` / `team.record_update_approval(..)`) or its failure (`team.record_update_failure()`).
  state.rs:1330-1333 — `SessionKind` renders only `Self::Baseline => "StartApprovalTeamBaseline"` and `Self::Update => "UpdateApprovalTeam"`: no deletion session kind exists, so no deletion approval is ever recorded.

Test coverage that pins today's (divergent) behaviour, with no stated reason for the divergence:
  tests/all/approval_team_lifecycle.rs:1142-1147 — asserts `completion - start == expected_days`, i.e. the window measured from `DeletionStartTime` rather than from an approval.
  tests/all/approval_team_lifecycle.rs:1156-1160 — asserts `StatusCode == DELETE_PENDING_APPROVAL`.
  tests/all/approval_team_lifecycle.rs:1180-1186 — `harness.expire_deletion_window(&arn);` then asserts `GetApprovalTeam` is 404: the team is gone with no approval ever recorded.

## Steps to Reproduce

1. Create an approval team and let its approvers' invitations come due so GetApprovalTeam reports Status=ACTIVE.
2. Call POST /approval-teams/{Arn}?Delete with no body (PendingWindowDays defaults to 7).
3. GetApprovalTeam reports Status=DELETING, StatusCode=DELETE_PENDING_APPROVAL — but ListSessions for the same team reports no new session, and GetApprovalTeam reports no session ARN for the deletion, so there is nothing for the team to approve and no way to approve or refuse it.
4. Observe the returned DeletionCompletionTime equals DeletionStartTime + 7 days, i.e. 7 days from the REQUEST, although the model defines PendingWindowDays as the span from the team's approval.
5. Advance the store past DeletionCompletionTime (tests/all/approval_team_lifecycle.rs does this with harness.expire_deletion_window): GetApprovalTeam answers 404 and ListApprovalTeams omits the team — an active team was deleted without the approval the model requires.
6. Contrast with UpdateApprovalTeam on the same ACTIVE team: it starts an update session (GetApprovalTeam reports UpdateSessionArn) and only applies the change once the approval is recorded.

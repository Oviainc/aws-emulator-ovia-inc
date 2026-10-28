Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# materialize_due expires a deletion session before the team approval that fell due first, so an approved deletion is discarded and the team is never removed

_Filed: 2026-10-28T05:29:41Z_
_Operation: StartActiveApprovalTeamDeletion | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due` applies the clock-owed changes in CODE order, not in the order the clock made them due. The session loop runs FIRST and moves every outstanding session whose `expiration_time` has passed to `FAILED` / `EXPIRED` (`SESSION_EXPIRATION` = 24 h, state.rs:309). Only afterwards does the requested-DELETION loop look at that session to decide whether the approvers' answer — owed at `approval_due`, which is `DELETION_APPROVAL_DELAY` = 500 ms after the StartActiveApprovalTeamDeletion call (state.rs:270) — has come due. By then the session is already `FAILED`, so the deletion loop falls into its `_ =>` arm and calls `team.record_deletion_failure()`, which TAKES `pending_deletion` and sets the team back to `ACTIVE` / `DELETE_FAILED_APPROVAL`.

Consequence: whether an accepted deletion ever happens depends only on WHEN the next request arrives. A caller that makes any call ~1 s after the delete request has the approval materialised (`record_deletion_approval`), the pending window starts, and the team is removed when `removal_due()` arrives. A caller whose next request is more than `SESSION_EXPIRATION` after the delete request instead sees the deletion silently abandoned: the recorded `PendingTeamDeletion` is dropped, the team lives on, and the `DeletionCompletionTime` the 200 response already promised (start_active_approval_team_deletion.rs:334-339, answered out of `PendingTeamDeletion::completion_time`) is an instant at which nothing will ever happen. The earlier-due approval is lost, so stored state (the requested deletion) is destroyed by a later-due event.

This contradicts the rule the crate states for itself in three places: `materialize_due` is documented to apply "only what the CLOCK already owed at this instant" — "changes a caller who never polled would have seen anyway" (get_approval_team.rs:9-15); the deletion block's own header says the approval "is recorded first and the removal below is owed only afterwards" (state.rs:2613-2616); and `PendingTeamDeletion::completion_time` is documented as fixed "because the approval the clock owes is stamped with the instant it was DUE" (state.rs:1099-1103).

NOTE on overlap: the open issue 20261027T153945Z-mpa-getapprovalteam-wrong-behavior reports the same ordering defect for the UPDATE loop (state.rs:2540-2611) only. This is the second, independent block — the deletion loop at state.rs:2617-2675 — with a different observable loss (a team that approved its own deletion is never deleted), and a fix to the update loop alone leaves it broken.

## Evidence

The expiry is applied first — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:2513-2533:
```
for session in self.sessions.values_mut() {
    ...
    // A session no approver answered before its expiration is over: it FAILED,
    // with the `EXPIRED` status code, and completed at the instant it expired.
    if session.status.is_outstanding() && session.expiration_time <= now {
        session.status = SessionStatus::Failed;
        session.status_code = Some(SessionStatusCode::Expired);
        session.completion_time = Some(session.expiration_time);
```
…and only THEN is the earlier-due deletion approval considered — state.rs:2613-2616 and 2632-2674:
```
// The deletion a team's approvers were asked to approve. Their approval is what
// STARTS the pending window, so it is recorded first and the removal below is owed
// only afterwards. A session that ended without an approval — cancelled, or
// expired unanswered — is a deletion the team refused, so the team stays.
...
match self.sessions.get(&session_arn).map(|session| (session.status, session.completion_time)) {
    // Still outstanding: the approvers' answer is owed at `due` ...
    Some((status, _)) if status.is_outstanding() => { if due <= now { ... team.record_deletion_approval(due, &answered); } }
    ...
    // The session ended without an approval, or no longer exists: this is a
    // deletion the team did not approve, and an unapproved deletion deletes
    // nothing.
    _ => {
        if let Some(team) = self.approval_teams.get_mut(&team_arn) {
            team.record_deletion_failure();
```
The discard — state.rs:1341-1347:
```
pub fn record_deletion_failure(&mut self) {
    if self.pending_deletion.take().is_none() { return; }
    self.status = TeamStatus::Active;
    self.status_code = Some(TeamStatusCode::DeleteFailedApproval);
}
```
The two due instants — state.rs:270 and state.rs:309:
```
pub const DELETION_APPROVAL_DELAY: Duration = Duration::from_millis(500);
pub const SESSION_EXPIRATION: Duration = Duration::from_secs(24 * 60 * 60);
```
The rules the code breaks — src/ops/get_approval_team.rs:9-15:
```
//! `@readonly`: this handler writes nothing of its own. It calls
//! [...]::materialize_due first, which applies only what the
//! CLOCK already owed at this instant (an invitation response that has come due, the
//! approval of a deletion the team was asked for and the end of the pending window that
//! approval starts, ...) — changes a caller who never polled would
//! have seen anyway.
```
and state.rs:1099-1103:
```
/// Instant the deletion is scheduled to complete, as `DeletionCompletionTime` reports
/// it: the approval instant plus the pending window. It is fixed when the request is
/// recorded and never moves, because the approval the clock owes is stamped with the
/// instant it was DUE.
pub completion_time: SystemTime,
```
The response that promises that instant — src/ops/start_active_approval_team_deletion.rs:331-339:
```
let recorded = state.team(&input.arn).and_then(|team| team.pending_deletion.as_ref());
Ok(StartActiveApprovalTeamDeletionOutput {
    deletion_start_time: recorded.map(|deletion| ...deletion.start_time),
    deletion_completion_time: recorded.map(|deletion| ...deletion.completion_time),
})
```
Test coverage: the only test of an expired deletion session deliberately POSTPONES the approval first, so the approval-due-before-expiry path is untested — tests/all/active_team_deletion_approval.rs:421-445:
```
// The approvers must not answer while this row arranges the ending under test.
harness.postpone_deletion_approval(&arn);
...
Ending::Expired => { expire_session(&harness, &session_arn, SystemTime::now() - Duration::from_secs(1)); }
```
and tests/all/approval_team_lifecycle.rs:301-305 confirms what postponing does:
```
pub(crate) fn postpone_deletion_approval(&self, arn: &str) {
    self.with_pending_deletion(arn, |deletion| {
        deletion.approval_due = Some(SystemTime::now() + Duration::from_secs(3600));
```

## Steps to Reproduce

1. Create an approval team and let its approvers' invitations be materialised so GetApprovalTeam reports Status=ACTIVE (as `active_team` in tests/all/approval_sessions.rs does).
2. Call POST /approval-teams/{Arn}?Delete with {"PendingWindowDays": 1}. It answers 200 with DeletionStartTime and DeletionCompletionTime, records a PendingTeamDeletion with approval_due = now + 500 ms, and starts a deletion session (src/ops/start_active_approval_team_deletion.rs:287-312).
3. Make NO other call, and move that deletion session's expiration_time into the past — the clock-equivalent of letting SESSION_EXPIRATION (24 h) elapse before the next request — exactly as tests/all/approval_sessions.rs:166 `expire_session` does, WITHOUT calling postpone_deletion_approval.
4. Call GET /approval-teams/{Arn}: materialize_due expires the session at state.rs:2528 and then runs the deletion loop, which takes the `_ =>` arm and calls record_deletion_failure. The team answers 200 with Status=ACTIVE and StatusCode=DELETE_FAILED_APPROVAL, and the deletion the clock had already approved 23:59:59 earlier is gone.
5. Repeat steps 1-2 and instead make any call after DELETION_APPROVAL_DELAY: the same deletion is approved, the pending window starts, and expiring the window removes the team — so the outcome of an already-accepted deletion, and the truth of the DeletionCompletionTime already returned to the caller, depend only on when the next request arrived.

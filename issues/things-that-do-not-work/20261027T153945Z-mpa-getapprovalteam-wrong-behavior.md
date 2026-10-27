Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0d4ec79976a0b91eeb5a23059b2beecdecae287a

# materialize_due applies an update session's expiry before the team approval that fell due first, so a read taken late discards the approved change

_Filed: 2026-10-27T15:39:45Z_
_Operation: GetApprovalTeam | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0d4ec79976a0b91eeb5a23059b2beecdecae287a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due` applies the clock-owed changes in CODE order, not in the order the clock made them due. The session loop runs first and expires every outstanding session whose `expiration_time` has passed (`SESSION_EXPIRATION` = 24h); only afterwards does the requested-update loop look at the update session to see whether the approvers' answer — owed at `approval_due`, which is `UPDATE_APPROVAL_DELAY` = 500ms after the UpdateApprovalTeam call — has come due. Because the session is already `FAILED`/`EXPIRED` by then, the update loop falls into its `_ =>` arm and calls `record_update_failure()`.

Consequence: for a change requested on an ACTIVE team, what `GetApprovalTeam` reports depends on WHEN the caller polls. A caller that reads ~1s after the update sees the approval materialise and the requested version become live (the behaviour `pending_team_update.rs` pins). A caller whose next request is more than `SESSION_EXPIRATION` after the update instead sees `StatusCode: UPDATE_FAILED_APPROVAL`, the live `VersionId` unchanged and `PendingUpdate.Status: INACTIVE` — the accepted change is discarded although its approval was owed 23:59:59 before the expiry that killed it. The clock-owed approval that was due earlier is silently dropped, so stored state (the requested version) is lost.

That directly contradicts the rule the handler and the state module state for themselves: materialize_due is supposed to apply "only what the CLOCK already owed at this instant" — "changes a caller who never polled would have seen anyway" — and the approval is supposed to be "stamped with that instant rather than with `now`, so a materialised value does not move between reads".

## Evidence

The expiry is applied first — src/state.rs:2177-2197:
```
for session in self.sessions.values_mut() {
    ...
    // A session no approver answered before its expiration is over: it FAILED,
    // with the `EXPIRED` status code, and completed at the instant it expired.
    if session.status.is_outstanding() && session.expiration_time <= now {
        session.status = SessionStatus::Failed;
        session.status_code = Some(SessionStatusCode::Expired);
```
…and only then is the earlier-due approval considered — src/state.rs:2200-2203 (comment) and 2219-2260:
```
// The change a team's approvers were asked to approve: their approval comes due
// first, then the activation of the configuration it carries. A session that ended
// without an approval — cancelled, or expired unanswered — is a change the team
// never approved, so the team keeps the configuration it has.
...
if let Some(due) = approval_due {
    match self.sessions.get(&session_arn).map(...) {
        // Still outstanding: the approvers' answer is owed at `due`, and the
        // answer is stamped with that instant rather than with `now`, so a
        // materialised value does not move between reads.
        Some((status, _)) if status.is_outstanding() => { if due <= now { ... } }
        ...
        // The session ended without an approval, or no longer exists: the
        // requested version is one this team will never move to.
        _ => { if let Some(team) = self.approval_teams.get_mut(&team_arn) {
                   team.record_update_failure();
```
The two due instants, src/state.rs:211 and src/state.rs:259:
```
pub const UPDATE_APPROVAL_DELAY: Duration = Duration::from_millis(500);
pub const SESSION_EXPIRATION: Duration = Duration::from_secs(24 * 60 * 60);
```
The rule the code breaks — src/ops/get_approval_team.rs:9-14:
```
//! `@readonly`: this handler writes nothing of its own. It calls
//! [`crate::state::ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`] first, which applies only what the
//! CLOCK already owed at this instant (an invitation response that has come due, a
//! pending deletion window that has run out, the approval and activation of a change the
//! team was asked for) — changes a caller who never polled would
//! have seen anyway.
```
and src/state.rs:52-55:
```
//! * The approval of a change an active team was asked for ([`UPDATE_APPROVAL_DELAY`])
//!   and, after it, that change's activation ([`UPDATE_ACTIVATION_DELAY`]): the team
//!   reports `UPDATE_PENDING_APPROVAL`, then `UPDATE_PENDING_ACTIVATION`, and the
//!   requested configuration becomes the live one when the activation comes due.
```
Test coverage: only the short-gap path is pinned — tests/all/pending_team_update.rs:600-651 (`the_clock_alone_carries_a_requested_change_through_to_the_live_version`, `wait_for_the_clock(state::UPDATE_APPROVAL_DELAY)` at :621). The crate already owns a helper that reproduces the long gap without waiting, tests/all/approval_sessions.rs:163-172 (`expire_session`, "Only the DUE INSTANT is moved"), but it is used for BASELINE sessions only (tests/all/approval_sessions.rs:575, :651; tests/all/session_listing.rs:543; tests/all/session_cancellation.rs:345) — no test exercises an UPDATE session whose expiration passes before any read materialises the approval.

## Steps to Reproduce

1. Create an approval team and let its approvers' invitations be materialised so the team is ACTIVE (as `active_team` in tests/all/approval_team_lifecycle.rs does).
2. Call `PATCH /approval-teams/{Arn}` with `{"Description": "carried by the clock"}`; it answers 200 with the requested `VersionId` and records a pending update plus an update session (src/ops/update_approval_team.rs:387-437).
3. Without making any other call, move that update session's `expiration_time` into the past (the clock-equivalent of letting `SESSION_EXPIRATION` = 24h elapse before the next request), exactly as tests/all/approval_sessions.rs:166 `expire_session` does for a baseline session.
4. Call `GET /approval-teams/{Arn}`: `materialize_due` expires the session first (src/state.rs:2192) and then reports the change as never approved — `StatusCode` is `UPDATE_FAILED_APPROVAL`, `VersionId` is still the pre-update version and `PendingUpdate.Status` is `INACTIVE`.
5. Repeat steps 1-2 and instead read after `UPDATE_APPROVAL_DELAY` + `UPDATE_ACTIVATION_DELAY`: the same update is approved and activated, so the outcome of an already-accepted change depends only on when the caller polled.

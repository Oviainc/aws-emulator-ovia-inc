Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# A second baseline session may be started for an approver who already has one outstanding, and the team read then silently reports only the first

_Filed: 2026-10-28T19:29:36Z_
_Operation: StartApprovalTeamBaseline | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartApprovalTeamBaseline` is the only session-starting path in this crate with no guard against a second OUTSTANDING session for the same subject. Its refusals are: team not `ACTIVE`, an `ApproverIds` entry that is not an approver of the team, the same approver id named twice in ONE request, and the store's session ceiling. Nothing looks at the sessions the team already has, so calling it twice for the same approver records two outstanding baseline sessions that both ask that approver.

`GetApprovalTeamResponseApprover$PendingBaselineSessionArn` is single-valued, and its one resolver answers with `.find(...)` — the FIRST outstanding baseline session that includes the approver. So the second session exists in the store and `GetSession`/`ListSessions` report it as an outstanding session asking approver A, while `GetApprovalTeam`'s entry for A names only the first session's ARN: the second is invisible from the team read. That is exactly the disagreement the two comments over that code say cannot happen.

The crate's two other session-starting paths both refuse a second outstanding session for the same subject: `UpdateApprovalTeam` answers `ConflictException` ("One change at a time") and `StartActiveApprovalTeamDeletion` treats a team already `DELETING` as an idempotent no-op ("nothing is started twice"). Only the baseline path skips that check.

## Evidence

The resolver that can only report one, state.rs:3243-3260:
```
/// The one resolver for `GetApprovalTeamResponseApprover$PendingBaselineSessionArn`,
/// so the team read and `GetSession` report the same session for the same approver.
...
pub fn pending_baseline_session_arn(&self, team_arn: &str, approver_id: &str) -> Option<String> {
    self.team_sessions_in_initiation_order(team_arn)
        .into_iter()
        .find(|session| {
            session.kind == SessionKind::Baseline
                && session.status.is_outstanding()
                && session.includes_approver(approver_id)
        })
```
The claim it is relied on for, ops/get_approval_team.rs:215-217:
```
// The outstanding baseline session of each approver, resolved out of the SAME
// session records `GetSession` reports, so the two reads cannot disagree about
// which session is pending for whom.
```
The guard the siblings apply and this handler skips:
- ops/update_approval_team.rs:253-264 — `// One change at a time: a team already waiting on its approvers has a version it is moving to, and a second request would ask them to approve two.` followed by `return Err(... ConflictException ...)`.
- ops/start_active_approval_team_deletion.rs:244-246 — `// Already requested: \`@idempotent\`, so the caller is answered with the window the first request recorded and nothing is started twice.` / `crate::state::TeamStatus::Deleting => {}`.
- ops/start_approval_team_baseline.rs:197-275 — the complete set of refusals (team not ACTIVE; `"ApproverIds names '{approver_id}', which is not an approver of approval team"`; `"ApproverIds names approver '{approver_id}' more than once"`; `state.sessions.len() >= crate::state::MAX_SESSIONS`). No stored session is consulted before `state.start_session(crate::state::SessionKind::Baseline, ...)` at line 282.

Test coverage stops at ONE baseline session: tests/all/approval_sessions.rs:611-640 (`the_team_read_reports_the_approver_s_outstanding_baseline_session`) starts a single baseline and asserts each selected approver reports that one ARN; no test starts a second overlapping baseline.

## Steps to Reproduce

1. Record the management account in the store as tests/all/approval_team_lifecycle.rs:192-195 does, create an approval team with 2 approvers and wait until `GetApprovalTeam` reports it ACTIVE.
2. `POST /approval-teams/{Arn}/baseline` with `ApproverIds: [A]` -> 200, `BaselineSessionArn` S1.
3. `POST /approval-teams/{Arn}/baseline` with `ApproverIds: [A]` again -> 200, `BaselineSessionArn` S2 (accepted; a second outstanding baseline session now asks A).
4. `GET /sessions/S2` -> an outstanding session whose `ApproverResponses` names A, and `POST /approval-teams/{Arn}/sessions/?List` lists both S1 and S2.
5. `GET /approval-teams/{Arn}` -> approver A's `PendingBaselineSessionArn` is S1 only; S2 is reported by no approver, so the team read and the session reads disagree about which session is pending for A.

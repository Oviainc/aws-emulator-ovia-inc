Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# A session whose protected operation the clock executed still reports ExecutionStatus PENDING: EXECUTED is a token no code path can produce

_Filed: 2026-10-28T19:29:36Z_
_Operation: GetSession | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every session this crate mints is stamped `ExecutionStatus = PENDING` once, at construction, and nothing ever moves it: `SessionExecutionStatus::` appears exactly ONE time in the whole of `src/` (the constructor). The comment that justifies it says the emulator executes no protected operation — but it does. `materialize_due` carries out exactly the action the session approves: for a `SessionKind::Update` session it calls `team.activate_pending_update(due)`, which installs the requested description/strategy/approvers and makes the requested version the team's live one; for a `SessionKind::Deletion` session it calls `remove_team`. `SessionKind::action_name` itself names those operations (`UpdateApprovalTeam`, `StartActiveApprovalTeamDeletion`) as "the action this kind of session approves".

So after an update session is approved and activated, `GetApprovalTeam` proves the protected operation ran (new `VersionId`, new `Description`, team `ACTIVE`, nothing pending), while `GetSession` on that same session answers `Status: APPROVED`, `ActionCompletionStrategy: AUTO_COMPLETION_UPON_APPROVAL` and `ExecutionStatus: PENDING` — i.e. it reports the executed action as still waiting to run. `SessionExecutionStatus` declares `EXECUTED`, and no path in this crate can ever produce it.

The two existing assertions that pin `PENDING` do so for sessions where nothing was executed (an outstanding BASELINE session, and a CANCELLED session), so neither covers the approved case.

## Evidence

Model (`smithy_query(query="shape",shape="ListSessionsResponseSession")`, member `ActionCompletionStrategy`): "Strategy for executing the protected operation. `AUTO_COMPLETION_UPON_APPROVAL` means the operation is executed automatically using the requester's permissions, if approved." — and every session carries that strategy:
state.rs:1938-1939 — `// The only strategy \`ActionCompletionStrategy\` declares.` / `action_completion_strategy: ActionCompletionStrategy::AutoCompletionUponApproval,`

The claim the PENDING value rests on, state.rs:1934-1937:
```
// This emulator executes no protected operation, so nothing has run and
// nothing has failed: the execution is `PENDING` for as long as the session
// exists.
execution_status: SessionExecutionStatus::Pending,
```
The same crate executes it, state.rs:2789-2797:
```
// The approved configuration becomes the live one once its activation is due.
let activation_due = ...
if let Some(due) = activation_due.filter(|due| *due <= now) {
    if let Some(team) = self.approval_teams.get_mut(&team_arn) {
        team.activate_pending_update(due);
```
and state.rs:1827-1832 — `/// The action this kind of session approves, as \`ActionName\` reports it: the MPA operation that starts it.` ... `Self::Update => "UpdateApprovalTeam",` / `Self::Deletion => "StartActiveApprovalTeamDeletion",`
and ops/update_approval_team.rs:13-17 — "The clock owes the rest: the approvers' approval ..., and the activation of the approved configuration ..., at which point the requested version becomes the team's live one".

The read: ops/get_session.rs:262 — `execution_status: Some(session.execution_status.as_wire().to_string()),` (and ops/list_sessions.rs reports the same record).

Grep proof that nothing moves it: `SessionExecutionStatus::` matches only `src/state.rs:1937` in the whole crate.

Test coverage does NOT pin the approved case:
- tests/all/approval_sessions.rs:383-393 asserts `ActionName == "StartApprovalTeamBaseline"` then `ExecutionStatus == "PENDING"` with the reason "nothing was executed" — an OUTSTANDING baseline session (line 414-417 asserts it has no `CompletionTime`).
- tests/all/session_cancellation.rs:217-222 asserts `ExecutionStatus == "PENDING"` for a session that was just CANCELLED, again "nothing was executed".

## Steps to Reproduce

1. Record the management account in the store, as the crate's own tests must (tests/all/approval_team_lifecycle.rs:192-195 `record_management_account`), since no wire path records one.
2. `POST /approval-teams` with a valid body; wait past `state::INVITATION_RESPONSE_DELAY` so `GetApprovalTeam` reports the team ACTIVE.
3. `PATCH /approval-teams/{Arn}` with a new `Description`; note the `UpdateSessionArn` that `GET /approval-teams/{Arn}` now reports.
4. Wait past `state::UPDATE_APPROVAL_DELAY + state::UPDATE_ACTIVATION_DELAY` (~1s) and `GET /approval-teams/{Arn}`: the team is ACTIVE at the requested VersionId with the new Description — the protected operation (`UpdateApprovalTeam`) has been executed.
5. `GET /sessions/{UpdateSessionArn}`: `Status` is `APPROVED`, `ActionName` is `UpdateApprovalTeam`, `ActionCompletionStrategy` is `AUTO_COMPLETION_UPON_APPROVAL`, but `ExecutionStatus` is still `PENDING`.

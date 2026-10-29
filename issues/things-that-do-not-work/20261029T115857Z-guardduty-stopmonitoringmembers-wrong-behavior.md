Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fb07764462a6e3726541f141027927b0567b370d

# StopMonitoringMembers overwrites the `Invited` relationship of a member whose invitation is still outstanding, so a stop-then-start promotes it to `Enabled` without any acceptance

_Filed: 2026-10-29T11:58:56Z_
_Operation: StopMonitoringMembers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fb07764462a6e3726541f141027927b0567b370d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StopMonitoringMembers` and `StartMonitoringMembers` are declared in this crate to be inverses of one another over one stored field, `MemberRecord::relationship_status`. `StartMonitoringMembers` states — in its own file header, in its handler comment, and in a test that pins the rule with a stated reason — that a member whose invitation is still OUTSTANDING (`Created`, `Invited`) must not be moved by a monitoring call, because "monitoring of such an account begins when it accepts the invitation, and restarting monitoring does not accept one on its behalf". Its code implements exactly that: it moves a member only when the stored status is `Disabled`.

Its twin breaks the same rule. `StopMonitoringMembers` moves ANY associated member whose status is not already `Disabled` — and `MemberRecord::is_associated()` is true for `Created`, `Invited`, `Enabled` and `Disabled` alike (only `Removed` is excluded) — so a member sitting at `Invited` with an unanswered invitation is rewritten to `Disabled`. The relationship state that recorded the outstanding invitation is gone (only `invited_at_millis` survives, so `GetMembers` reports an account with an `invitedAt` instant and `relationshipStatus: Disabled`).

The composition is the damage: once the stop has rewritten the member to `Disabled`, that member now satisfies `StartMonitoringMembers`' one precondition, so the very next `StartMonitoringMembers` legitimately moves it to `Enabled`. The crate's own definition of `Enabled` (state.rs:1843-1851) says that status is reached either "once the member account has accepted the invitation" or "once `StartMonitoringMembers` has turned monitoring back on for an account `StopMonitoringMembers` had stopped" — and this path reaches it with no acceptance and with no monitoring ever having been on. The protection `StartMonitoringMembers` was written to provide is defeated by its twin, by a sequence of two calls that each answer HTTP 200.

The already-pinned `Created` → `Disabled` behaviour of the stop (tests/all/member_monitoring.rs:139-152) is not what this reports; the `Invited` status — the one that represents an invitation awaiting the member account's answer, and the one the twin names explicitly — is.

## Evidence

THE RULE, stated by the twin's own file header — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/start_monitoring_members.rs:12-17:
```
//! Monitoring is a DESIRED STATE, not a transition: an associated member whose
//! monitoring is not stopped is processed again with nothing changed (its
//! `UpdatedAt` does not move), so a retried call is indistinguishable from the
//! first. That also covers a member whose invitation is still outstanding
//! (`Created`, `Invited`): monitoring of such an account begins when it accepts the
//! invitation, and restarting monitoring does not accept one on its behalf.
```
and IMPLEMENTED by it — src/ops/start_monitoring_members.rs:212-224:
```
            match detector.members.get_mut(account_id) {
                Some(member) if member.is_associated() => {
                    // Only monitoring restarts: ... Monitoring is a desired state, so only a member
                    // whose monitoring was STOPPED moves — a member already monitored,
                    // and one whose invitation is still outstanding, keep their
                    // relationship and their `UpdatedAt`.
                    if member.relationship_status == crate::state::RELATIONSHIP_DISABLED {
                        member.relationship_status =
                            crate::state::RELATIONSHIP_ENABLED.to_string();
                        member.updated_at_millis = now;
                    }
                }
```

THE VIOLATION — src/ops/stop_monitoring_members.rs:232-243 moves every status that is not already `Disabled`, `Invited` included, and its header (stop_monitoring_members.rs:4-9, "moving each one's `RelationshipStatus` to `Disabled`") names no exception:
```
            match detector.members.get_mut(account_id) {
                Some(member) if member.is_associated() => {
                    // The record is RETAINED and the association is untouched: only
                    // monitoring stops, so a later `GetMembers` still reports the
                    // account and its email. Stopping is a desired state, so a
                    // member already stopped moves neither its status nor its
                    // `UpdatedAt`.
                    if member.relationship_status != crate::state::RELATIONSHIP_DISABLED {
                        member.relationship_status =
                            crate::state::RELATIONSHIP_DISABLED.to_string();
                        member.updated_at_millis = now;
                    }
                }
```

WHY `Invited` REACHES THAT ARM — src/state.rs:2590-2599:
```
    /// Is this member currently ASSOCIATED with the administrator?
    pub fn is_associated(&self) -> bool {
        self.relationship_status != RELATIONSHIP_REMOVED
    }
```
so `Created`, `Invited`, `Enabled` and `Disabled` all pass the guard; `Invited` is "the relationship status of an invitation that has been delivered and not yet answered" (src/state.rs:1837-1841).

WHAT `Enabled` IS SUPPOSED TO MEAN — src/state.rs:1843-1851:
```
/// The relationship status of a member account that is being MONITORED:
/// `Master.RelationshipStatus` once the member account has accepted the
/// invitation, and `Member.RelationshipStatus` once `StartMonitoringMembers` has
/// turned monitoring back on for an account `StopMonitoringMembers` had stopped
```

IT IS OBSERVABLE — src/state.rs:2635-2639 (`member_view`, shared by `GetMembers` and `ListMembers`) reports `relationship_status: member.relationship_status.clone()` while `invited_at: member.invited_at_millis.map(iso8601)` stays set.

TEST COVERAGE: the twin's rule is pinned WITH its reason — tests/all/member_monitoring_restart.rs:372-381:
```
async fn restarting_monitoring_of_a_member_that_was_not_stopped_changes_nothing() {
    // Rows differing along the relationship the member is in when monitoring is
    // turned on: recorded and not yet invited, invited and not yet answered. Neither
    // is a member whose monitoring was stopped, so neither moves — an outstanding
    // invitation is answered by the member account, never by this call.
    let rows: [(&str, &str); 2] = [
        ("Created", "recorded by CreateMembers and not yet invited"),
        ("Invited", "invited and not yet answered"),
    ];
```
Nothing pins the stop side for an invited member: tests/all/member_monitoring.rs builds its fixture with `detector_with_members` (:85-97), which only calls `CreateMembers`, so every row of `stopping_monitoring_moves_only_the_named_members_to_disabled` (:109-179) and of `stopping_monitoring_again_changes_nothing` (:232) starts from `Created` — the suite never sends `InviteMembers`, and the `Invited` status appears nowhere in that file.

## Steps to Reproduce

1. Create an enabled detector: `POST /detector` with `{"enable": true}`; note `detectorId`.
2. Record a member: `POST /detector/{detectorId}/member` with `{"accountDetails": [{"accountId": "100000000001", "email": "member@example.com"}]}` — 200, `unprocessedAccounts` empty.
3. Send it an invitation: `POST /detector/{detectorId}/member/invite` with `{"accountIds": ["100000000001"]}` — 200.
4. Read it back: `POST /detector/{detectorId}/member/get` with `{"accountIds": ["100000000001"]}` — the member reports `relationshipStatus: "Invited"` and an `invitedAt` instant (src/state.rs:2635-2639). The invitation is delivered and unanswered.
5. Stop monitoring that member: `POST /detector/{detectorId}/member/stop` with `{"accountIds": ["100000000001"]}` — 200, `unprocessedAccounts` empty.
6. Read it back again: `relationshipStatus` is now `"Disabled"` (stop_monitoring_members.rs:239-242) while `invitedAt` is still set — the outstanding invitation's relationship status has been overwritten, which src/ops/start_monitoring_members.rs:12-17 says a monitoring call must not do.
7. Restart monitoring: `POST /detector/{detectorId}/member/start` with `{"accountIds": ["100000000001"]}` — 200.
8. Read it back a third time. Expected, per start_monitoring_members.rs:15-17 ("monitoring of such an account begins when it accepts the invitation, and restarting monitoring does not accept one on its behalf") and per src/state.rs:1843-1851: the account is still awaiting its own acceptance, so it is not reported as monitored. Actual: `relationshipStatus: "Enabled"` — the status the crate reserves for an account that accepted the invitation or whose monitoring had genuinely been stopped — reached by two 200 calls and no acceptance.
9. Contrast with the protected path: repeat steps 1-4 and then call `POST /detector/{detectorId}/member/start` directly, without the stop. The member stays `Invited` and its `updatedAt` does not move (start_monitoring_members.rs:220-223, pinned by tests/all/member_monitoring_restart.rs:372-418).

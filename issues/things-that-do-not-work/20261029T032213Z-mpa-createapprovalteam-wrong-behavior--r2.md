Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c0c2239955f7987f5942ca86a100c431d5defb9f

# A ClientToken replay answers with the team's CURRENT VersionId, not the one the first call returned, which the handler's own rule says it must

_Filed: 2026-10-29T03:22:13Z_
_Operation: CreateApprovalTeam | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c0c2239955f7987f5942ca86a100c431d5defb9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateApprovalTeam`'s `@idempotencyToken` replay is documented to answer with the team the first call minted — "same `Arn`, same `VersionId`, no second team". The implementation does not answer from what the first call returned: it resolves the token to the stored ARN and re-renders the response out of the LIVE record (`created(team)` reads `team.version_id`). A team's live `version_id` is not immutable — `UpdateApprovalTeam` moves it (`bump_version`, which assigns `self.version_id`) on the apply-in-place path (a team whose configuration has never been activated, which is exactly the state a freshly created team is in for `INVITATION_RESPONSE_DELAY`), and `activate_pending_update` moves it on the ACTIVE handshake path.

So after any update the replay of the create returns a `VersionId` the first create never returned (e.g. "2" where the first answer was "1"), while still claiming, per the handler doc, to be "the team the first call minted". A client that retries a timed-out create — the whole purpose of the token — therefore reads a version id it must not attribute to its create, and two answers to the same request (same body, same token) disagree. Nothing else in the create response drifts (`Arn`, `Name`, `CreationTime` are fixed at creation), so `VersionId` is the one member that breaks the stated rule.

The value the first call returns is always `FIRST_VERSION` (`const FIRST_VERSION: u64 = 1;`, stored as `version_id: FIRST_VERSION.to_string()`), so the replay has a well-defined answer available without any new state: the version the create minted, rather than whatever version the team has moved on to.

## Evidence

The rule the handler states, services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/create_approval_team.rs:20-21:
```
//! * `ClientToken` (`@idempotencyToken`) replay returns the team the first call minted —
//!   same `Arn`, same `VersionId`, no second team.
```

The replay path, which re-renders from the LIVE record, src/ops/create_approval_team.rs:255-261:
```
        if let Some(token) = client_token.as_deref() {
            if let Some(arn) = state.idempotency_tokens.get(token).cloned() {
                if let Some(team) = state.team(&arn) {
                    return Ok(created(team));
                }
            }
        }
```

What `created` reports, src/ops/create_approval_team.rs:440-451:
```
/// Version a newly minted team carries.
const FIRST_VERSION: u64 = 1;

/// The `CreateApprovalTeam` response for a stored team.
fn created(team: &crate::state::ApprovalTeam) -> CreateApprovalTeamOutput {
    CreateApprovalTeamOutput {
        arn: Some(team.arn.clone()),
        name: Some(team.name.clone()),
        version_id: Some(team.version_id.clone()),
        creation_time: Some(::aws_smithy_types::DateTime::from(team.creation_time)),
    }
}
```

The sibling that moves the member the replay re-reads, src/ops/update_approval_team.rs:460-480 (the apply-in-place path a still-`PENDING` team takes):
```
        let team = state.team_mut(&input.arn).ok_or_else(not_found)?;
        let mut applied = false;
        if let Some(description) = input.description.as_ref() {
            team.description = description.clone();
            applied = true;
        }
        ...
        if applied {
            team.bump_version(now);
        }
```
and src/state.rs:1326-1337:
```
    /// Record a new version of the team IN PLACE: bump the version id and the update
    /// time.
    ...
    pub fn bump_version(&mut self, now: SystemTime) {
        self.version = self.mint_version();
        self.version_id = self.version.to_string();
        self.last_update_time = Some(now);
    }
```
(the ACTIVE handshake reaches the same member through `activate_pending_update`, src/state.rs:1443-1445: `self.version = update.version; ... self.version_id = update.version_id;`).

The model's own idempotency note is about not acting twice, not about re-reading live state (smithy_query doc=CreateApprovalTeamRequest$ClientToken): "With an idempotent request, if the original request completes successfully, any subsequent retries complete successfully without performing any further actions." — the drift here is in the ANSWER, against the handler's own "same `VersionId`" rule.

Test coverage: the replay is pinned only without an intervening update, so it passes today and would still pass with the fix — tests/all/approval_team_lifecycle.rs:681-689:
```
        let (status, first) = harness.create(&body).await;
        assert_eq!(status, 200, "case {label}: first create returned {first:?}");
        let (status, replay) = harness.create(&body).await;
        assert_eq!(status, 200, "case {label}: replay returned {replay:?}");
        assert_eq!(
            (text(&replay, "Arn"), text(&replay, "VersionId")),
            (text(&first, "Arn"), text(&first, "VersionId")),
            "case {label}: a replayed ClientToken must answer with the first team"
        );
```
No test updates a team and then replays its create token.

## Steps to Reproduce

1. Record the organization management account so the gated create is served (as tests/all/approval_team_lifecycle.rs does), then `POST /approval-teams` with `ClientToken: "t1"`, one approver and a valid `Policies` reference. Note the answer: `VersionId` is "1".
2. Immediately (while the team is still `PENDING`, before `crate::state::INVITATION_RESPONSE_DELAY` elapses) `PATCH /approval-teams/{Arn}` with `{"Description":"changed"}`. The apply-in-place branch runs `bump_version`, so the stored team now carries `VersionId` "2".
3. Replay the create byte-for-byte: `POST /approval-teams` with the same body and `ClientToken: "t1"`.
4. Observe HTTP 200 with `VersionId: "2"` — not the "1" the first call returned, although the handler doc at create_approval_team.rs:20-21 states the replay answers with "the team the first call minted — same `Arn`, same `VersionId`". `Arn`, `Name` and `CreationTime` are unchanged, so `VersionId` alone drifts.
5. The same drift occurs on the ACTIVE path: wait past the invitation delay, `PATCH` a `Description`, wait past `UPDATE_APPROVAL_DELAY` + `UPDATE_ACTIVATION_DELAY`, then replay the create with `ClientToken: "t1"` — the replay reports the activated version id.

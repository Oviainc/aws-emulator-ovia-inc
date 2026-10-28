Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# Three of the store's clock-instant constructors use the panicking `SystemTime + Duration`, the arithmetic this crate's own rule and its four sibling constructors replace with `checked_add`

_Filed: 2026-10-28T23:43:41Z_
_Operation: CreateApprovalTeam | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` records every clock-owed transition as a DUE INSTANT on the stored record, and the crate states one rule for computing those instants: they are computed with CHECKED arithmetic, before anything is written, so a handler can neither panic nor leave a record half-written. Four construction sites implement that rule — `PendingTeamUpdate::requested` (`now.checked_add(UPDATE_APPROVAL_DELAY)`), `ApprovalTeam::record_update_approval` (`at.checked_add(UPDATE_ACTIVATION_DELAY)`), `Session::for_team` (`now.checked_add(SESSION_EXPIRATION)?` / `now.checked_add(SESSION_ACTIVATION_DELAY)`) and `deletion_schedule` (`start.checked_add(..)?` twice) — and two handlers turn a `None` from them into a modelled refusal rather than letting the arithmetic run (`update_approval_team.rs:332-342`, `start_active_approval_team_deletion.rs:216-235`).

Three sibling constructors in the SAME module skip the rule and use the panicking `std::ops::Add` impl for `SystemTime` instead:

* `Approver::invited` — `invitation_response_due: Some(now + INVITATION_RESPONSE_DELAY)` (state.rs:1102). This is reached from the one approver-minting path `ComAmazonawsMpaAwsfluffycoreserviceState::invite_requested_approvers`, which `CreateApprovalTeam` (`create_approval_team.rs:360-369`) and `UpdateApprovalTeam` (`update_approval_team.rs:488-497`) both call.
* `IdentitySource::creating` — `activation_due: Some(now + IDENTITY_SOURCE_ACTIVATION_DELAY)` (state.rs:1607), reached from `create_identity_source.rs:326-342`.
* `IdentitySource::begin_deletion` — `self.deletion_due = Some(now + IDENTITY_SOURCE_DELETION_DELAY)` (state.rs:1640), reached from `delete_identity_source.rs:273`.

`SystemTime: Add<Duration>` panics on overflow (the four `checked_add` sites and the two handler refusals above exist precisely because it can fail), so these three sites abort the request instead of answering the modelled server error the siblings answer — and `Approver::invited` is called from inside `invite_requested_approvers`, i.e. partway through building the approver set, which is exactly the "half-written record" state `start_active_approval_team_deletion.rs:216-219` says the checked arithmetic is there to prevent. The inconsistency is purely internal: the three skipped sites take the same `now: SystemTime` argument as the four checked ones, so there is no reason for them to be spelled differently.

## Evidence

The crate's stated rule, services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/start_active_approval_team_deletion.rs:216-219:
```
        // The whole schedule — when the team's answer comes due, and when the window that
        // answer starts ends — is computed BEFORE anything is mutated, with checked
        // arithmetic throughout, so a request can neither panic this handler nor leave a
        // team half-moved into DELETING.
```

The same rule, src/ops/update_approval_team.rs:329-342:
```
            // Nothing is written until every instant this handshake records can be
            // computed, so a requested change can never be left waiting on an approval
            // that is owed at no instant at all.
            if now
                .checked_add(crate::state::UPDATE_APPROVAL_DELAY)
                .is_none()
            {
                return Err(UpdateApprovalTeamErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the update's approval instant could not be computed".to_string(),
                    },
                )
                .into());
            }
```

The four sibling constructors that implement it (src/state.rs):
- `864:     start.checked_add(pending_window(days)?)`
- `887:     let approval_due = start.checked_add(DELETION_APPROVAL_DELAY)?;`
- `1177:            approval_due: now.checked_add(UPDATE_APPROVAL_DELAY),`
- `1366:        update.activation_due = at.checked_add(UPDATE_ACTIVATION_DELAY);`
- `1953:            expiration_time: now.checked_add(SESSION_EXPIRATION)?,`
- `1955:            activation_due: now.checked_add(SESSION_ACTIVATION_DELAY),`

The three that skip it (src/state.rs), each the panicking `+`:
- `1102:            invitation_response_due: Some(now + INVITATION_RESPONSE_DELAY),`
- `1607:            activation_due: Some(now + IDENTITY_SOURCE_ACTIVATION_DELAY),`
- `1640:        self.deletion_due = Some(now + IDENTITY_SOURCE_DELETION_DELAY);`

`Session::for_team`'s doc states what the checked form buys, src/state.rs:1903-1906:
```
    /// Each caller keeps its own refusals, which the model fixes per operation: the
    /// store's session ceiling ([`MAX_SESSIONS`]) is reported with the error the calling
    /// operation declares for it, and `None` here says only that `now` names no instant
    /// this service could record the session's expiration at.
```
and `start_session` relies on it to leave the store untouched, src/state.rs:3273-3277:
```
    /// The initiation-order sequence number the session's ARN carries is consumed by a
    /// session that is really stored and by nothing else: when the session cannot be built
    /// ([`Session::for_team`] answers `None` because `now` names no instant its expiration
    /// could be recorded at) nothing is written ...
```
The same guarantee is unavailable at state.rs:1102/1607/1640 because the arithmetic there has no failure value to return.

The call sites that reach the unchecked three:
- src/ops/create_approval_team.rs:360-369 — `state.invite_requested_approvers(...)` -> `Approver::invited` (state.rs:3200-3205).
- src/ops/update_approval_team.rs:488-497 — the same helper, via `requested_approver_set`.
- src/ops/create_identity_source.rs:326-342 — `crate::state::IdentitySource::creating(...)`.
- src/ops/delete_identity_source.rs:273 — `source.begin_deletion(now);`.

Test coverage: the clock-owed instants these constructors set are exercised end to end by tests/all/approval_team_lifecycle.rs, tests/all/identity_source_deletion.rs, tests/all/identity_source_detail.rs, tests/all/clock_due_order.rs and tests/all/clock_owed_order.rs; the checked-arithmetic refusals of the siblings are the ones update_approval_team.rs:332 and start_active_approval_team_deletion.rs:223 raise. Replacing `now + DELAY` with `now.checked_add(DELAY)` and routing a `None` to the same modelled server error the siblings use changes no response for any instant those suites use, so every existing test stays green.

## Steps to Reproduce

1. Open services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs and grep for the clock arithmetic: `grep -n 'checked_add\|now + ' src/state.rs`.
2. Observe six sites use `checked_add` (lines 864, 887, 1177, 1366, 1953, 1955) and three use the panicking `+` (lines 1102, 1607, 1640).
3. Read src/ops/start_active_approval_team_deletion.rs:216-219 and src/ops/update_approval_team.rs:329-342: the crate states that every instant it records is computed with checked arithmetic BEFORE anything is mutated, precisely so a request can neither panic a handler nor leave a record half-written, and both handlers answer the modelled `InternalServerException` / `ValidationException` when the computation cannot be made.
4. Follow the three unchecked sites to their handlers: `Approver::invited` (state.rs:1102) is reached from `invite_requested_approvers` (state.rs:3182-3209), called by src/ops/create_approval_team.rs:360 and src/ops/update_approval_team.rs:488; `IdentitySource::creating` (state.rs:1607) from src/ops/create_identity_source.rs:326; `begin_deletion` (state.rs:1640) from src/ops/delete_identity_source.rs:273.
5. Note that `Approver::invited` is called from inside the loop that builds the approver set, so a failure there cannot be turned into a refusal at all — the exact "half-written record" outcome start_active_approval_team_deletion.rs:216-219 says the checked form exists to rule out, and the opposite of the `start_session` guarantee stated at state.rs:3273-3277.

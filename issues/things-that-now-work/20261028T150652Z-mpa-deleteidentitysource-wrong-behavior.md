Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8668e0a390f031d37f864cf13043544a41d29fed

# The clock removes an identity source without re-checking the in-use rule, so a source state.rs says "is not removed at all" is removed while a stored team still authenticates through it

_Filed: 2026-10-28T15:06:52Z_
_Operation: DeleteIdentitySource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8668e0a390f031d37f864cf13043544a41d29fed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteIdentitySource` does not remove the source: it records `DELETING` and lets the clock remove it `IDENTITY_SOURCE_DELETION_DELAY` later (`IdentitySource::begin_deletion`, then `materialize_due`). The IN-USE rule, however, is evaluated ONLY in the handler, at request time: the removal pass in `materialize_due` deletes every source whose `deletion_due` has passed without ever consulting `team_arn_using_identity_source`, the one resolver the crate declares for that decision.

So a source that becomes in-use AFTER the delete request was accepted — any `CreateApprovalTeam`/`UpdateApprovalTeam` inside the delay window naming it in `Approvers[].PrimaryIdentitySourceArn`; neither write path validates the source — is still removed when the clock comes due. The store is then in exactly the state the handler's `ConflictException` exists to prevent ("removing it would leave that team pointing at a source this store no longer holds"): `GetApprovalTeam` keeps reporting an approver authenticated by an ARN `ListIdentitySources`/`GetIdentitySource` no longer hold, and a second `DeleteIdentitySource` for it can never raise the 409 because the record is gone.

This is not the already-fixed request-time gap (20261025T201417Z-mpa-deleteidentitysource-validation, which taught the handler's check to read the pending update's approvers): the handler's check is correct, the REMOVAL path simply does not make it. Fixing this does not reverse that issue's rule — it extends the same rule to the path that actually deletes the record.

## Evidence

The rule the crate states for itself, in the state module that owns the removal — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:120-125:
```
//! `DeleteIdentitySource` is what takes one out again: it puts the stored record into
//! `DELETING` ([`IdentitySource::begin_deletion`]) and the clock removes it
//! ([`ComAmazonawsMpaAwsfluffycoreserviceState::remove_identity_source`]), so the
//! record a later read reports — or omits — is this one store's. A source an approver
//! record of a stored team authenticates through is IN USE and is not removed at all
//! ([`ComAmazonawsMpaAwsfluffycoreserviceState::team_arn_using_identity_source`]).
```
The removal that breaks it — state.rs:2623-2637 (no in-use consultation anywhere in `materialize_due`):
```
        // An identity source whose deletion `DeleteIdentitySource` started is gone once
        // that removal comes due: until then it is reported `DELETING`, afterwards
        // `ListIdentitySources` omits it.
        let deleted: Vec<String> = self
            .identity_sources
            .values()
            .filter(|source| {
                source.status == IdentitySourceStatus::Deleting
                    && source.deletion_due.is_some_and(|due| due <= now)
            })
            .map(|source| source.arn.clone())
            .collect();
        for arn in deleted {
            self.remove_identity_source(&arn);
        }
```
The helper the OTHER path calls — src/ops/delete_identity_source.rs:243-256:
```
        // What the decision needs, read out of the store before anything is written: an
        // identity source a team's approvers authenticate through cannot be removed
        // without leaving that team pointing at a source this store no longer holds.
        if let Some(team_arn) = state.team_arn_using_identity_source(&input.identity_source_arn) {
            ...
            return Err(DeleteIdentitySourceErrorModeled::ConflictException(ConflictException { message }).into());
        }
```
and that helper's own doc claim — state.rs:2964-2966:
```
    /// this store holds. It is the one place that reference is resolved, so every
    /// operation deciding whether a source is in use reads the same records.
```
Test coverage: tests/all/identity_source_in_use_by_pending_update.rs and tests/all/identity_source_deletion.rs exercise the rule ONLY at request time ("the delete is refused with the modelled `ConflictException` (409) and the source stays exactly as it was", identity_source_in_use_by_pending_update.rs:6-8); no test covers a reference created inside the `IDENTITY_SOURCE_DELETION_DELAY` window, so nothing pins the current behaviour.

## Steps to Reproduce

1. Build the crate's test harness (tests/all/approval_team_lifecycle.rs `Harness`) and call `record_management_account(engine::emulator_account_id())` so the gated creates are served (approval_team_lifecycle.rs:192-195).
2. `POST /identity-sources` with a valid `IdentitySourceParameters.IamIdentityCenter` → note the returned `IdentitySourceArn` A.
3. `DELETE /identity-sources/{A}` → 200; the record is now `DELETING` with `deletion_due = now + state::IDENTITY_SOURCE_DELETION_DELAY` (500 ms).
4. Within that window, `POST /approval-teams` with one approver whose `PrimaryIdentitySourceArn` is A (or `PATCH /approval-teams/{Arn}` adding such an approver to an existing team) → 200; `GET /approval-teams/{Arn}` reports the approver authenticated by A.
5. Sleep past `state::IDENTITY_SOURCE_DELETION_DELAY`, then `POST /identity-sources/?List` (or `GET /identity-sources/{A}`): A is gone, while `GET /approval-teams/{Arn}` still reports an approver whose `PrimaryIdentitySourceArn` is A — the state state.rs:124-125 says cannot happen.
6. `DELETE /identity-sources/{A}` again → 200 (already absent), so the documented `ConflictException` (409) is now unreachable for a source a stored team is using.

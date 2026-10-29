Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7b6bbea9d5777d5e3200cc7b9f4654c9e364acc7

# An UpdateActions-only request is refused 409 while a version awaits approval, although the same handler says an action is not a change the approvers approve and takes effect where it is asked for

_Filed: 2026-10-29T15:18:24Z_
_Operation: UpdateApprovalTeam | Class: error-response_

> _Note: this issue was identified against the code base at commit `7b6bbea9d5777d5e3200cc7b9f4654c9e364acc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateApprovalTeam`'s "one change at a time" gate refuses EVERY second request while a requested version is outstanding — including a request that carries only `UpdateActions: [SYNCHRONIZE_MFA_DEVICES]` and no configuration member at all.

The handler itself draws exactly that distinction three lines further down: it computes `changes_configuration` from `Description`/`ApprovalStrategy`/`Approvers` alone and states in the comment above it that an update action "asks for no configuration member to change ... which is not a change the team's approvers have anything to approve", and the module doc states that the action "takes effect where it is asked for rather than waiting on an approval". The gate, however, runs BEFORE `changes_configuration` is ever computed and keys only on `outstanding_update()`, so the branch that would apply the action is unreachable while a version is pending. Its refusal message ("already has version 'X' awaiting the team's approval, so it cannot be updated again") asserts a second change the request did not ask for.

The contradiction is sharpest against the crate's own pinned behaviour: an action sent TOGETHER with a configuration change on an ACTIVE team is applied at once (the crate has a test that asserts exactly that, with the reason "The action asks for no configuration member to change, so it is applied at once"), while the identical action sent as its own request moments later — once that configuration change is the pending version — is refused 409. One and the same effect is allowed when bundled and refused when sent alone, which is a classification by request shape rather than by what the request asks for.

Scope of the defect: the gate's condition, not the gate. A second CONFIGURATION change must keep its 409 (that is the case the crate's test pins); a request whose `changes_configuration` is false asks for nothing the approvers decide. Note that the apply-in-place path such a request would then take synchronizes only the live approver set (`ApprovalTeam::synchronize_mfa_devices` walks `self.approvers`), while state.rs says the action synchronizes "for every approver on the team" and the ACTIVE+change branch explicitly synchronizes both the live set and the requested one — so the pending version's approvers need the same treatment.

## Evidence

The gate, unconditional on what the request asks for — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/update_approval_team.rs:259-270:
```
        // One change at a time: a team already waiting on its approvers has a version it
        // is moving to, and a second request would ask them to approve two.
        if let Some(version_id) = outstanding_update {
            return Err(UpdateApprovalTeamErrorModeled::ConflictException(ConflictException {
                message: crate::state::bounded_message(format!(
                    "Approval team '{}' already has version '{version_id}' awaiting the team's \
                     approval, so it cannot be updated again.",
                    input.arn
                )),
            })
            .into());
        }
```

The distinction the SAME handler makes, 70 lines later — src/ops/update_approval_team.rs:337-342:
```
        // Whether this request asks for the team's CONFIGURATION to change. An update
        // action asks for no configuration member to change: it reads each approver's MFA
        // devices back off the identity source, which is not a change the team's approvers
        // have anything to approve.
        let changes_configuration =
            input.description.is_some() || new_strategy.is_some() || input.approvers.is_some();
```
and src/ops/update_approval_team.rs:447-449:
```
            // An update action is applied where it was asked for, to the team as it
            // stands: it changes no member the team's approvers approved.
```

The module doc of this same handler — src/ops/update_approval_team.rs:50-56:
```
//! Every `UpdateActions` entry is APPLIED to the team the update produces: the single
//! modelled action ([`crate::state::SYNCHRONIZE_MFA_DEVICES_ACTION`]) synchronizes the
//! MFA devices of every approver on the team, which `GetApprovalTeam` reports per
//! approver in `MfaMethods`. It changes no member of the configuration the team approved
//! — it reads each approver's devices back off the identity source — so it takes effect
//! where it is asked for rather than waiting on an approval.
```
while the doc sentence the gate implements speaks only of a CHANGE — src/ops/update_approval_team.rs:27-29:
```
//! * A team already in `DELETING` cannot be changed: `ConflictException` (409). Neither
//!   can a team that already has a version awaiting its approvers' approval — one change
//!   at a time.
```

The crate's own pinned behaviour for the bundled form — tests/all/pending_team_update.rs:501-521,526-527 (`an_update_action_applies_at_once_while_the_configuration_change_waits`):
```
/// An update action and a configuration change in ONE request on an ACTIVE team: the
/// action takes effect where it was asked for, under a version of its own, while the
/// configuration change waits for the team under the version the call answered with.
```
```
    // The action asks for no configuration member to change, so it is applied at once:
    // every device is read back off the identity source, under a new live version.
```

No test pins the 409 for an action-only request: the only `UpdateApprovalTeam` 409 assertion sends a configuration member, with that reason — tests/all/pending_team_update.rs:358-365:
```
        // A second change would ask the approvers to approve two versions at once.
        let (status, conflict) = harness
            .update(&arn, &json!({ "Description": "a second change" }))
            .await;
        assert_eq!(
            status, 409,
```
and the `UpdateActions` suite only ever updates teams with nothing pending (tests/all/approver_mfa_synchronization.rs:179-209, :304). So scoping the gate to `changes_configuration` leaves every existing assertion unchanged.

The related half-measure the apply-in-place path would need — src/state.rs:1470-1476:
```
    /// Synchronize the MFA devices of EVERY approver on the team — the effect the
    /// `SYNCHRONIZE_MFA_DEVICES` update action asks for.
    pub fn synchronize_mfa_devices(&mut self) {
        for approver in &mut self.approvers {
            approver.synchronize_mfa_devices();
        }
    }
```
against src/state.rs:107-110: "`UpdateApprovalTeam`'s [`SYNCHRONIZE_MFA_DEVICES_ACTION`] is what asks for that, for every approver on the team" — and src/ops/update_approval_team.rs:441-452, where the ACTIVE+change branch synchronizes BOTH the requested set and the live one.

## Steps to Reproduce

1. Record the caller as the organization's management account so the gated create is served (as tests/all/approval_team_lifecycle.rs:194 does).
2. `POST /approval-teams` with a valid body (one approver, one seeded `Policies` reference, `MofN` M=1), then wait past `state::INVITATION_RESPONSE_DELAY` (500 ms) so `GET /approval-teams/{Arn}` reports `Status: ACTIVE`.
3. `PATCH /approval-teams/{Arn}` with `{"Description": "changed"}` → 200; `GET /approval-teams/{Arn}` now reports `Status: PENDING`, `StatusCode: UPDATE_PENDING_APPROVAL` and a `PendingUpdate`.
4. Within `state::UPDATE_APPROVAL_DELAY` send `PATCH /approval-teams/{Arn}` with `{"UpdateActions": ["SYNCHRONIZE_MFA_DEVICES"]}` — a request carrying no `Description`, no `ApprovalStrategy` and no `Approvers`.
5. Observe 409 `ConflictException` "... already has version '2' awaiting the team's approval, so it cannot be updated again." — although src/ops/update_approval_team.rs:337-342 computes that this request changes no configuration member and :50-56 says such an action "takes effect where it is asked for rather than waiting on an approval". Expected: the action applied in place (every approver's `MfaMethods[].SyncStatus` = `IN_SYNC`), the pending version untouched.
6. For the contrast the crate already pins, repeat from step 2 and send `{"Description": "changed", "UpdateActions": ["SYNCHRONIZE_MFA_DEVICES"]}` as ONE request: 200, and `GET /approval-teams/{Arn}` reports every device `IN_SYNC` (tests/all/pending_team_update.rs:504-547) — the same action, accepted.

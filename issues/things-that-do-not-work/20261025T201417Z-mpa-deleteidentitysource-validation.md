Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c7879b242a52aaa0fabb95009fbb05c28e2cce49

# The in-use conflict check ignores a pending update's approvers, so deleting that source leaves the activated team pointing at a source the store no longer holds

_Filed: 2026-10-25T20:14:17Z_
_Operation: DeleteIdentitySource | Class: validation_

> _Note: this issue was identified against the code base at commit `c7879b242a52aaa0fabb95009fbb05c28e2cce49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteIdentitySource refuses to delete an identity source that an approval team's approvers authenticate through, for the stated reason that removing it "would leave that team pointing at a source this store no longer holds". The resolver it uses, `ComAmazonawsMpaAwsfluffycoreserviceState::team_arn_using_identity_source`, scans ONLY `team.approvers` — the team's LIVE approver set. It does not scan the second approver set the same crate keeps on a team: `ApprovalTeam::pending_update.approvers`, the set `UpdateApprovalTeam` records when an ACTIVE team is asked to approve a change, which `GetApprovalTeam` reports member-for-member (including `PrimaryIdentitySourceArn`) in `PendingUpdate$Approvers` and which `ApprovalTeam::activate_pending_update` then installs AS the team's live approvers.

So: update an ACTIVE team to add an approver authenticated by identity source S (S is now referenced only by the pending update), then call DeleteIdentitySource(S). The conflict check finds no live approver referencing S, answers success, and the clock removes S. When the pending change is activated moments later, the team's live approvers reference an identity source the store no longer holds — exactly the state the ConflictException exists to prevent. Until then, GetApprovalTeam already reports a PendingUpdate approver whose identity source is gone.

The crate itself treats the pending update's approvers as a real approver set everywhere else: `materialize_due` walks "the approvers of the team AND of the change it has been asked to approve" when invitations come due, and `activate_pending_update` promotes them. Only this in-use check looks at one of the two sets.

## Evidence

Rule the handler states — src/ops/delete_identity_source.rs:32-37: "An identity source a stored approval team's approvers authenticate through is IN USE, and removing it would leave that team pointing at a source this store no longer holds: `ConflictException` (409) ... ([`...::team_arn_using_identity_source`] resolves the reference)."

The check — src/ops/delete_identity_source.rs:242: `if let Some(team_arn) = state.team_arn_using_identity_source(&input.identity_source_arn) {`.

The resolver, and its claim to be exhaustive — src/state.rs:2241-2252: "It is the one place that reference is resolved, so every operation deciding whether a source is in use reads the same records." followed by `self.approval_teams.values().find(|team| team.approvers.iter().any(|approver| approver.primary_identity_source_arn == arn))` — `team.approvers` only.

The second approver set the crate keeps, which this check skips — src/state.rs:1966-1975: "The approvers of the team AND of the change it has been asked to approve: an approver that change ADDS is invited when the change is requested, so its response comes due while the change is still waiting." / `for approver in team.approvers.iter_mut().chain(requested_approvers)`. It is written by src/ops/update_approval_team.rs:426-447 (`let mut requested_approvers = new_approvers.unwrap_or(existing_approvers); ... team.request_update(crate::state::PendingTeamUpdate::requested(version, ..., requested_approvers, session_arn, now));`) and promoted to the live set by src/state.rs:842-849 (`self.approvers = update.approvers.into_iter().map(...)`).

Test coverage: tests/all/identity_source_deletion.rs:391-404 (`an_identity_source_in_use_by_an_approval_team_is_refused`) varies only WHERE the reference sits in `team.approvers` ("the team's only approver", "the first of two approvers", "the last of three approvers"); no test references the source from a pending update, so the hole is unpinned. tests/all/pending_team_update.rs:41-50 shows a pending update is created with approver source ARNs in exactly the form `state::identity_source_arn` mints, so a stored source can be the one referenced.

## Steps to Reproduce

1. Record the caller's account as the organization management account (otherwise the creates are refused) and POST /identity-sources twice to store sources S1 and S2; note S2's IdentitySourceArn.
2. POST /approval-teams with one approver authenticated by S1 and wait for the team to report ACTIVE.
3. PATCH /approval-teams/{Arn} with Approvers = [the existing S1 approver, a new approver whose PrimaryIdentitySourceArn is S2]; the response returns the requested VersionId and GET /approval-teams/{Arn} reports PendingUpdate with an approver referencing S2.
4. Immediately DELETE /identity-sources/{S2}: observe HTTP 200 (the source goes to DELETING and is then removed) instead of the ConflictException (409) the same call returns once S2 is referenced by the LIVE approver set.
5. Let the pending update activate, then GET /approval-teams/{Arn}: the live Approvers list now carries PrimaryIdentitySourceArn = S2, and POST /identity-sources/?List no longer holds S2 — the dangling reference the 409 exists to prevent.

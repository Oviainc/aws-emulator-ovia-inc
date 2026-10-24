Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# UpdateApprovalTeam applies the change in place instead of creating a pending update that requires team approval

_Filed: 2026-10-24T00:24:17Z_
_Operation: UpdateApprovalTeam | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (model documentation for com.amazonaws.mpa#UpdateApprovalTeam): "Updates require team approval — Updates to an active team must be approved by the team." So an UpdateApprovalTeam call against an ACTIVE team registers a REQUESTED change: the team keeps its current Description/ApprovalStrategy/Approvers and its current VersionId until the team approves, GetApprovalTeam reports Status=PENDING with StatusCode=UPDATE_PENDING_APPROVAL (later UPDATE_PENDING_ACTIVATION), reports the requested change in the PendingUpdate structure (VersionId, Description, ApprovalStrategy, NumberOfApprovers, Approvers, Status, StatusCode, UpdateInitiationTime) and names the approval session in UpdateSessionArn. Only once the update is approved does the live configuration and VersionId move; the superseded/draft version is what DeleteInactiveApprovalTeamVersion exists to remove.

Actual: src/ops/update_approval_team.rs mutates the stored team immediately — it writes team.description, team.approval_strategy and team.approvers and calls team.bump_version(now) (lines ~300-316) — and returns the already-live new VersionId. No pending update is recorded anywhere in crate::state, so GetApprovalTeam hardcodes update_session_arn: None and pending_update: None (src/ops/get_approval_team.rs lines ~256-259, with the comment "An update is applied in place by UpdateApprovalTeam, so no update session is ever outstanding"), the team's Status stays ACTIVE and neither UPDATE_PENDING_APPROVAL nor UPDATE_PENDING_ACTIVATION is ever reported. A consequence is that TeamStatus::Inactive is never assigned by any code path in src/state.rs (only its wire string exists), so no superseded/draft version ever exists for DeleteInactiveApprovalTeamVersion to delete — that operation can only remove the whole team.

A client that polls GetApprovalTeam for the pending-update handshake (wait for PendingUpdate to appear, follow UpdateSessionArn, wait for Status to return to ACTIVE with the new VersionId) sees an update that is already live and a PendingUpdate that never materialises, and a client that reads the team right after the update sees the NEW configuration where AWS still reports the old one.

## Evidence

Spec: smithy_query(doc, shape=UpdateApprovalTeam) — "Updates require team approval. Updates to an active team must be approved by the team."; GetApprovalTeamResponse declares UpdateSessionArn and PendingUpdate (PendingUpdate carries VersionId/Description/ApprovalStrategy/NumberOfApprovers/Approvers/Status/StatusCode/StatusMessage/UpdateInitiationTime) and ApprovalTeamStatusCode declares UPDATE_PENDING_APPROVAL and UPDATE_PENDING_ACTIVATION — tokens the emulator can never emit. Handler: services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/update_approval_team.rs (in-place writes + bump_version); services/.../src/ops/get_approval_team.rs lines 256-259 (update_session_arn/pending_update hardcoded None). State: src/state.rs has no pending-update field and never assigns TeamStatus::Inactive. Tests: tests/all/approval_team_lifecycle.rs asserts the in-place semantics (update then read back the new values); no test asserts a PendingUpdate/UpdateSessionArn or an UPDATE_PENDING_* status code, so this gap is not already covered.

## Steps to Reproduce

1. Boot the emulator with an MPA store in which an approval team exists and is ACTIVE (the test harness in tests/all/approval_team_lifecycle.rs records the caller's account as the organization management account before CreateApprovalTeam, which is the only way a team can be created today).
2. Call GetApprovalTeam on the team and note Status=ACTIVE, Description, VersionId.
3. Call UpdateApprovalTeam with a new Description (PATCH /approval-teams/{Arn}).
4. Call GetApprovalTeam again. Emulator: the new Description is already live, VersionId has advanced, Status is still ACTIVE, PendingUpdate and UpdateSessionArn are absent. AWS: the previous Description and VersionId are still reported, Status is PENDING with StatusCode UPDATE_PENDING_APPROVAL, PendingUpdate describes the requested change, and UpdateSessionArn names the approval session.
5. Call DeleteInactiveApprovalTeamVersion with the pre-update VersionId: on the emulator it is a silent 200 no-op because only one version per team is ever stored and no version is ever INACTIVE.

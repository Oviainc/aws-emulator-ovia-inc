Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# UpdateApprovalTeam silently drops UpdateActions (SYNCHRONIZE_MFA_DEVICES)

_Filed: 2026-10-24T00:24:17Z_
_Operation: UpdateApprovalTeam | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: UpdateApprovalTeamRequest declares UpdateActions (list<UpdateAction>, length 0..100), documented as "A list of UpdateAction to perform when updating the team", whose single token SYNCHRONIZE_MFA_DEVICES means "Synchronize MFA devices for all approvers on the team". A request naming it has an effect on the team's approvers (their MFA methods are re-read, which GetApprovalTeam reports per approver in GetApprovalTeamResponseApprover$MfaMethods).

Actual: the handler never reads input.update_actions (the field exists in src/ops/types/update_approval_team_types.rs but appears nowhere in src/ops/update_approval_team.rs), the store holds no MFA state at all, and GetApprovalTeam hardcodes mfa_methods: None for every approver. So an UpdateApprovalTeam that asks only for SYNCHRONIZE_MFA_DEVICES is answered 200 with a VersionId and changes nothing observable — the request is accepted and discarded rather than applied or refused.

## Evidence

Spec: smithy_query(shape=UpdateAction) -> enum values [SYNCHRONIZE_MFA_DEVICES]; smithy_query(doc, shape=UpdateApprovalTeamRequest, member=UpdateActions) -> "A list of UpdateAction to perform when updating the team"; GetApprovalTeamResponseApprover declares MfaMethods. Handler: services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/update_approval_team.rs reads only approval_strategy/approvers/description/arn; grep for update_actions in src/ matches only the generated input type. GetApprovalTeam: src/ops/get_approval_team.rs line 240 sets mfa_methods: None unconditionally. No test exercises UpdateActions.

## Steps to Reproduce

1. Boot the emulator with an existing ACTIVE approval team (see the management-account precondition used by tests/all/approval_team_lifecycle.rs).
2. Call UpdateApprovalTeam (PATCH /approval-teams/{Arn}) with body {"UpdateActions": ["SYNCHRONIZE_MFA_DEVICES"]} and no other member.
3. Observe HTTP 200 with an unchanged VersionId and no state change of any kind; GetApprovalTeam reports no MfaMethods for any approver, so the requested action left no trace.

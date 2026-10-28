Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f3bbb57dafae938115053b9b00b150b1f9a288bb

# DisassociateFromAdministratorAccount ignores autoEnableOrganizationMembers=ALL, and the deprecated twin's header still claims the configuration cannot be set

_Filed: 2026-10-25T14:36:04Z_
_Operation: DisassociateFromAdministratorAccount | Class: validation_

> _Note: this issue was identified against the code base at commit `f3bbb57dafae938115053b9b00b150b1f9a288bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents that leaving the administrator relationship is REFUSED while the organization is configured with `autoEnableOrganizationMembers` = `ALL`. The handler's only decisions are "is this detector mine" and "is there a relationship to leave": it calls one state seam and answers 200 on success, never reading the stored organization configuration. The module header quotes the rule and admits the refusal is not implemented. The deprecated twin `DisassociateFromMasterAccount` carries a now-stale version of the same note claiming the configuration "is not modelled yet, so no request can set it", which contradicts its sibling headers and `UpdateOrganizationConfiguration`, which does store it.

## Evidence

Model (smithy_query(query="doc",shape="DisassociateFromAdministratorAccount")): "With <code>autoEnableOrganizationMembers</code> configuration for your organization set to <code>ALL</code>, you'll receive an error if you attempt to disable GuardDuty in a member account."

Handler admits the gap — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/disassociate_from_administrator_account.rs:27-32: "//! The contract below also refuses a disassociation \"with autoEnableOrganizationMembers configuration for your organization set to ALL\". That configuration is now stored — `UpdateOrganizationConfiguration` writes it and `DescribeOrganizationConfiguration` reports it — so the refusal is implementable here; it is this operation's own behaviour and is not modelled yet, so a disassociation succeeds whatever the organization's auto-enablement says."

Handler body — disassociate_from_administrator_account.rs:187-197: `match state.disassociate_detector_from_administrator(&region, &input.detector_id) { None => Err(not_owned(..)), Some(false) => Err(.. NO_ADMINISTRATOR_MESSAGE ..), Some(true) => Ok(DisassociateFromAdministratorAccountOutput {}) }` — no organization configuration is read on any path.

Self-contradiction in the crate: src/ops/disassociate_from_master_account.rs:31-34 still says "That configuration is not modelled yet, so no request can set it and no call can be refused for it", while src/ops/update_organization_configuration.rs:222 writes it (`configuration.auto_enable_members = value.to_string();`) and src/state.rs:6006 holds it (`pub auto_enable_members: String,`).

Test coverage: tests/all/administrator_disassociation.rs and master_account_disassociation.rs cover the success and no-administrator refusals; neither sets `autoEnableOrganizationMembers`, so nothing pins the current acceptance.

## Steps to Reproduce

1. CreateDetector in the request Region.
2. UpdateOrganizationConfiguration on that detector with {"autoEnableOrganizationMembers": "ALL"} and confirm with DescribeOrganizationConfiguration.
3. Establish an administrator relationship for the detector (InviteMembers + AcceptAdministratorInvitation) so there is a relationship to leave.
4. Call DisassociateFromAdministratorAccount with that detectorId.
5. Expected per the model: an error (GuardDuty cannot be disabled in a member account while the organization auto-enables all members). Actual: HTTP 200 and the relationship is dropped.

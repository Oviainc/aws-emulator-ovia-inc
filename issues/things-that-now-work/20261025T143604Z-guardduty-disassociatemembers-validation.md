Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f3bbb57dafae938115053b9b00b150b1f9a288bb

# DisassociateMembers ignores the organization's autoEnableOrganizationMembers=ALL and disassociates anyway

_Filed: 2026-10-25T14:36:04Z_
_Operation: DisassociateMembers | Class: validation_

> _Note: this issue was identified against the code base at commit `f3bbb57dafae938115053b9b00b150b1f9a288bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents that a disassociation is REFUSED while the organization is configured to auto-enable all accounts. The handler never reads the detector's stored organization configuration: it checks only that the `DetectorId` resolves in the request's Region and that each named account has an associated member record, then moves every associated member to `Removed` and answers HTTP 200 with an empty `unprocessedAccounts`. The configuration the refusal needs IS stored by this crate (`UpdateOrganizationConfiguration` writes `OrganizationConfiguration::auto_enable_members`, `DescribeOrganizationConfiguration` reports it), and the handler's own module header says so and admits the refusal is not implemented. Result: with `autoEnableOrganizationMembers` = `ALL`, a caller that should be told to remove the account from the organization first gets a silent success and the member relationship is destroyed.

## Evidence

Model (smithy_query(query="doc",shape="DisassociateMembers")): "With <code>autoEnableOrganizationMembers</code> configuration for your organization set to <code>ALL</code>, you'll receive an error if you attempt to disassociate a member account before removing them from your organization."

Handler admits the gap — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/disassociate_members.rs:20-26: "//! The contract below also refuses a disassociation \"with autoEnableOrganizationMembers configuration for your organization set to ALL ... before removing them from your organization\". That configuration is now stored — `UpdateOrganizationConfiguration` writes it and `DescribeOrganizationConfiguration` reports it — so the refusal is implementable here; it is this operation's own behaviour and is not modelled yet, so a disassociation succeeds whatever the organization's auto-enablement says."

Handler body has no such check — disassociate_members.rs:205-231: `if state.detector_in(&region, &input.detector_id).is_none() { return Err(not_owned(&input.detector_id)); }` … `Some(member) if member.is_associated() => { member.relationship_status = crate::state::RELATIONSHIP_REMOVED.to_string(); member.updated_at_millis = now; }` and the only other outcome is `unprocessed.push(UnprocessedAccount { … NOT_ASSOCIATED_MESSAGE … })`.

The configuration really is persisted — src/ops/update_organization_configuration.rs:222: `configuration.auto_enable_members = value.to_string();` and src/state.rs:6006: `pub auto_enable_members: String,` (doc above it: "`AutoEnableMembers`: whether GuardDuty itself is enabled automatically for the organization's accounts.").

Test coverage: tests/all/organization_configuration.rs stores and reads back `"autoEnableOrganizationMembers": "ALL"` (e.g. lines 347, 1008); tests/all/member_accounts.rs / member_association.rs exercise disassociation, but no test sets `ALL` before disassociating, so nothing pins the current acceptance.

## Steps to Reproduce

1. CreateDetector in the request Region and note the detectorId.
2. CreateMembers with one accountDetails entry (a well-formed 12-digit account id and a valid email).
3. UpdateOrganizationConfiguration on that detector with {"autoEnableOrganizationMembers": "ALL"}; DescribeOrganizationConfiguration confirms ALL is stored.
4. Call DisassociateMembers with that detectorId and accountIds=[the member account].
5. Expected per the model: an error (the account must be removed from the organization first). Actual: HTTP 200 with unprocessedAccounts=[] and GetMembers now reports relationshipStatus=Removed.

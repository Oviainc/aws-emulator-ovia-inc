Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f3bbb57dafae938115053b9b00b150b1f9a288bb

# StopMonitoringMembers ignores the organization's autoEnableOrganizationMembers=ALL and stops monitoring anyway

_Filed: 2026-10-25T14:36:04Z_
_Operation: StopMonitoringMembers | Class: validation_

> _Note: this issue was identified against the code base at commit `f3bbb57dafae938115053b9b00b150b1f9a288bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents that stopping monitoring for organization member accounts is REFUSED while `autoEnableOrganizationMembers` is `ALL`. The handler never consults the detector's stored organization configuration: after the detector-ownership check it moves every associated member to `Disabled` and answers HTTP 200. The handler's own module header quotes the rule, states that the configuration is now stored by this crate, and admits the refusal is missing, so a stop succeeds whatever the organization's auto-enablement says.

## Evidence

Model (smithy_query(query="doc",shape="StopMonitoringMembers")): "With <code>autoEnableOrganizationMembers</code> configuration for your organization set to <code>ALL</code>, you'll receive an error if you attempt to stop monitoring the member accounts in your organization."

Handler admits the gap — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/stop_monitoring_members.rs:23-28: "//! The contract below also refuses a stop \"with autoEnableOrganizationMembers configuration for your organization set to ALL\". That configuration is now stored — `UpdateOrganizationConfiguration` writes it and `DescribeOrganizationConfiguration` reports it — so the refusal is implementable here; it is this operation's own behaviour and is not modelled yet, so a stop succeeds whatever the organization's auto-enablement says."

Handler body has no such check — stop_monitoring_members.rs:196-228: `if state.detector_in(&region, &input.detector_id).is_none() { return Err(not_owned(&input.detector_id)); }` … `if member.relationship_status != crate::state::RELATIONSHIP_DISABLED { member.relationship_status = crate::state::RELATIONSHIP_DISABLED.to_string(); member.updated_at_millis = now; }`, with the only alternative being an `unprocessed` entry for a non-member.

The configuration is persisted — src/ops/update_organization_configuration.rs:222: `configuration.auto_enable_members = value.to_string();`; src/state.rs:6006: `pub auto_enable_members: String,`.

Test coverage: tests/all/member_monitoring.rs exercises stop/restart of monitoring but never sets `autoEnableOrganizationMembers`, so no test pins the current acceptance.

## Steps to Reproduce

1. CreateDetector and CreateMembers with one member account.
2. UpdateOrganizationConfiguration with {"autoEnableOrganizationMembers": "ALL"}.
3. Call StopMonitoringMembers with that detectorId and accountIds=[the member account].
4. Expected per the model: an error. Actual: HTTP 200 with unprocessedAccounts=[] and the member's relationshipStatus becomes Disabled.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# CreateMembers ignores the organization's auto-enable settings, so a member's feature configuration contradicts DescribeOrganizationConfiguration

_Filed: 2026-10-27T08:19:55Z_
_Operation: CreateMembers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states that CreateMembers applies the organization's auto-enable settings to every account the request names, new or existing. The handler never reads the detector's stored organization configuration: a NEW member record is given `crate::state::default_features(now)` (which sets every opt-in feature to DISABLED) and an EXISTING member's feature list is not touched at all — only its email and relationship status can move. So after `UpdateOrganizationConfiguration` turns a feature's auto-enable on (ALL/NEW), a `CreateMembers` for an account produces a member whose `GetMemberDetectors` configuration has that feature DISABLED, contradicting what `DescribeOrganizationConfiguration` reports for the same organization.

This is not a feature the crate lacks the parts for: the organization's per-feature auto-enable is persisted and readable (`OrganizationConfiguration::feature_auto_enable`), the member-side writer exists (`apply_member_feature_configurations` / `MemberRecord::set_feature`, used by `UpdateMemberDetectors`), the stored org feature set is exactly the member-configurable set (`MEMBER_FEATURES = ORG_FEATURES = ORG_FEATURE_VALUES`), and sibling member handlers already consult `detector.organization` before they act (`DeleteMembers`, `DisassociateMembers`, `StopMonitoringMembers`). CreateMembers is the one member operation in the family that never looks at it.

Secondary, same handler: its module header still says the invitation flow is unmodelled because "`InviteMembers` is still a scaffold", which the crate now contradicts — `src/ops/invite_members.rs` is implemented and moves a member to `Invited`.

## Evidence

Model (smithy_query(query="doc",shape="com.amazonaws.guardduty#CreateMembers")): "When you use CreateMembers as an Organizations delegated administrator, GuardDuty applies your organization's auto-enable settings to the member accounts in this request, irrespective of the accounts being new or existing members. For more information about the existing auto-enable settings for your organization, see DescribeOrganizationConfiguration." The same sentence is quoted in the handler's own header: services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_members.rs:72-75.

Handler, new member — src/ops/create_members.rs:286-292: "// The member account's detector starts in the / // configuration a detector created without a / // `Features` list gets; `UpdateMemberDetectors` is / // what the administrator changes it with. / features: crate::state::default_features(now)," and src/state.rs:5351-5364: `pub fn default_features(now: i64) -> Vec<FeatureRecord> { reported_features() … status: if OPT_IN_FEATURES.contains(&name) { DISABLED.to_string() } else { ENABLED.to_string() } … }`.

Handler, existing member — src/ops/create_members.rs:249-261: `let mut changed = false; if existing.email != detail.email { … } if !existing.is_associated() { … }` — no feature write on any path; the whole handler contains no reference to `organization` (grep for `auto_enable|organization_configuration` in src/ops matches delete_members.rs:201, disassociate_members.rs:216, stop_monitoring_members.rs:207, update_organization_configuration.rs, describe_organization_configuration.rs — never create_members.rs).

The configuration is stored and reportable — src/state.rs:6129 `pub auto_enable_members: String,`, src/state.rs:6132 `pub features: Vec<OrgFeatureRecord>,`, src/state.rs:6173-6178 `pub fn feature_auto_enable(&self, name: &str) -> &str { … }`; the member-side writer already exists — src/state.rs:2395 `pub fn set_feature(&mut self, name: &str, status: &str, now: i64)` and src/state.rs:5658 `pub fn apply_member_feature_configurations(…)`; and the two sets coincide — src/state.rs:5826 `pub const MEMBER_FEATURES: &[&str] = crate::ops::types::shared_types::ORG_FEATURE_VALUES;` vs src/state.rs:6057 `pub const ORG_FEATURES: &[&str] = crate::ops::types::shared_types::ORG_FEATURE_VALUES;`.

Stale header contradicted by the crate — src/ops/create_members.rs:20-24: "What this does NOT model yet … the invitation flow (`InviteMembers` is still a scaffold), so a recorded member's `RelationshipStatus` is `Created` until an invitation is modelled." against src/ops/invite_members.rs:240-245: `if member.relationship_status != crate::state::RELATIONSHIP_INVITED { member.relationship_status = crate::state::RELATIONSHIP_INVITED.to_string(); member.invited_at_millis = Some(now); member.updated_at_millis = now; }`.

Test coverage: tests/all/member_creation.rs and tests/all/member_detector_configuration.rs exercise CreateMembers and GetMemberDetectors, and tests/all/organization_configuration.rs exercises UpdateOrganizationConfiguration/DescribeOrganizationConfiguration, but no test creates a member after setting an organization feature's auto-enable, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateDetector in us-east-1 and note the detectorId.
2. UpdateOrganizationConfiguration with autoEnableOrganizationMembers=ALL and features=[{name: <an opt-in OrgFeature, e.g. EBS_MALWARE_PROTECTION>, autoEnable: ALL}].
3. DescribeOrganizationConfiguration — the feature reports autoEnable=ALL.
4. CreateMembers with accountDetails=[{accountId: 111122223333, email: member@example.com}] → HTTP 200, unprocessedAccounts=[].
5. GetMemberDetectors with accountIds=[111122223333]. Expected per the model: the member's configuration reflects the organization's auto-enable settings. Actual: the feature is DISABLED (the default feature set), identical to what it would be with no organization configuration at all; repeating CreateMembers for the now-existing member changes nothing either.

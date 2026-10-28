Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8269d8069d160c27d0acf36557f2407fe5d81546

# UpdateMemberDetectors writes a member's feature switches without the organization auto-enable rule CreateMembers applies, so a member can end up DISABLED for a feature the organization auto-enables for ALL accounts

_Filed: 2026-10-28T23:17:37Z_
_Operation: UpdateMemberDetectors | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8269d8069d160c27d0acf36557f2407fe5d81546`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two operations in this crate write the same stored value — `MemberRecord::features` — and only one of them respects the organization's auto-enable settings.

`CreateMembers` reads `detector.organization` and runs `crate::state::apply_organization_auto_enable(..)` over every account it touches, NEW and EXISTING alike, and its own header states the invariant that write exists to maintain: "So a member's `GetMemberDetectors` configuration agrees with what `DescribeOrganizationConfiguration` reports for the same organization."

`UpdateMemberDetectors` writes the request's statuses straight onto the same member record (`apply_member_data_sources` + `apply_member_feature_configurations`) and never looks at `detector.organization` at all — the file contains no reference to the organization configuration (grep for `organization|auto_enable|autoEnable` in `src/ops/update_member_detectors.rs` returns no match). So after the administrator has set `autoEnableOrganizationMembers=ALL` and a feature's `autoEnable=ALL`, one `UpdateMemberDetectors` with `status=DISABLED` leaves that member reporting DISABLED for a feature `DescribeOrganizationConfiguration` still reports as auto-enabled for every account — the exact disagreement the invariant above forbids — and a later `CreateMembers` for the very same account silently flips it back to ENABLED. The two writers of one stored switch disagree about whether the organization setting wins.

This is not a feature the crate lacks the parts for: the helper exists and is already called from the sibling create (twice, once per branch), the organization configuration is persisted on the detector and reachable from this handler (it already holds `detector` mutably), and three other member handlers (`DeleteMembers`, `DisassociateMembers`, `StopMonitoringMembers`) consult `detector.organization` before they act. `UpdateMemberDetectors` is now the only member-write operation that ignores it.

## Evidence

MODEL — the organization status that says what must hold for every account, `smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`:
```
ALL: Indicates that all accounts in the organization have the feature enabled automatically. This includes NEW accounts that join the organization and accounts that may have been suspended or removed from the organization in GuardDuty.
```
(and for the other direction: "NONE: Indicates that the feature will not be automatically enabled for any account in the organization. The administrator must manage the feature for each account individually.")

THE RULE AND THE INVARIANT, stated by the sibling that writes the same field — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_members.rs:11-17:
```
//! Every account the request names takes the organization's auto-enable settings,
//! "irrespective of the accounts being new or existing members" (the modelled
//! contract below): a feature this administrator's organization configuration
//! auto-enables is ENABLED for the account, while one it auto-enables for no account
//! is left to be managed per account and keeps whatever the member's configuration
//! already said. So a member's `GetMemberDetectors` configuration agrees with what
//! `DescribeOrganizationConfiguration` reports for the same organization.
```
and it follows it — create_members.rs:233-236:
```
        // The organization's auto-enable settings, read before any member is written
        // so every account of one call is given the same configuration. Taken as a
        // copy because it is a sibling field of the members being written.
        let organization = detector.organization.clone();
```
create_members.rs:274-280 (EXISTING member branch):
```
                    // An EXISTING member takes the organization's auto-enable
                    // settings too, which is what `irrespective of the accounts
                    // being new or existing members` says. ...
                    crate::state::apply_organization_auto_enable(existing, &organization, now);
```
create_members.rs:312-316 (NEW member branch) calls the same helper.

THE HELPER IT SKIPS — src/state.rs:5874-5900:
```
/// Write the organization's auto-enable settings onto one member account's stored
/// configuration.
///
/// A feature the organization auto-enables — any `OrgFeatureStatus` other than
/// [`AUTO_ENABLE_NONE`] ... — is ENABLED for the account, together with every additional
/// configuration that feature auto-enables.
pub fn apply_organization_auto_enable(
    member: &mut MemberRecord,
    organization: &OrganizationConfiguration,
    now: i64,
) {
```

THE VIOLATION — src/ops/update_member_detectors.rs:208-223, the whole write, with no organization read anywhere in the file:
```
            match detector.members.get_mut(account_id) {
                Some(member) if member.is_associated() => {
                    // The deprecated `DataSources` view is applied first so an
                    // explicit `Features` entry for the same switch wins — the order
                    // `CreateDetector` and `UpdateDetector` apply the two views in.
                    crate::state::apply_member_data_sources(
                        member,
                        input.data_sources.as_ref(),
                        now,
                    );
                    crate::state::apply_member_feature_configurations(
                        member,
                        input.features.as_ref(),
                        now,
                    );
                }
```

SIBLING MEMBER HANDLERS DO CONSULT IT — src/ops/delete_members.rs:195-207:
```
        // An organization that auto-enables GuardDuty for every one of its accounts
        // owns their member relationships, so none of their records can be dropped
        // here whatever the request names ...
        if detector.organization.auto_enables_all_members() {
```
(the same read at src/ops/disassociate_members.rs and src/ops/stop_monitoring_members.rs).

TEST COVERAGE: tests/all/member_organization_auto_enable.rs asserts the invariant only through `CreateMembers` — "Rule 3: a member's configuration agrees with the organization's own read" (tests/all/member_organization_auto_enable.rs:416-492) creates members under a mixed organization and never calls `UpdateMemberDetectors` afterwards. tests/all/member_detector_configuration.rs does drive `UpdateMemberDetectors` with `status: "DISABLED"` (e.g. :467, :583, :675), but always against a detector whose organization configuration is untouched (every feature's `autoEnable` is `NONE`, src/state.rs:6482-6487), where the auto-enable rule forces nothing — so nothing pins the current behaviour and no existing assertion changes.

## Steps to Reproduce

1. CreateDetector in us-east-1; note the detectorId.
2. UpdateOrganizationConfiguration with `autoEnableOrganizationMembers: "ALL"` and `features: [{"name": "RDS_LOGIN_EVENTS", "autoEnable": "ALL"}]` (RDS_LOGIN_EVENTS is one of the opt-in features a member starts DISABLED, so it discriminates).
3. DescribeOrganizationConfiguration — the feature reports `autoEnable: ALL`.
4. CreateMembers with `accountDetails: [{accountId: "811122223333", email: "member@example.com"}]` → 200.
5. GetMemberDetectors for that account — RDS_LOGIN_EVENTS is ENABLED, as the organization read says it must be for all accounts.
6. UpdateMemberDetectors with `accountIds: ["811122223333"]`, `features: [{"name": "RDS_LOGIN_EVENTS", "status": "DISABLED"}]` → 200, unprocessedAccounts empty.
7. GetMemberDetectors again — RDS_LOGIN_EVENTS is now DISABLED while DescribeOrganizationConfiguration still reports `autoEnable: ALL` for it: the member configuration no longer agrees with the organization read, the agreement create_members.rs:11-17 states.
8. Call CreateMembers again for the same accountId/email → 200, then GetMemberDetectors: the feature is ENABLED again. Two operations that write the same switch disagree about whether the organization's ALL auto-enablement wins.

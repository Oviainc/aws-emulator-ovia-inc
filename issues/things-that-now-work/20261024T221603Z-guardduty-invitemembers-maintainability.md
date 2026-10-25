Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: d2641f8668c2a4abe0c9d800c33fcdc9b555a45a

# The AccountIds well-formedness refusal is hand-copied into six member handlers instead of living in one state helper

_Filed: 2026-10-24T22:16:03Z_
_Operation: InviteMembers | Class: maintainability_

> _Note: this issue was identified against the code base at commit `d2641f8668c2a4abe0c9d800c33fcdc9b555a45a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Six handlers open with byte-identical copies of the same rule — "if any `AccountIds` entry is not a well-formed account id, refuse the whole call with `invalid_parameter_message(ACCOUNT_ID_MEMBER)` before the store is touched" — and two more (`CreateCustomDetectionRuleOrgConfiguration`, `UpdateCustomDetectionRuleOrgConfiguration`) carry the chained include/exclude variant of it. Every other cross-handler rule on this service is sourced once from `crate::state` (`detector_id_rejection`, `not_a_member_message`, `validate_feature_configurations`, `validate_finding_criteria`), so these copies are the one place where one handler can be fixed and the other five left behind. Folding them onto a single `crate::state` reader that returns the message leaves every response and every existing test unchanged.

## Evidence

services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/invite_members.rs:201-209:
```
        if input
            .account_ids
            .iter()
            .any(|account_id| !crate::state::is_account_id(account_id))
        {
            return Err(crate::scaffolded::Scaffolded::invalid_input(
                crate::state::invalid_parameter_message(crate::state::ACCOUNT_ID_MEMBER),
            ));
        }
```
src/ops/get_member_detectors.rs:167-175 (the same block, only the error constructor differs):
```
        if input
            .account_ids
            .iter()
            .any(|account_id| !crate::state::is_account_id(account_id))
        {
            return Err(bad_request(crate::state::invalid_parameter_message(
                crate::state::ACCOUNT_ID_MEMBER,
            )));
        }
```
The other copies: src/ops/disassociate_members.rs:197-205, src/ops/stop_monitoring_members.rs:189-197, src/ops/get_members.rs:175-183, src/ops/update_member_detectors.rs:173-181, plus the chained variant in src/ops/create_custom_detection_rule_org_configuration.rs:176-184 and src/ops/update_custom_detection_rule_org_configuration.rs:182-190.

Contrast the single-source rules these same handlers already use: `crate::state::detector_id_rejection` (src/state.rs:1487) and `crate::state::not_a_member_message` (src/state.rs:1289).

Existing coverage that must stay green: tests/all/member_invitation.rs, tests/all/member_accounts.rs, tests/all/member_creation.rs, tests/all/member_monitoring.rs, tests/all/member_detector_configuration.rs and tests/all/client_rejections.rs all assert the 400 + error class for a malformed accountId.

## Steps to Reproduce

1. grep -n 'any(|account_id| !crate::state::is_account_id' services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/*.rs
2. Observe six identical pre-store validation blocks (invite_members, disassociate_members, stop_monitoring_members, get_members, get_member_detectors, update_member_detectors) plus two chained include/exclude variants
3. Note that every other shared refusal on this service is sourced once from src/state.rs, so these copies are free to drift

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# GetMemberDetectors answers an empty `members` list, a value its own modelled `MemberDataSourceConfigurations` (len 1..50) does not admit

_Filed: 2026-10-29T09:03:12Z_
_Operation: GetMemberDetectors | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetMemberDetectors` builds its `members` list by walking the requested `AccountIds` and pushing a configuration only for an account the detector holds an ASSOCIATED member record for; every other account goes to `unprocessedAccounts`. So a request whose accounts are all non-members (an id nothing recorded, the administrator's own account, or a member a disassociation already moved) answers HTTP 200 with `members: []`.

That is a response the model forbids. `MemberDataSourceConfigurations` — the shape the output member targets — is `length {min: 1, max: 50}`, and the handler's own generated header states that bound verbatim. Nothing in the framework enforces an output length (the handler is the only place that can honour it), and nothing in the handler, in the crate, or in the test suite states a reason for emitting a value outside it.

This is specific to this one operation: it is the ONLY output list of this service whose modelled minimum is 1. Every sibling batch/listing output admits the empty list — `Members` (the `GetMembers` counterpart, same request shape, same per-account `UnprocessedAccounts` reporting) is `len 0..50`, as are `UnprocessedAccounts`, `Findings`, `FindingIds`, `DetectorIds`, `FilterNames`, `IpSetIds`, `ThreatIntelSetIds`, `TrustedEntitySetIds`, `ThreatEntitySetIds`, `Invitations`, `InvestigationSummaries`, and `AdminAccounts` is `len 0..1`. The crate treats `GetMemberDetectors` and `GetMembers` identically — same loop, same `seen` dedup, same unprocessed push, an empty `Vec` when nothing matched — without noticing that only one of the two is modelled to allow an empty answer. A strictly-validating client deserialising `GetMemberDetectorsResponse` against the model therefore rejects a body this emulator considers a success.

## Evidence

THE MODELLED BOUND, quoted in the handler's OWN header — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/get_member_detectors.rs:63-67:
```
//! ## Output
//!
//! * `MemberDataSourceConfigurations` (MemberDataSourceConfigurations, len 1..50)
//!   **required** — An object that describes which data sources are enabled for a member
//!   account.
```
and from the model itself, `smithy_query(query="shape", shape="MemberDataSourceConfigurations,Members")`:
```
"MemberDataSourceConfigurations": {"type": "list", "length": {"min": 1, "max": 50}, "member": "MemberDataSourceConfiguration"}
"Members": {"type": "list", "length": {"min": 0, "max": 50}, "member": "Member"}
```
so the sibling read's list admits the empty answer and THIS one does not.

THE CODE THAT EMITS THE EMPTY LIST — src/ops/get_member_detectors.rs:181-205:
```
        let mut configurations = Vec::new();
        let mut unprocessed = Vec::new();
        let mut seen = ::std::collections::BTreeSet::new();
        for account_id in &input.account_ids {
            ...
            match detector.members.get(account_id) {
                // Only an account ASSOCIATED with this administrator is monitored
                // under it, so only that account's configuration is reported here.
                Some(member) if member.is_associated() => configurations
                    .push(crate::state::member_data_source_configuration_view(member)),
                _ => unprocessed.push(UnprocessedAccount {
                    account_id: account_id.clone(),
                    result: crate::state::not_a_member_message(&administrator),
                }),
            }
        }

        Ok(GetMemberDetectorsOutput {
            member_data_source_configurations: configurations,
            unprocessed_accounts: unprocessed,
        })
```
There is NO branch, and no comment anywhere in the file, that mentions the `len 1..50` minimum (`grep -n 'len|empty|1\.\.50|Present' src/ops/get_member_detectors.rs` matches only the four generated header lines above). The output type is a bare `Vec` with no non-empty invariant — src/ops/types/get_member_detectors_types.rs:21-26:
```
#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#GetMemberDetectorsResponse")]
pub struct GetMemberDetectorsOutput {
    pub member_data_source_configurations: Vec<MemberDataSourceConfiguration>,
    pub unprocessed_accounts: Vec<UnprocessedAccount>,
}
```

THE SIBLING THE CRATE WROTE THE SAME WAY, where empty IS modelled — src/ops/get_members.rs:187-207 (identical loop, identical empty `Vec`, and its own header states `Members (Members, len 0..50)`):
```
        let mut members = Vec::new();
        ...
        Ok(GetMembersOutput {
            members,
            unprocessed_accounts: unprocessed,
        })
```
Elsewhere in this crate, when a list is modelled to allow the empty answer the handler SAYS SO — e.g. src/ops/list_members.rs:217-219 ("Present and empty when the detector holds no member: an empty list is the answer") and src/ops/list_malware_protection_plans.rs:184-186. `get_member_detectors.rs` carries no such note, because its list is not one of those.

TEST COVERAGE — the empty-list path is exercised, but nothing pins the emptiness as deliberate. tests/all/member_detector_configuration.rs:640-691 drives exactly one non-member account through `get_member_detectors` and asserts only the refusal classification and the unprocessed row:
```
// Rule 5: an account that is not an associated member is REPORTED, not refused.
...
            } else {
                get_member_detectors(&router, &detector_id, &[account_id]).await
            };
            assert_eq!(status, 200, ...);
            assert_eq!(unprocessed_ids(&body), vec![account_id.to_string()], ...);
            assert!(member_of(&body, account_id).is_none(), ...);
```
The stated rule is about REPORTING versus REFUSING the account; the test never asserts that `members` is present-and-empty, and no comment in the suite gives a reason for crossing the modelled minimum. (That the suite asserts 200 also shows the framework does not validate the response against `OUTPUT_SCHEMA`, so the out-of-bound body goes out on the wire.)

## Steps to Reproduce

1. Create a detector: `POST /detector` with `{"enable": true}`; note the returned `detectorId`.
2. Call `POST /detector/{detectorId}/member/detector/get` with `{"accountIds": ["111111111111"]}` — an account id this detector holds no member record for.
3. The response is HTTP 200 carrying `"members": []` and one `unprocessedAccounts` entry (get_member_detectors.rs:181-205 pushes nothing onto `configurations`).
4. Compare against the model: `GetMemberDetectorsResponse.MemberDataSourceConfigurations` targets `MemberDataSourceConfigurations`, modelled `length {min: 1, max: 50}` — the body carries a list of length 0.
5. Repeat with the administrator's own account id, and with an account id that `POST /detector/{detectorId}/member/disassociate` has already moved: both take the same `_ =>` arm and produce the same empty `members` list.
6. Contrast with the sibling `POST /detector/{detectorId}/member/get` (GetMembers) given the same accountIds: it also answers `"members": []`, which IS admitted there because `Members` is modelled `len 0..50`.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# GetFindings reports one finding twice when findingIds repeats an id, unlike every sibling batch read in the crate, which processes a repeated id once

_Filed: 2026-10-28T12:15:12Z_
_Operation: GetFindings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetFindings` resolves its answer by mapping the request's `findingIds` list straight onto records (`input.finding_ids.iter().filter_map(|id| detector.findings.get(id))`), so an id named twice resolves twice and the modelled `Findings` list carries two copies of the same finding. Every other batch-by-id operation in this crate normalises the request's id list to a SET before it answers, with the reasoning written out beside the code: GetMembers, GetRemainingFreeTrialDays, DeleteMembers, UpdateMemberDetectors, InviteMembers and the shared invitation taker all keep a `seen` set and skip a repeated entry so it is reported once. GetFindings is the one read that does not, so the same request shape gets two different normalisation rules depending on which batch read answers it.

## Evidence

The handler — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/get_findings.rs:161-167: "// An id the detector does not hold is absent from the result, not an error:\n// `Findings` is `len 0..50` and carries only what was found." followed by `let mut found: Vec<&crate::state::FindingRecord> = input.finding_ids.iter().filter_map(|id| detector.findings.get(id)).collect();` — no de-duplication, so a repeated id yields the record twice.

The sibling rule it skips — src/ops/get_members.rs:189-194: `let mut seen = ::std::collections::BTreeSet::new(); for account_id in &input.account_ids { // `AccountIds` names a SET of accounts: a repeated id is reported once.\n if !seen.insert(account_id.as_str()) { continue; } ...`. The identical normalisation plus comment is in src/ops/get_remaining_free_trial_days.rs:186-192 ("a repeated id is reported once, on either list"), src/ops/delete_members.rs:216-223, src/ops/update_member_detectors.rs:201-207, src/ops/invite_members.rs:223-229 and src/state.rs:594-598.

Model shapes involved (smithy_query(operation="GetFindings")): input `FindingIds` is `len 0..50`, output `Findings` is `len 0..50` — so the duplicate stays inside the modelled bound and is served as a 200 with a repeated entry rather than refused.

Test coverage: every GetFindings case passes ids taken from a ListFindings answer (tests/all/finding_field_catalog.rs:401, tests/all/detector_lifecycle.rs:672, tests/all/sample_finding_catalog.rs:75), i.e. distinct ids; no test sends a repeated findingId, so nothing pins the current duplicate output.

## Steps to Reproduce

1. CreateDetector in the request Region, then CreateSampleFindings so the detector holds findings.
2. ListFindings and take one finding id, e.g. F.
3. POST /detector/{detectorId}/findings/get with {"findingIds": [F, F]}.
4. Expected, per the normalisation every sibling batch read in this crate applies: the repeated id is processed once and `findings` carries ONE entry for F.
5. Actual: `findings` carries TWO identical entries for F.
6. Compare with the sibling: POST /detector/{detectorId}/member/get with {"accountIds": [A, A]} reports A exactly once.

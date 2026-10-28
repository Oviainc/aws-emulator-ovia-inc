Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# UpdateFindingsFeedback rewrites a finding without moving the finding's UpdatedAt, unlike the archive write that shares the same state module

_Filed: 2026-10-28T01:40:50Z_
_Operation: UpdateFindingsFeedback | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateFindingsFeedback` writes two fields of the stored finding record (`user_feedback` and `feedback_comments`) and then deliberately leaves `FindingRecord::updated_at_millis` where it was. The model member it leaves stale is documented as the record's own update clock — Finding.UpdatedAt is "The time and date when the finding was last updated" — and the ACTIVITY clock is a different member (`Service.EventLastSeen`), which this crate stores separately in `event_last_seen_millis`.

The crate already states the correct rule for the other finding-metadata write: `ArchiveFindings`/`UnarchiveFindings` go through `ComAmazonawsGuarddutyGuarddutyapiserviceState::set_findings_archived_in` -> `Detector::set_findings_archived`, whose doc says the archive state "is a property of the RECORD, not an observation of the activity, so only `FindingRecord::updated_at_millis` moves" — and the code moves it only when the value actually changes. Submitting feedback is the same kind of write (a property of the record, not an observation of the activity), yet it moves neither clock, so a client cannot see from `GetFindings` that the finding changed at all, and `findingCriteria`/`sortCriteria` on `updatedAt` (ListFindings/GetFindings) order the finding as if nothing had happened.

The justification comment in the handler rests on a claim about a sibling handler that is false in this crate: it says `UpdatedAt` is "the clock of the DETECTED ACTIVITY" and that "`GetFindings` renders `Service.EventLastSeen` from it", while `get_findings.rs` explicitly renders `EventLastSeen` from `event_last_seen_millis` and says so. Note that the crate's established shape for this write is "move UpdatedAt only when a value actually changes" (`set_findings_archived`, `update_ip_set.rs`'s `changed` flag), so a re-submission of the same feedback value would still be a true no-op.

## Evidence

Model (`smithy_query(query="doc",shape="Finding",member="UpdatedAt")`): "The time and date when the finding was last updated."
Model (`smithy_query(query="doc",shape="Service",member="EventLastSeen")`): "The last-seen timestamp of the activity that prompted GuardDuty to generate this finding."

The crate's rule for the sibling metadata write — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:2646-2669:
```
    /// Put every named finding this detector holds into the `archived` state,
    /// recording `now` as the `UpdatedAt` of each finding the call actually moves.
...
    /// The archive state is a property of the RECORD, not an observation of the
    /// activity, so only [`FindingRecord::updated_at_millis`] moves:
    /// [`FindingRecord::event_last_seen_millis`] stays where the last observation
    /// left it.
    pub fn set_findings_archived(&mut self, finding_ids: &[String], archived: bool, now: i64) {
        for id in finding_ids {
            if let Some(finding) = self.findings.get_mut(id) {
                if finding.archived != archived {
                    finding.archived = archived;
                    finding.updated_at_millis = now;
```

The handler, writing the record and skipping the same field — src/ops/update_findings_feedback.rs:172-186:
```
        for finding_id in &input.finding_ids {
            if let Some(finding) = detector.findings.get_mut(finding_id) {
                finding.user_feedback = Some(input.feedback.clone());
                finding.feedback_comments = input.comments.clone();
            }
        }

        // `Finding.UpdatedAt` is deliberately NOT moved here. In this service it is
        // the clock of the DETECTED ACTIVITY — `CreateSampleFindings` advances it when
        // it aggregates another occurrence of a finding type, `GetFindings` renders
        // `Service.EventLastSeen` from it, ...
```
(no `updated_at_millis` assignment anywhere in the file; the whole handler body is lines 156-191.)

The sibling read contradicts that comment's premise — src/ops/get_findings.rs:216-220:
```
            // The ACTIVITY's clock, not the record's: a change to the finding's own
            // metadata (its archive state, its feedback) is not an observation of
            // the activity, so `EventLastSeen` comes from the record's
            // `event_last_seen_millis` rather than from its `UpdatedAt`.
            event_last_seen: Some(crate::state::iso8601(record.event_last_seen_millis)),
```

Test coverage: tests/all/findings_feedback.rs:215-223 pins the current behaviour — "// Rule 3: feedback is not an observation of the activity, so the finding's clocks and its archived flag do not move." with `for member in ["createdAt", "updatedAt", ...] { assert_eq!(after[member], before[member], ...) }` — but its stated reason is the claim the model sentence above contradicts (`UpdatedAt` is the record's update clock, not the activity clock). The opposite rule is asserted for the sibling write in tests/all/findings_archive_state.rs:20: "3. A finding's `updatedAt` moves when the call changes its archive state and stays" (and lines 341-343 pin that a repeated, no-change archive does NOT move it).

## Steps to Reproduce

1. CreateDetector with {"enable": true}.
2. POST /detector/{detectorId}/findings/create with {"findingTypes": ["Recon:EC2/Portscan"]}, then POST /detector/{detectorId}/findings and POST /detector/{detectorId}/findings/get to read the finding; note `updatedAt` and `service.userFeedback` (absent).
3. POST /detector/{detectorId}/findings/feedback with {"findingIds": [<id>], "feedback": "USEFUL"} (200).
4. GET the finding again with /findings/get: `service.userFeedback` is now "USEFUL" — the stored record changed — but `updatedAt` is byte-for-byte the value from step 2, so no client can tell from Finding.UpdatedAt that the finding was updated, and ListFindings sorted by `updatedAt` still orders it as unchanged.
5. For contrast, POST /detector/{detectorId}/findings/archive for the same id and re-read: `updatedAt` has moved (src/state.rs:2661-2669), although archiving is the same kind of record-metadata change.

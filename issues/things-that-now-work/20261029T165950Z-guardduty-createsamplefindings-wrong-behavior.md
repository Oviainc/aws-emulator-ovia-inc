Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# CreateSampleFindings adds (and evicts) records in the detector's `findings` collection without moving the detector's `UpdatedAt`, unlike every other create of a record the detector holds

_Filed: 2026-10-29T16:59:50Z_
_Operation: CreateSampleFindings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states one rule for the collections that hang off `crate::state::Detector`: a call that changes WHICH records a detector holds changes the detector itself, so `GetDetector`'s `UpdatedAt` moves to the instant of that call. Every sibling create writes `detector.updated_at_millis = now` and says so in a comment that names the rule as a CLASS rule (`CreateFilter`, `CreateIPSet`, `CreateThreatIntelSet`, `CreateThreatEntitySet`, `CreateTrustedEntitySet`, `CreatePublishingDestination`, `CreateInvestigation`), and the state module places `findings` in exactly the same ownership class as `investigations` and `filters`.

`CreateSampleFindings` is the one operation that changes which records `detector.findings` holds, and it never touches `detector.updated_at_millis`: it inserts the generated `FindingRecord` into `detector.findings` (create_sample_findings.rs:253) and, when the retention window is full, REMOVES the oldest finding from the same map (create_sample_findings.rs:260-270), with `now` already in hand from create_sample_findings.rs:193. So `GetDetector` reports the same `UpdatedAt` before and after the detector gained — or silently lost — findings, while the identical change to any other collection of the same detector moves it.

Scope: only the INSERT and the eviction are the defect. The aggregation branch (create_sample_findings.rs:201-214), which only bumps an existing finding's `count`/`updatedAt`/`eventLastSeen`, leaves the set of records the detector holds unchanged and so is the `UpdateIPSet` case the crate deliberately exempts ("the set of sub-resources it holds is unchanged").

## Evidence

THE RULE, as the sibling create of another detector-owned collection states it — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_investigation.rs:274-280:
```
        // The detector itself changed: it holds one more sub-resource than before,
        // which is the change `UpdatedAt` — "the last-updated timestamp for the
        // detector" (`smithy_query(operation="GetDetector")`) — records, the same way
        // every other create of a sub-resource the detector holds records it. A
        // refused create, and a replay that initiates nothing, return above and so
        // move nothing.
        detector.updated_at_millis = now;
```
and src/ops/create_publishing_destination.rs:271-272:
```
        // The detector itself changed: it holds one more sub-resource than before.
        detector.updated_at_millis = now;
```

FINDINGS ARE ONE OF THOSE COLLECTIONS — src/state.rs:2394-2395 and src/state.rs:2413-2416:
```
    /// This detector's findings, keyed by finding id.
    pub findings: BTreeMap<String, FindingRecord>,
...
    /// This detector's investigations, keyed by investigation id. They hang off the
    /// detector like its findings and filters, so `DeleteDetector` drops them with
    /// it.
    pub investigations: BTreeMap<String, InvestigationRecord>,
```
(the same phrasing at src/state.rs:2441-2443 for `members`).

THE VIOLATION — src/ops/create_sample_findings.rs:252-270 is the whole write, and the file contains no `detector.updated_at_millis` at all (a grep for `updated_at_millis` in it matches only create_sample_findings.rs:211, the FINDING's own clock in the aggregation branch, and :241, the new record's own field):
```
            if let Some(detector) = state.detector_in_mut(&region, &input.detector_id) {
                detector.findings.insert(finding_id, record);
                // GuardDuty retains a finding for 90 days and then drops it. This
                // emulator has no 90-day-long process, so it bounds the store by the
                // same SHAPE of rule — a window over the most recent findings — and
                // drops the oldest once the window is full.
                while detector.findings.len() > MAX_RETAINED_FINDINGS_PER_DETECTOR {
                    ...
                    detector.findings.remove(&oldest);
                }
            }
```

THE CRATE STATES THE RULE AS A CLASS RULE OVER THE WHOLE SET — tests/all/detector_sub_resource_creates.rs:10-19:
```
//! The rule under test, asserted over the SET of sub-resource families a detector
//! holds rather than over one of them: a call that changes the set of sub-resources a
//! detector holds changes the detector, so `GetDetector`'s `UpdatedAt` moves to the
//! instant of that change — and a call that changes nothing (an idempotent replay, a
//! refused request, a read) moves it nowhere. ...
//! This suite covers the families whose create takes a `Format` and a `Location`;
//! `filter_detector_timestamp` and `investigation_detector_timestamp` assert the same
//! rule over the two families whose creates carry other members.
```

TEST COVERAGE: no suite reads `GetDetector`'s `updatedAt` across a `CreateSampleFindings` — a repo-wide grep for `detector_updated_at` across tests/all lists only the ipset/threatintelset/threatentityset/trustedentityset/publishingDestination/filter/investigation suites plus the clock suites (tests/all/detector_sub_resource_creates.rs:169, tests/all/filter_detector_timestamp.rs:165, tests/all/investigation_detector_timestamp.rs:38, tests/all/detector_sub_resource_deletes.rs:176, tests/all/refused_create_clock.rs:251, tests/all/no_op_mutation_clock.rs:37). The finding suites (tests/all/sample_finding_catalog.rs, tests/all/finding_record_update_clock.rs, tests/all/findings_archive_state.rs) assert only each FINDING's own `updatedAt` (e.g. tests/all/finding_record_update_clock.rs:246-257), so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateDetector in us-east-1 (`POST /detector` with `enable: true`) and note `detectorId`.
2. `GET /detector/{detectorId}` and record `updatedAt` as T0.
3. CreateSampleFindings (`POST /detector/{detectorId}/findings/create` with `{"findingTypes": ["Recon:EC2/Portscan"]}`) → HTTP 200; `POST /detector/{detectorId}/findings` (ListFindings) now reports the generated finding, so the detector holds a record it did not hold before.
4. `GET /detector/{detectorId}` again. Expected, by the rule tests/all/detector_sub_resource_creates.rs:10-15 states for every collection a detector holds and create_investigation.rs:274-280 implements for the sibling collection: `updatedAt` has moved past T0. Actual: still T0.
5. For contrast, repeat with a trusted entity set (`POST /detector/{detectorId}/trustedentityset`): the detector's `updatedAt` moves.

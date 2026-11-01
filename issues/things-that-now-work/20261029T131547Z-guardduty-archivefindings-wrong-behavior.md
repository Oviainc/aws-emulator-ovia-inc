Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7bcaea858dd8788316443d36e8dbe675c4b82465

# A findings-metadata write that changes no finding still consumes an instant of the state's monotonic clock, which the crate's own no-op rule says it must not

_Filed: 2026-10-29T13:15:47Z_
_Operation: ArchiveFindings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7bcaea858dd8788316443d36e8dbe675c4b82465`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis` is not a read — it writes `self.last_millis = max(wall, last_millis + 1)` — so every call permanently advances the store's "this resource changed now" counter by at least a millisecond. The shared findings-metadata write path reads it BEFORE it knows whether any finding will move.

`set_findings_archived_in` (src/state.rs:471) resolves the detector and every named id, then calls `tick_millis()` at :484 and only afterwards calls `Detector::set_findings_archived`, which is the function that decides whether anything changes: it writes only `if finding.archived != archived`, and its own documentation states that a repeat must change nothing at all. So `ArchiveFindings` on findings that are already archived (and `UnarchiveFindings` on findings that are already unarchived) answers 200, correctly moves no finding's `UpdatedAt` — and still burns an instant of the service clock. The same holds for the other two no-op shapes the crate's own test suite enumerates for this path: an EMPTY `FindingIds` list (`len 0..50`, so empty is a legal request) and a batch naming only ids the detector does not hold. `set_findings_feedback_in` (src/state.rs:508) has the identical ordering — `tick_millis()` at :519 before `set_findings_feedback`, whose doc says "A submission that leaves every stored value where it was moves nothing at all, which is what makes a re-submission of the same feedback a true no-op" — so `UpdateFindingsFeedback` behaves the same way.

The consequence is the one the crate's clock suites were written to catch: a burst of repeated archives inside one wall-clock millisecond pushes `last_millis` past the wall clock, after which the next genuinely accepted create or update stamps a `CreatedAt`/`UpdatedAt` in the future relative to real time — a value no caller's activity produced. The crate states and implements the correct ordering in `UpdateCustomDetectionRuleOrgConfiguration`, which reads `tick_millis()` INSIDE its `if changed` block. This is distinct from the already-filed clock issues: `20261029T102128Z-guardduty-updatetrustedentityset-wrong-behavior` names five UPDATE handlers (UpdateTrustedEntitySet, UpdateThreatEntitySet, UpdateIPSet, UpdatePublishingDestination, UpdateMalwareScanSettings) and none of the findings-metadata path; the three earlier ones concern REFUSED calls. The fix (move the tick to the paths that write) changes no successful response.

## Evidence

WHAT THE CLOCK IS — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:206-222:
```
    /// The current instant in epoch milliseconds, strictly greater than every
    /// instant this state handed out before.
    ///
    /// Call it only from a MUTATING operation: it is the "this resource changed
    /// now" clock, and a reporting operation must not advance it. ...
    pub fn tick_millis(&mut self) -> i64 {
        let wall = now_millis();
        let next = if wall > self.last_millis { wall } else { self.last_millis.saturating_add(1) };
        self.last_millis = next;
        next
    }
```

THE VIOLATION — src/state.rs:471-490, the whole helper `ArchiveFindings`/`UnarchiveFindings` call (src/ops/archive_findings.rs:157 `state.set_findings_archived_in(...)`, src/ops/unarchive_findings.rs:154); the tick at :484 runs before the write at :488 decides anything:
```
        let Some(detector) = self.detector_in(region, detector_id) else {
            return Err(detector_id_rejection(detector_id));
        };
        if let Some(unheld) = detector.first_unheld_finding_id(finding_ids) {
            return Err(finding_id_rejection(unheld));
        }
        let now = self.tick_millis();
        let Some(detector) = self.detector_in_mut(region, detector_id) else {
            return Err(detector_id_rejection(detector_id));
        };
        detector.set_findings_archived(finding_ids, archived, now);
```
its own doc (src/state.rs:468-470) accounts only for the REFUSED case: "Every id is resolved BEFORE anything is written, and the detector is resolved before [`Self::tick_millis`] runs, so a refused request moves nothing and advances nothing."

THE RULE THE SAME MODULE STATES FOR THE NO-OP — src/state.rs:2833-2837 and the gate at :2846:
```
    /// Archiving and unarchiving are the two directions of ONE write ... A finding
    /// already in the requested state is left untouched — `UpdatedAt` included,
    /// which is what makes a repeated call a true no-op — and an id this detector
    /// does not hold moves nothing ...
        for id in finding_ids {
            if let Some(finding) = self.findings.get_mut(id) {
                if finding.archived != archived {
```
and for the feedback twin — src/state.rs:2859-2869 (`UpdateFindingsFeedback`, src/ops/update_findings_feedback.rs:182, ticks at src/state.rs:519 the same way):
```
    /// An id this detector does not hold writes nothing and is not an error ...
    /// A submission that leaves every stored value where it was moves nothing at
    /// all, which is what makes a re-submission of the same feedback a true no-op.
```

THE SIBLING THAT ORDERS IT CORRECTLY — src/ops/update_custom_detection_rule_org_configuration.rs:245-251:
```
        if changed {
            // A MUTATION: the instant this hands out is the configuration's new
            // `UpdatedAt` ... An update
            // asking for the state the configuration is already in changes nothing,
            // so it advances neither ...
            let now = state.tick_millis();
```

WHERE THE PUSHED-FORWARD CLOCK BECOMES OBSERVABLE — a later accepted sub-resource create stamps the detector from the same counter, src/ops/create_trusted_entity_set.rs:235 (`let now = state.tick_millis();`) and :273 (`detector.updated_at_millis = now;`), reported by `GetDetector`.

TEST COVERAGE: tests/all/finding_record_update_clock.rs:9-26 states the rule and enumerates exactly the no-op rows — "a write that changes a stored property of a finding records the instant of that write as the finding's own `UpdatedAt` ... and moves nothing for a write that changes no stored value ... the ways a submission can change a stored value or change none: a first value, a different value, a repeat, ... an empty batch and a batch naming only ids the detector does not hold" — but it asserts only each FINDING's `updatedAt`, never the service clock. tests/all/refused_mutation_clock.rs:11-27 is the suite that measures the clock ("a refused call changes nothing at all — it neither moves a resource nor consumes an instant"), and every row of its `rows()` is a refused call; no row is an accepted no-op archive or feedback submission, so nothing pins this path.

## Steps to Reproduce

1. Create a detector in us-east-1 (`POST /detector` with `enable: true`) and note `detectorId`.
2. Populate it: `POST /detector/{detectorId}/findings/create` with `findingTypes: ["Recon:EC2/Portscan"]`, then `POST /detector/{detectorId}/findings` (ListFindings) to get the finding id.
3. Archive it once: `POST /detector/{detectorId}/findings/archive` with `findingIds: [id]` → 200. `GET`/ListFindings sorted by `updatedAt` shows the finding's own `updatedAt` at the instant of this call.
4. Issue a burst of several thousand further `POST /detector/{detectorId}/findings/archive` calls with the same `findingIds` (or with `findingIds: []`, or with an id the detector does not hold). Each answers 200 and — correctly, per src/state.rs:2833-2837 — leaves every finding's `updatedAt` where it was. But each has already run `state.tick_millis()` at src/state.rs:484, so each advanced `last_millis` by at least 1 ms.
5. Immediately create a sub-resource on the same detector (`POST /detector/{detectorId}/trustedentityset` with a name, format and location) and read the detector with `GET /detector/{detectorId}`.
6. Expected, by the ordering `UpdateCustomDetectionRuleOrgConfiguration` uses (src/ops/update_custom_detection_rule_org_configuration.rs:245-251): the detector's `updatedAt` is the wall-clock instant of that create. Actual: it sits ahead of real wall-clock time by roughly the number of no-op archive calls issued in the preceding millisecond.
7. Repeat with `POST /detector/{detectorId}/findings/feedback` submitting the same `feedback` value twice for the same id: the second call changes no stored value (src/state.rs:2867-2869) yet still consumes an instant through src/state.rs:519.

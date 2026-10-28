Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 67d3ef025eb7c6b921e8b895d06c7f32bf688664

# DeletePublishingDestination and UpdateMalwareScanSettings advance the state's monotonic clock before their refusals, each on a justification a sibling handler in the same crate disproves

_Filed: 2026-10-28T22:42:59Z_
_Operation: DeletePublishingDestination | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `67d3ef025eb7c6b921e8b895d06c7f32bf688664`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states one ordering rule for a mutating operation: classify every refusal FIRST, over an immutable borrow, and only then read `ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis()`, so a refused request leaves nothing behind — "not the service clock either". The state module itself restates the rule for the findings writes ("the detector is resolved before `Self::tick_millis` runs, so a refused request moves nothing and advances nothing"), and `DeleteThreatIntelSet`, `UpdateFilter`, `UpdateMemberDetectors` and `StartMonitoringMembers` all implement it by resolving the detector immutably, returning the refusals, ticking, and only then re-resolving mutably.

Two handlers do the opposite, and each one's comment states a REASON that another handler of the same crate disproves:

1. `DeletePublishingDestination` ticks as the first thing it does under the lock, then refuses an unresolvable `DetectorId` and a `DestinationId` the detector does not hold. Its comment justifies that with "Taken before the lookups the way every other detector sub-resource delete on this service takes it" — but `DeleteThreatIntelSet` is a detector sub-resource delete that does the opposite and says why ("Every refusal is classified BEFORE the clock moves ... and not the service clock either").

2. `UpdateMalwareScanSettings` ticks before resolving the detector, justified with "`tick_millis` and the detector record both borrow the store mutably, so the clock is taken first and the detector is looked up ONCE". The sibling `UpdateMemberDetectors` — same shape of request, same store — looks the detector up TWICE for exactly this reason: `state.detector_in(..)` to refuse, then `tick_millis()`, then `state.detector_in_mut(..)`. So the borrow conflict the comment cites is not a reason the ordering has to be this way; the crate already shows the alternative.

`tick_millis` is not a read: it writes `self.last_millis = max(wall, last_millis + 1)`. Every refused call therefore permanently advances the store's "this resource changed now" counter by at least a millisecond, and a burst of refused calls inside one wall-clock millisecond pushes `last_millis` past the wall clock — after which the next SUCCESSFUL create or update stamps a `CreatedAt`/`UpdatedAt` in the future relative to real time, a value no caller's activity produced.

Note on scope: the two existing open issues on this rule name other handlers only — `20261028T152108Z-guardduty-deleteipset-wrong-behavior.md` enumerates `DeleteIPSet`, `DeleteTrustedEntitySet`, `DeleteThreatEntitySet`, `DeleteFilter`, `UpdateIPSet` and the two accepts, and `20261028T180651Z-guardduty-createipset-wrong-behavior.md` the two creates. Neither names `DeletePublishingDestination` or `UpdateMalwareScanSettings`, which are the two whose own comments carry a stated justification the crate contradicts. The fix is the ordering the siblings already show, and it changes no successful response.

## Evidence

THE RULE, stated by a sibling detector sub-resource delete — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/delete_threat_intel_set.rs:147-149, with its two refusals at :150-169 and the tick only afterwards at :171-174:
```
        // Every refusal is classified BEFORE the clock moves, so a refused delete
        // leaves nothing observable behind: not the detector, not the id, and not
        // the service clock either.
        let Some(detector) = state.detector_in(&region, &input.detector_id) else {
...
        // Past the refusals this is a MUTATION, so it advances this state's clock:
        let now = state.tick_millis();
```
and restated in the state module itself — src/state.rs:468-470 (`set_findings_archived_in`):
```
    /// Every id is resolved BEFORE anything is written, and the detector is resolved
    /// before [`Self::tick_millis`] runs, so a refused request moves nothing and
    /// advances nothing.
```

WHAT THE CLOCK IS — src/state.rs:209-222:
```
    /// Call it only from a MUTATING operation: it is the "this resource changed
    /// now" clock, and a reporting operation must not advance it. ...
    pub fn tick_millis(&mut self) -> i64 {
        let wall = now_millis();
        let next = if wall > self.last_millis { wall } else { self.last_millis.saturating_add(1) };
        self.last_millis = next;
        next
    }
```

VIOLATION 1 — src/ops/delete_publishing_destination.rs:140-179 ticks first and refuses after, on a claim about "every other" delete that delete_threat_intel_set.rs:147-174 above disproves:
```
140        let mut state = state.lock();
141        // A MUTATION, so it advances this state's clock: the instant it hands out is
142        // when the deletion landed. Taken before the lookups the way every other
143        // detector sub-resource delete on this service takes it — a refused call
144        // leaves every record exactly as it was, and the counter itself is not
145        // something any read reports.
146        let now = state.tick_millis();
147
148        let Some(detector) = state.detector_in_mut(&region, &input.detector_id) else {
...
153            return Err(bad_request(crate::state::detector_id_rejection(
...
171        if detector
172            .publishing_destinations
173            .remove(&input.destination_id)
174            .is_none()
175        {
176            return Err(bad_request(crate::state::destination_id_rejection(
177                &input.destination_id,
178            )));
```

VIOLATION 2 — src/ops/update_malware_scan_settings.rs:176-185:
```
176        let mut state = state.lock();
177        // `tick_millis` and the detector record both borrow the store mutably, so
178        // the clock is taken first and the detector is looked up ONCE: there is one
179        // refusal path for an id this account does not hold, not two.
180        let now = state.tick_millis();
181        let Some(detector) = state.detector_in_mut(&region, &input.detector_id) else {
182            return Err(bad_request(crate::state::not_owned_message(
183                crate::state::DETECTOR_ID_MEMBER,
184            )));
185        };
```
THE SIBLING THAT DISPROVES THAT REASON — src/ops/update_member_detectors.rs:184-198 does the double lookup to keep the refusal ahead of the tick:
```
184        let mut state = state.lock();
...
187        if state.detector_in(&region, &input.detector_id).is_none() {
188            return Err(not_owned(&input.detector_id));
189        }
190        // The configuration changes now, so the clock advances once for the whole
191        // call: every switch this call moves carries the same instant.
192        let now = state.tick_millis();
193        let Some(detector) = state.detector_in_mut(&region, &input.detector_id) else {
194            // Unreachable: the detector was resolved above and the lock has not been
195            // released since.
```
(the same two-step shape at src/ops/start_monitoring_members.rs:188-202 and src/ops/update_filter.rs:183-205.)

TEST COVERAGE of the affected paths: tests/all/publishing_destination_delete.rs and tests/all/malware_scan_settings.rs exercise both operations and their refusals; a repo-wide grep for `clock`/`last_millis` across tests/all shows every clock assertion is about clock-OWED status transitions (e.g. publishing_destination_delete.rs:301-310, ip_set_lifecycle.rs:296-346), and none asserts that a refused call advanced the counter — so nothing pins the current ordering.

## Steps to Reproduce

1. Create a detector in us-east-1 (`POST /detector`) and a publishing destination under it.
2. Issue a burst of `DELETE /detector/{detectorId}/publishingDestination/{unknownButWellFormedDestinationId}` calls in quick succession. Each answers 400 `InvalidInputException` via `destination_id_rejection` (delete_publishing_destination.rs:176), but each one has already reached `state.tick_millis()` at delete_publishing_destination.rs:146, so each advances `last_millis` by at least 1 ms.
3. Do the same with `POST /detector/{bogusButWellFormedDetectorId}/malware-scan-settings`: each answers 400 through `not_owned_message` at update_malware_scan_settings.rs:182 after ticking at :180.
4. Immediately create a sub-resource on the good detector (e.g. `POST /detector/{detectorId}/filter`) and read it back with `GetFilter`, or read the detector with `GetDetector`.
5. Observe `createdAt`/`updatedAt` sitting ahead of real wall-clock time by roughly the number of refused calls made in the preceding millisecond — state the refused calls were supposed to leave untouched.
6. Contrast with the same burst against `DELETE /detector/{detectorId}/threatintelset/{unknownId}`: delete_threat_intel_set.rs refuses at :150-169 before ticking at :174, so refused calls there consume no tick.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# CreateIPSet and CreatePublishingDestination read the mutation clock before their refusals, breaking the rule the sibling creates state in their own comments

_Filed: 2026-10-28T18:06:51Z_
_Operation: CreateIPSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states one ordering rule for a create: decide every refusal over an immutable borrow FIRST, then read `tick_millis()`, so a refused create leaves nothing behind — "neither mints an id nor advances the clock". `CreateTrustedEntitySet`, `CreateThreatEntitySet` and `CreateThreatIntelSet` implement it.

`CreateIPSet` and `CreatePublishingDestination` do the opposite: each takes the lock and immediately ticks, then goes on to refuse an unresolvable `DetectorId` and a create past the per-detector ceiling. `tick_millis` is not a read — it writes `self.last_millis = max(wall, last_millis + 1)` — so every refused call permanently advances the store's "this resource changed now" counter, and a burst of refused creates inside one wall-clock millisecond pushes `last_millis` past the wall clock, after which the next SUCCESSFUL create or update stamps a `CreatedAt`/`UpdatedAt` in the future relative to real time.

Note on scope: the existing open issue on this rule (`20261028T152108Z-guardduty-deleteipset-wrong-behavior.md`) enumerates the deletes, the two updates and the two accepts; it names neither of these two CREATE handlers, which are the ones whose own sibling creates spell the rule out. The fix is the ordering those siblings already show — resolve and refuse over an immutable borrow, then tick, then re-resolve mutably — and it changes no successful response.

## Evidence

THE RULE, stated by a sibling create of the same kind of detector sub-resource — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_trusted_entity_set.rs:203-209:
```
        // Everything that can refuse the request is decided over an IMMUTABLE
        // borrow, so a refused create neither mints an id nor advances the clock.
        let Some(detector) = state.detector_in(&region, &input.detector_id) else {
            return Err(bad_request(crate::state::detector_id_rejection(
                &input.detector_id,
            )));
        };
```
(the tick only afterwards, create_trusted_entity_set.rs:227 `let now = state.tick_millis();`); the same two lines at create_threat_entity_set.rs:203-204 with the tick at :228; and create_threat_intel_set.rs:270-275 "...and\n        // a refused create returned before the clock moved at all." with its refusals at :175-218 and the tick at :220.

WHAT THE CLOCK IS — src/state.rs:209-221:
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

VIOLATION 1 — src/ops/create_ip_set.rs:177-214 ticks first, refuses after:
```
177        let mut state = state.lock();
178        let now = state.tick_millis();
...
191            let Some(detector) = state.detector_in(&region, &input.detector_id) else {
...
194                return Err(detector_rejection(&input.detector_id));
...
208            if replayed.is_none()
209                && detector.live_ip_set_count() >= crate::state::IP_SETS_PER_DETECTOR
210            {
211                return Err(crate::scaffolded::Scaffolded::invalid_input(
212                    crate::state::ip_set_limit_message(),
```

VIOLATION 2 — src/ops/create_publishing_destination.rs:171-209:
```
171        let mut state = state.lock();
172        let now = state.tick_millis();
...
186            let Some(detector) = state.detector_in(&region, &input.detector_id) else {
187                return Err(bad_request(crate::state::detector_id_rejection(
...
202            if replayed.is_none()
203                && detector.publishing_destinations.len()
204                    >= crate::state::PUBLISHING_DESTINATIONS_PER_DETECTOR
205            {
206                return Err(bad_request(
207                    crate::state::publishing_destination_limit_message(),
```

TEST COVERAGE of these paths: tests/all/ip_set_lifecycle.rs, tests/all/detector_sub_resource_creates.rs, tests/all/publishing_destination_lifecycle.rs and tests/all/client_rejections.rs exercise both creates and their refusals; none asserts that a refused create advanced the clock, so nothing pins the current ordering.

## Steps to Reproduce

1. Create a detector in us-east-1 and note the detectorId.
2. Issue a burst of `POST /detector/{bogusButWellFormedDetectorId}/ipset` calls (each answers 400 `InvalidInputException` through `detector_rejection`). Each one reaches `state.tick_millis()` at create_ip_set.rs:178 before the refusal at :194, so each advances `last_millis` by at least 1 ms.
3. Immediately create a real sub-resource on the good detector (e.g. `POST /detector/{detectorId}/ipset`) and read it back (GetIPSet / GetDetector).
4. Observe the detector's `updatedAt` (and a subsequently created filter's `createdAt`) sitting ahead of real wall-clock time by roughly the number of refused calls made in the preceding millisecond — state the refused calls were supposed to leave untouched.
5. Contrast with the same burst against `POST /detector/{bogus}/threatentityset`: create_threat_entity_set.rs refuses at :205-226 before ticking at :228, so refused calls there consume no tick.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a0740c4b10bc61036cc0b642f74c6c28951c1b9

# An update that changes nothing still consumes an instant of the state's monotonic clock, which the crate's own no-op rule says it must not

_Filed: 2026-10-29T10:21:28Z_
_Operation: UpdateTrustedEntitySet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a0740c4b10bc61036cc0b642f74c6c28951c1b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis` is not a read — it writes `self.last_millis = max(wall, last_millis + 1)` — so every call permanently advances the store's "this resource changed now" counter by at least a millisecond. The crate states, and in one handler implements, the rule that a call which changes nothing must not advance it: `UpdateCustomDetectionRuleOrgConfiguration` reads `tick_millis()` INSIDE its `if changed` block precisely so that "an update asking for the state the configuration is already in changes nothing, so it advances neither", and `UpdateCustomDetectionRuleAssociation` returns early, before any tick, when the request asks for the mode the association already has.

Five sibling update handlers break that rule. Each one computes a `changed` flag and gates the stored `UpdatedAt` on it — so each one agrees that a request writing back the values the record already holds changed nothing — yet each reads `tick_millis()` BEFORE `changed` is known:

* `UpdateTrustedEntitySet` — tick at :220, `changed` gate at :290
* `UpdateThreatEntitySet` — tick at :220, `changed` gate at :290
* `UpdateIPSet` — tick at :199, `changed` gate at :238
* `UpdatePublishingDestination` — tick at :178, `changed` gate at :220
* `UpdateMalwareScanSettings` — tick at :187, `changed` gate at :218

The consequence is the one the crate's own refused-clock test suites were written to catch: a burst of no-op updates inside one wall-clock millisecond pushes `last_millis` past the wall clock, after which the next genuinely accepted create or update stamps a `CreatedAt`/`UpdatedAt` in the future relative to real time — a value no caller's activity produced. It is distinct from the three already-filed clock issues, which all concern REFUSED calls (`20261028T152108Z-guardduty-deleteipset-wrong-behavior`, `20261028T180651Z-guardduty-createipset-wrong-behavior`, `20261028T224259Z-guardduty-deletepublishingdestination-wrong-behavior`); none of them names a no-op 200 path. The fix is the ordering `UpdateCustomDetectionRuleOrgConfiguration` already shows and it changes no successful response.

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

THE RULE, stated and implemented by a sibling update — src/ops/update_custom_detection_rule_org_configuration.rs:237-260:
```
        let mut changed = record.mode != input.mode;
        record.mode = input.mode.clone();
        if scope_given { ... }
        if changed {
            // A MUTATION: the instant this hands out is the configuration's new
            // `UpdatedAt` and what the roll-out transition is owed from. An update
            // asking for the state the configuration is already in changes nothing,
            // so it advances neither — which is what makes a replayed PUT
            // (`@idempotent`) observably a no-op.
            let now = state.tick_millis();
```
and by the other rule-following sibling — src/ops/update_custom_detection_rule_association.rs:178-184, which returns before the tick at :210:
```
        // Asking for the mode the association is already in changes nothing, so
        // neither the store nor `UpdatedAt` moves — which is what `@idempotent` on
        // this PUT ... means observably.
        if current_mode == input.mode {
            return Ok(UpdateCustomDetectionRuleAssociationOutput {});
        }
```

THE VIOLATION — src/ops/update_trusted_entity_set.rs:217-220 ticks first:
```
        // Past the refusals this is a MUTATION, so it advances this state's clock:
        // the instant it hands out is what the set's `UpdatedAt` records and what a
        // transition `Activate` starts is owed from.
        let now = state.tick_millis();
```
while the SAME handler, at :287-292, states that a no-op changed nothing:
```
        // `UpdatedAt` moves only when this call really changed the set, so an
        // update that writes back the values the set already held reports what it
        // did: nothing.
        if changed {
            set.updated_at_millis = now;
        }
```
The same shape in the four other handlers: src/ops/update_threat_entity_set.rs:220 vs :290-292; src/ops/update_ip_set.rs:196-199 ("Past the refusals this is a MUTATION: the instant this hands out is the IPSet's new `UpdatedAt`") vs :238-240; src/ops/update_publishing_destination.rs:175-178 vs :220-221; src/ops/update_malware_scan_settings.rs:187 vs :218.

WHERE THE PUSHED-FORWARD CLOCK BECOMES OBSERVABLE — a later accepted sub-resource create stamps the detector from the same counter, src/ops/create_trusted_entity_set.rs:235 + :273 (`let now = state.tick_millis();` ... `detector.updated_at_millis = now;`), and `GetDetector` reports that `UpdatedAt`.

Test coverage: tests/all/refused_mutation_clock.rs:11-27 states the rule and the measurement — "a mutating operation reads the service's monotonic 'this resource changed now' clock only on the path that actually writes, so a refused call changes nothing at all — it neither moves a resource nor consumes an instant — and the `CreatedAt`/`UpdatedAt` a LATER accepted call stamps therefore still reports the wall-clock instant of that change rather than one pushed into the future by the refusals before it" — but every row of `rows()` (:93 onward) is a REFUSED call; no row sends an accepted update whose body repeats the stored values, so nothing pins the no-op path. tests/all/trusted_entity_set_update.rs and tests/all/threat_entity_set_update.rs exercise these updates but assert only the record's own `updatedAt`.

## Steps to Reproduce

1. Create a detector in us-east-1 (`POST /detector`) and note `detectorId`.
2. Create a trusted entity set under it (`POST /detector/{detectorId}/trustedentityset` with a name, format, location and `activate: true`) and note `trustedEntitySetId`.
3. `GET /detector/{detectorId}/trustedentityset/{id}` and record the stored `name`, `location` and status.
4. Issue a burst of several thousand `POST /detector/{detectorId}/trustedentityset/{id}` calls in quick succession, each body repeating exactly the stored `name` and `location` (and no `activate`). Each answers 200 and, correctly, leaves the set's own `updatedAt` where it was (src/ops/update_trusted_entity_set.rs:290-292) — but each has already run `state.tick_millis()` at :220, so each advanced `last_millis` by at least 1 ms.
5. Immediately create another sub-resource on the same detector (e.g. `POST /detector/{detectorId}/filter`) and read the detector with `GET /detector/{detectorId}`.
6. Expected, per src/ops/update_custom_detection_rule_org_configuration.rs:245-251 ("An update asking for the state the configuration is already in changes nothing, so it advances neither"): the detector's `updatedAt` is the wall-clock instant of the create. Actual: it sits ahead of real wall-clock time by roughly the number of no-op updates issued in the preceding millisecond.
7. Contrast with the same burst of no-op `PUT /custom-detection-rule/org-configuration/{ruleId}` calls repeating the stored mode and scope: that handler ticks only inside `if changed`, so the burst consumes no instant and the following create stamps a present-time timestamp.

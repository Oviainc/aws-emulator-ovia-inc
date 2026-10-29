Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: b45fe0b1030ed2b932d59ccf686ccc68d031b528

# Refused deletes/updates advance the state's monotonic clock, the one thing the crate's own rule says a refusal must not touch

_Filed: 2026-10-28T15:21:08Z_
_Operation: DeleteIPSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b45fe0b1030ed2b932d59ccf686ccc68d031b528`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states a single, explicit ordering rule for every mutating operation: classify every refusal FIRST, and only then read `ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis()`, so that a refused request leaves nothing behind — "not the service clock either". The ThreatIntelSet family, `UpdateTrustedEntitySet`, `UpdateThreatEntitySet`, `UpdateFilter` and `CreateTrustedEntitySet` all implement it: they resolve the detector and the sub-resource id over an IMMUTABLE borrow, return the refusals, and only afterwards tick.

A second group of handlers does the opposite: it ticks the clock as the first thing it does under the lock and then returns the same refusals. `DeleteIPSet`, `DeleteTrustedEntitySet`, `DeleteThreatEntitySet`, `DeleteFilter`, `UpdateIPSet`, `AcceptInvitation` and `AcceptAdministratorInvitation` all consume a tick on a request they go on to refuse — an unresolvable `DetectorId`, an `IpSetId`/`TrustedEntitySetId`/`FilterName` the detector does not hold, or an invitation that is not pending.

`tick_millis` is not a pure read: it writes `self.last_millis = max(wall_clock, last_millis + 1)`. So each refused call permanently advances the store's "this resource changed now" counter by at least one millisecond. A burst of refused `DeleteIPSet` calls inside one wall-clock millisecond pushes `last_millis` ahead of the wall clock, and the next SUCCESSFUL create or update then stamps a `CreatedAt`/`UpdatedAt` that is in the future relative to real time — a value no caller's activity produced. The sibling that states the rule exists precisely so this cannot happen.

`AcceptInvitation` contradicts itself inside one file: its comment at the tick says the instant "is what the accepted relationship moves the detector's `UpdatedAt` to", yet the tick is taken two lines before the detector is even resolved and ~20 lines before the handler knows whether any relationship is accepted at all.

The fix is the ordering the ThreatIntelSet family already shows: resolve and refuse over an immutable borrow, then `tick_millis()`, then re-resolve mutably. Responses on the success paths are unchanged.

## Evidence

THE RULE, stated by the sibling delete of the same kind of detector sub-resource — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/delete_threat_intel_set.rs:147-149:
```
        // Every refusal is classified BEFORE the clock moves, so a refused delete
        // leaves nothing observable behind: not the detector, not the id, and not
        // the service clock either.
```
and it follows it — the two refusals are at delete_threat_intel_set.rs:150-169, the tick only afterwards at :171-174:
```
        // Past the refusals this is a MUTATION, so it advances this state's clock:
        // the instant it hands out is when the deletion was accepted, and what the
        // `DELETED` transition is owed from.
        let now = state.tick_millis();
```
Same rule restated in src/ops/update_threat_intel_set.rs:158-160:
```
        // Every refusal is classified BEFORE the clock moves, so a refused update
        // leaves nothing behind: not the list, and not the service clock either —
        // the ordering `DeleteThreatIntelSet` and `CreateThreatIntelSet` follow.
```
and in src/ops/create_trusted_entity_set.rs:203-204:
```
        // Everything that can refuse the request is decided over an IMMUTABLE
        // borrow, so a refused create neither mints an id nor advances the clock.
```
and in src/ops/update_filter.rs:192-194 ("Checked before the clock is read, so a refused update moves no timestamp and no version.", tick at :205), and src/ops/update_trusted_entity_set.rs:217-220 ("Past the refusals this is a MUTATION, so it advances this state's clock").

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

THE VIOLATIONS — src/ops/delete_ip_set.rs:143-170 ticks first, refuses after:
```
143        let mut state = state.lock();
144        // A MUTATION, so it advances this state's clock: the instant it hands out is
...
147        let now = state.tick_millis();
148
149        let Some(detector) = state.detector_in_mut(&region, &input.detector_id) else {
...
152            return Err(crate::scaffolded::Scaffolded::invalid_input(
153                crate::state::detector_id_rejection(&input.detector_id),
...
164        let Some(accepted) = detector.delete_ip_set(&input.ip_set_id, now) else {
...
167            return Err(crate::scaffolded::Scaffolded::invalid_input(
168                crate::state::ip_set_id_rejection(&input.ip_set_id),
```
src/ops/delete_trusted_entity_set.rs:146-180 — tick at :150, detector refusal at :152-158, id refusal at :168-180 (its own sibling update_trusted_entity_set.rs:220 ticks only "past the refusals").
src/ops/delete_threat_entity_set.rs:151 — same shape.
src/ops/delete_filter.rs:140-163 — tick at :143, detector refusal at :144-152, `filterName` refusal at :159-163.
src/ops/update_ip_set.rs:168-194 — tick at :171, detector refusal at :173-179, `ipSetId` refusal at :190-194.
src/ops/accept_invitation.rs:158-181 — self-contradicting:
```
158        let mut state = state.lock();
159        // A MUTATION, so the clock is read here: the instant is what the accepted
160        // relationship moves the detector's `UpdatedAt` to.
161        let now = state.tick_millis();
162        let Some(detector) = state.detector_in_mut(&region, &input.detector_id) else {
...
177        if !detector.accept_invitation(&input.master_id, &input.invitation_id, now) {
178            return Err(crate::scaffolded::Scaffolded::invalid_input(
179                crate::state::invitation_not_pending_message(),
```
src/ops/accept_administrator_invitation.rs:164-182 — tick at :166, refusals at :167-171 and :176-182.

TEST COVERAGE of the affected paths: tests/all/ip_set_update_and_delete.rs, tests/all/detector_sub_resource_deletes.rs, tests/all/filter_delete.rs, tests/all/trusted_entity_set_delete.rs, tests/all/threat_entity_set_delete.rs, tests/all/threat_intel_set_delete.rs, tests/all/member_invitation.rs. None of them asserts that a refused call advanced the clock, so none pins the current ordering.

## Steps to Reproduce

1. Create a detector in one Region (`POST /detector`) and an IPSet under it.
2. Issue a burst of `DELETE /detector/{detectorId}/ipset/{ipSetId}` calls naming an `ipSetId` the detector does not hold (each answers 400 `InvalidInputException` via `ip_set_id_rejection`). Each one reaches `state.tick_millis()` at delete_ip_set.rs:147 before the refusal at :167, so each advances `last_millis` by at least 1 ms.
3. Immediately create a new sub-resource (e.g. `POST /detector/{detectorId}/filter`) and read it back with `GetFilter`.
4. Observe `createdAt`/`updatedAt` sitting ahead of the real wall clock by roughly the number of refused calls made in the preceding millisecond — state the refused calls were supposed to leave untouched.
5. Contrast with the same sequence against `DELETE /detector/{detectorId}/threatintelset/{threatIntelSetId}`: delete_threat_intel_set.rs refuses at :150-169 before ticking at :174, so refused calls there consume no tick.

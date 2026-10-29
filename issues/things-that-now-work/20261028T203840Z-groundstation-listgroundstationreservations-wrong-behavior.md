Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6d79682d5fafb93b7e79af5f6250590ad910f660

# A contact stopped mid-pass keeps its antenna held forever, against the crate's own "a released reservation holds nothing" rule

_Filed: 2026-10-28T20:38:40Z_
_Operation: ListGroundStationReservations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6d79682d5fafb93b7e79af5f6250590ad910f660`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CancelContact` on a contact whose pass is under way (`PREPASS`, `PASS`, `POSTPASS`) calls `Contact::begin_cancellation`, which sets `antenna_time_spent = true` and moves the contact to `CANCELLING` and then (on the clock) to `CANCELLED`. But `Contact::holds_antenna_time()` returns `true` as soon as `antenna_time_spent` is set, *whatever* the status becomes — so the released reservation is still counted as holding its antenna for its whole window, forever.

Two places read that one predicate and both therefore contradict their own documented rule:

* `ComAmazonawsGroundstationGroundstationState::antennas_held` filters contacts on `holds_antenna_time()`, directly under a doc sentence that says "A released reservation holds nothing, so the antenna it was on is free again." Since `free_antenna` is built on `antennas_held`, the antenna a stopped contact was placed on stays blocked: `ReserveContact` refuses a new reservation over the same window at that ground station (and `available_contact_windows`, which also calls `free_antenna`, stops offering the window) even though the reservation was released.
* `ListGroundStationReservations` filters on `!contact.holds_antenna_time()` right under a comment that says the settle is done "so a reservation released by a cancel that has since settled is no longer reported as holding its antenna". The settle does move `CANCELLING` -> `CANCELLED`, but that changes nothing here, so the listing keeps reporting a `CONTACT` reservation for a contact the API itself reports as `CANCELLED`.

The conflation is that `holds_antenna_time()` answers a BILLING question ("did this contact hold or hold antenna time, so its minutes count" — correct for `GetMinuteUsage`, which the crate's own comment says is deliberate: "the antenna time the pass already used stays spent") and is reused for a PLACEMENT/REPORTING question ("is this antenna held right now"), where a spent-but-released reservation must answer no.

A contact stopped while in `PREPASS` makes this plainly visible: its `startTime`..`endTime` window is still entirely in the future, yet after a successful cancel the antenna is still held for it and the reservation is still listed.

## Evidence

The write that sets the flag — `src/state.rs:1767-1778` (`Contact::begin_cancellation`):
```
    pub fn begin_cancellation(&mut self, now: DateTime) {
        if STATUSES_CANCELLABLE_DURING_PASS.contains(&self.contact_status.as_str()) {
            self.antenna_time_spent = true;
            self.contact_status = CONTACT_CANCELLING.to_string();
```
with `src/state.rs:1566`:
```
pub const STATUSES_CANCELLABLE_DURING_PASS: &[&str] = &["PREPASS", "PASS", "POSTPASS"];
```

The predicate that then never goes back to false — `src/state.rs:1697-1704`:
```
    /// Whether this contact holds (or held) antenna time for the account, so its minutes
    /// count against the usage of the month it starts in: its status is not one of
    /// [`STATUSES_WITHOUT_ANTENNA_TIME`], or its pass already spent the antenna time
    /// ([`Contact::antenna_time_spent`]) and no later status move gives it back.
    pub fn holds_antenna_time(&self) -> bool {
        self.antenna_time_spent
            || !STATUSES_WITHOUT_ANTENNA_TIME.contains(&self.contact_status.as_str())
    }
```

The rule the placement side states and breaks — `src/state.rs:4963-4970` and `src/state.rs:4986-4990`:
```
    /// The antennas at `ground_station` that something already holds for any instant of
    /// `window` ... and the ones a contact of this account whose reservation still holds
    /// antenna time ([`Contact::holds_antenna_time`]) is placed on.
    ///
    /// A released reservation holds nothing, so the antenna it was on is free again.
```
```
        let reserved = self
            .contacts
            .values()
            .filter(|contact| contact.ground_station == ground_station)
            .filter(|contact| contact.holds_antenna_time())
```

The rule this listing states and breaks — `src/ops/list_ground_station_reservations.rs:246-248` and `:288-291`:
```
        // Apply every status move already due, so a reservation released by a cancel that
        // has since settled is no longer reported as holding its antenna.
        guard.settle_due_contacts();
```
```
                if contact.ground_station != input.ground_station_id
                    || !contact.holds_antenna_time()
                    || !crate::state::windows_overlap(held, window)
```

The cancel handler's own claim that the reservation IS released — `src/ops/cancel_contact.rs:188-191`:
```
            // Accepted: the reservation is released. Before the pass that is CANCELLED
            // outright; during it the contact is STOPPED — CANCELLING while the stop is
            // carried out, and the antenna time the pass already used stays spent.
            contact.begin_cancellation(crate::state::now_timestamp());
```

Test coverage: nothing pins the mid-pass case. `tests/all/ground_station_reservations.rs:586-631` (`cancelling_a_contact_releases_its_reservation_and_frees_the_antenna`) cancels a contact that is still before its pass (so `antenna_time_spent` is never set) and asserts "case after-cancelling: a released reservation holds no antenna time" plus that the antenna is "free again". The status table at `tests/all/ground_station_reservations.rs:543-558` writes `contact.contact_status` directly and resets only `scheduling_settles_at`/`cancellation_settles_at` — it never sets `antenna_time_spent`, so the `CANCELLED`-after-a-stop combination is untested.

## Steps to Reproduce

1. Create a mission profile with `minimumViableContactDurationSeconds` small and a pre-pass duration > 0, then `ReserveContact` a window at ground station `S` so the contact is placed on antenna 1.
2. Let the clock reach the contact's `prePassStartTime` (so a read settles it to `PREPASS`), i.e. reserve a window starting a few seconds ahead with a pre-pass widening, then `GET /contact/{contactId}` until `contactStatus` is `PREPASS`.
3. `DELETE /contact/{contactId}` (CancelContact) → 200; the contact reads back `CANCELLING` and then `CANCELLED`.
4. `POST /groundstation/{S}/reservations` (ListGroundStationReservations) over a window covering the contact → the released contact is STILL reported as a `CONTACT` reservation on antenna 1, contradicting the handler's own comment.
5. `ReserveContact` the same window at `S` again → refused with `InvalidParameterException` "every antenna at ground station S is already held for a window overlapping this one", although the only holder is the cancelled contact and `antennas_held`'s doc says "A released reservation holds nothing, so the antenna it was on is free again".

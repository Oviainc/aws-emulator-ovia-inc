Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7f16775ab75866f4d67c2e0f0f0192fd6ebf8c3d

# ReserveContact places a pass without settling the clock-owed contact moves, so a released reservation keeps blocking its antenna until some other call settles it

_Filed: 2026-10-29T01:47:11Z_
_Operation: ReserveContact | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7f16775ab75866f4d67c2e0f0f0192fd6ebf8c3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ReserveContact` decides which antenna a pass can be placed on by reading the CURRENT status of every contact at the ground station (`free_antenna` -> `antennas_held` -> `Contact::reserves_antenna`). It is the only writer or reader of contact status in this crate that does NOT call `settle_due_contacts()` first.

That matters for exactly one status: a contact stopped mid-pass is left in `CANCELLING`, which the crate deliberately counts as still holding its antenna, and only the clock-owed `CANCELLING -> CANCELLED` move releases it. `settle_due_contacts()` is what materialises that move. Because `ReserveContact` skips it, the stopped contact stays `CANCELLING` in the store indefinitely — until some *other* operation happens to settle it — and `free_antenna` keeps reporting its antenna as held long after the release fell due.

The result is an order-dependent refusal of a request the crate's own rules say must succeed: after `CancelContact` on a pass under way and a wait past `CANCELLATION_SETTLES_AFTER`, `ReserveContact` over the same window answers `InvalidParameterException` "every antenna at ground station X is already held for a window overlapping this one" — yet issuing any reporting call first (`DescribeContact`, `ListGroundStationReservations`, `ListContacts`), which settles the move, makes the identical `ReserveContact` succeed. Two back-to-back calls therefore disagree about whether the antenna is free, and the discovery-then-reserve loop can be handed an `AVAILABLE` window by `ListContacts` (whose availability search does settle) that `ReserveContact` would have refused had it been called first.

The fix is the one line every sibling already has: take the lock and `settle_due_contacts()` before reading contact status for placement.

## Evidence

The crate's rule that only the SETTLED release frees the antenna — `src/state.rs:1785-1789` (`Contact::begin_cancellation` doc):
```
    /// That pass already had the antenna on the account's behalf,
    /// so the stop also records the antenna time as spent: a release cannot give back
    /// time the pass has already used. Those spent minutes are a BILLING record only — the
    /// released reservation holds no antenna once the stop has settled
    /// ([`Contact::reserves_antenna`]).
```
and `src/state.rs:1680-1682`, which says `CANCELLING` still holds it:
```
/// other value is a contact whose reservation still holds its antenna — including
/// `CANCELLING`, whose release has not completed, and `FAILED`, whose antenna time was
/// still reserved.
```
with `src/state.rs:1693-1699`:
```
pub const STATUSES_WITHOUT_ANTENNA_TIME: &[&str] = &[
    "AVAILABLE",
    "AWS_CANCELLED",
    "AWS_FAILED",
    "CANCELLED",
    "FAILED_TO_SCHEDULE",
];
```
and `src/state.rs:1712-1714`:
```
    pub fn reserves_antenna(&self) -> bool {
        !STATUSES_WITHOUT_ANTENNA_TIME.contains(&self.contact_status.as_str())
    }
```

The helper that materialises the move, and its own statement that any path reading status must call it — `src/state.rs:5102-5108`:
```
    /// Materialise every `SCHEDULING` -> `SCHEDULED` move, every `CANCELLING` ->
    /// `CANCELLED` move, every `UPDATING` -> `ACTIVE` contact-version move and every step
    /// of the pass lifecycle ([`CONTACT_PASS_LIFECYCLE`]) the clock has already made due.
    /// A reporting operation calls this before reading: the moves were owed at this
    /// instant whether or not anybody called, so applying them changes nothing a later
    /// caller could otherwise have seen.
    pub fn settle_due_contacts(&mut self) {
```
which `antennas_held` depends on — `src/state.rs:5164-5168`:
```
        let reserved = self
            .contacts
            .values()
            .filter(|contact| contact.ground_station == ground_station)
            .filter(|contact| contact.reserves_antenna())
```

The handler that skips it — `src/ops/reserve_contact.rs:162` takes the lock with no settle:
```
        let mut guard = state.lock();
```
and then reads the stale statuses at `src/ops/reserve_contact.rs:272-290`:
```
        let Some(antenna_name) = guard.free_antenna(
            &input.ground_station,
            (
                input.start_time.as_secs_f64(),
                input.end_time.as_secs_f64(),
            ),
        ) else {
            return Err(ReserveContactErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some(format!(
                        "every antenna at ground station {} is already held for a window \
                         overlapping this one",
```

Every sibling that reads contact status settles first, under an explicit comment:
* `src/ops/cancel_contact.rs:146-150` — "Judge the contact by the state in force NOW: a move the clock already owed — a reservation's SCHEDULING -> SCHEDULED, an earlier stop's CANCELLING -> CANCELLED ... it is what decides whether there is still a reservation to release." / `guard.settle_due_contacts();`
* `src/ops/list_ground_station_reservations.rs:246-248` — "Apply every status move already due, so a reservation released by a cancel that has since settled is no longer reported as holding its antenna." / `guard.settle_due_contacts();`
* `src/ops/list_contacts.rs:288-290`, `src/ops/describe_contact.rs:179-183`, `src/ops/update_contact.rs:156-159`, `src/ops/delete_mission_profile.rs:152-157`, `src/ops/get_minute_usage.rs:191-195`, `src/ops/describe_contact_version.rs:214-217`, `src/ops/list_contact_versions.rs:188-191`, `src/ops/delete_ephemeris.rs:165-172` — all `guard.settle_due_contacts();`

Test coverage: nothing exercises `ReserveContact` as the FIRST call after a mid-pass cancel settles. `tests/all/ground_station_reservations.rs:796-822` sleeps past the settle span and then calls `DescribeContact` (which does settle) and `ListGroundStationReservations` before re-reserving:
```
    // Every stop the cancels started is due by now, so each contact has settled.
    std::thread::sleep(AFTER_SETTLE);

    for (status, station, contact_id) in &released {
        let described = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
...
        let successor = reserve_pass_at(&router, &profile_arn, station, status, window).await;
```
so the intervening settling read is what makes that assertion pass. `tests/all/contact_window_rules.rs:31-32` covers only "the release: a cancelled contact holds nothing" for a cancel BEFORE the pass, where `begin_cancellation` sets `CANCELLED` outright (`src/state.rs:1795-1798`) and no settle is owed.

## Steps to Reproduce

1. Create a mission profile with a non-zero `contactPrePassDurationSeconds` and a small `minimumViableContactDurationSeconds`.
2. `ReserveContact` a window at ground station `S` starting a few seconds in the future, so the contact is placed on antenna 1.
3. `GET /contact/{contactId}` until `contactStatus` reports `PREPASS` or `PASS` (the pass is under way).
4. `DELETE /contact/{contactId}` (CancelContact) -> 200; the contact reports `CANCELLING` and the service owes it the move to `CANCELLED`.
5. Sleep past `CANCELLATION_SETTLES_AFTER` (3 s), and make NO other call to the service.
6. `ReserveContact` the same window at `S` again -> refused with HTTP 431 `InvalidParameterException` "every antenna at ground station S is already held for a window overlapping this one", although the only holder is the contact whose release fell due in step 5.
7. Now `GET /contact/{contactId}` (which calls `settle_due_contacts`) -> `contactStatus` is `CANCELLED`, and repeating the identical `ReserveContact` from step 6 succeeds — two consecutive calls disagreeing about whether the antenna is free.

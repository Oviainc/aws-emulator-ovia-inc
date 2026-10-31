Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9a3acde976cfb747ba29bb78f67d8041a784763f

# Two different tie-breaks decide "which ephemeris is in force": the availability search picks the smallest ephemerisId where the satellite read picks the most recently created

_Filed: 2026-10-29T03:07:00Z_
_Operation: ListContacts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9a3acde976cfb747ba29bb78f67d8041a784763f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate answers the question "of several ENABLED ephemerides held for one satellite, which one is in force?" in two places, with two different rules, and the two can disagree for the same satellite in the same instant.

`crate::state::ephemeris_precedence` is the crate's named helper for exactly that question, and `refresh_current_ephemerides` (the path `GetSatellite`, `ListSatellites` and `DeleteEphemeris` take) uses it: highest `priority`, then — of equal priority — the most recently created, then the larger `ephemerisId`.

`pointing_passes`, the path `ListContacts`' AVAILABLE search takes through `available_contact_windows`, re-implements the ordering inline instead of calling that helper: it sorts by descending `priority` and then by ASCENDING `ephemerisId`, and never looks at `creation_time` at all. The ephemeris earliest in that order is the one whose passes are kept, so of two equal-priority ephemerides the one with the lexicographically SMALLEST id wins.

Both copies cite the same model sentence (`priority` — "Higher numbers take precedence") as their basis, so they are two readings of one rule. For a satellite holding two ENABLED `AZ_EL` ephemerides for the same ground station at the same `priority` whose segments overlap, `GetSatellite` reports `currentEphemeris.ephemerisId` = the later-created one, while `ListContacts(statusList=[AVAILABLE])` reports every offered window's `ephemeris.ephemerisId` = the smaller-id one — which is the older one about half the time, ids being random uuids. A caller is then told the satellite's trajectory is computed from one ephemeris and offered windows whose geometry was read from another, and the peak elevation (`maximumElevation`) reported for each offered window is the one that other ephemeris carries.

The satellite side is the one the crate pins with a stated reason in its tests; the availability side's tie-break is pinned by nothing.

## Evidence

The crate's named helper for the question, with its chosen tie-breaks — `src/state.rs:3345-3363`:
```
/// The ephemeris of two held for the same satellite that its trajectory is computed from:
/// [`::std::cmp::Ordering::Greater`] when `left` takes precedence over `right`.
///
/// The highest `priority` wins: "A priority score that determines which ephemeris to use
/// when multiple ephemerides overlap. Higher numbers take precedence."
/// (`smithy_query(query="doc", shape="CreateEphemerisRequest", member="priority")`).
///
/// The tie-breaks are CHOSEN, so the answer is total and stable whatever order the store
/// iterates in: of equal priority the most recently created wins (the later upload is the
/// newer trajectory), and of equal creation instant the larger `ephemerisId`.
fn ephemeris_precedence(left: &Ephemeris, right: &Ephemeris) -> ::std::cmp::Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| {
            left.creation_time
                .as_secs_f64()
                .total_cmp(&right.creation_time.as_secs_f64())
        })
        .then_with(|| left.ephemeris_id.cmp(&right.ephemeris_id))
}
```

The path that calls it — `src/state.rs:5008-5024` (`refresh_current_ephemerides`, reached from `src/ops/get_satellite.rs:182-183` and `src/ops/list_satellites.rs:165-166`):
```
            let takes_over = in_force.get(satellite_id).is_none_or(|held| {
                ephemeris_precedence(ephemeris, held) == ::std::cmp::Ordering::Greater
            });
```

The path that re-implements it differently — `src/state.rs:5256-5261` (doc) and `src/state.rs:5302-5308` (code, inside `pointing_passes`):
```
    /// Where two ephemerides describe the same instants, the HIGHER PRIORITY one is the one
    /// used — `priority` is "a priority score that determines which ephemeris to use when
    /// multiple ephemerides overlap. Higher numbers take precedence."
    /// (`smithy_query(query="shape", shape="EphemerisItem")`) — so a pass overlapping one
    /// already read from a higher-priority ephemeris is left out, and ties are broken by
    /// `ephemerisId` so the answer is the same on every call.
```
```
        in_force.sort_by(|first, second| {
            second
                .0
                .priority
                .cmp(&first.0.priority)
                .then_with(|| first.0.ephemeris_id.cmp(&second.0.ephemeris_id))
        });
```
(ascending `ephemeris_id`, so the SMALLEST id wins; `creation_time` is never read.)

The operation that reaches it — `src/ops/list_contacts.rs:447-456` calls `guard.available_contact_windows(...)` (`src/state.rs:5371-5389`, whose only pass source is `pointing_passes`), and `src/ops/list_contacts.rs:509-512` reports the winner to the caller:
```
                        ephemeris: Some(EphemerisResponseData {
                            ephemeris_id: Some(pass.ephemeris_id),
                            ephemeris_type: pass.ephemeris_type,
                        }),
```

Test coverage: the SATELLITE tie-break is pinned with its reason — `tests/all/satellite_current_ephemeris.rs:139-148`:
```
        Row {
            case: "two in force at the same priority",
            supplied: vec![
                Supplied::at_priority("stored first at four", 4),
                Supplied::at_priority("stored second at four", 4),
            ],
            // The documented priority leaves this pair tied, and the tie-break is the
            // CHOSEN one: the most recently created of equal priority.
            in_force: Some("stored second at four"),
        },
```
The availability-search test only ever uses UNEQUAL priorities, so nothing pins the diverging copy — `tests/all/available_contacts.rs:813-819`:
```
async fn where_two_ephemerides_overlap_the_higher_priority_one_is_the_one_used() {
    // The same pass described twice, with different peaks, so which ephemeris was used is
    // visible in the answer. Both orders of the two priorities, so what decides it is the
    // priority rather than the order they were stored in.
    for (first_priority, second_priority) in [(1_i64, 9_i64), (9, 1)] {
```

## Steps to Reproduce

1. Onboard one satellite (`GROUNDSTATION_SATELLITES`-style operator setting) that is served by ground station `Ohio-1`.
2. `POST /ephemeris` twice, each with `satelliteId` = that satellite, `priority: 4`, `enabled: true`, and an inline `azEl` body for `groundStation: "Ohio-1"` whose segments describe the SAME future pass with different peak elevations. Note the two returned `ephemerisId`s and which call was made second.
3. Wait for both to leave `VALIDATING` (`GET /ephemeris/{id}` reports `ENABLED`).
4. `GET /satellite/{satelliteId}` → `currentEphemeris.ephemerisId` is the ephemeris created SECOND (state.rs:3355 tie-break: most recently created).
5. `POST /contacts` with `statusList: ["AVAILABLE"]`, the satellite's ARN, `groundStation: "Ohio-1"`, a mission profile ARN and a window around the pass → each offered window's `ephemeris.ephemerisId` is whichever of the two ids sorts FIRST lexicographically (state.rs:5302 tie-break), and `maximumElevation` is that ephemeris's peak.
6. Whenever the second-created ephemeris has the larger id, the two reads name different ephemerides as the trajectory in force for the same satellite at the same instant.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6d79682d5fafb93b7e79af5f6250590ad910f660

# An empty `statusList` makes ListContacts select nothing, while the crate's two sibling listings treat an empty list filter as "narrows nothing"

_Filed: 2026-10-28T20:38:40Z_
_Operation: ListContacts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6d79682d5fafb93b7e79af5f6250590ad910f660`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListContacts$statusList` is a `StatusList` of `len 0..500`, so `[]` clears the boundary validator and reaches the handler. The handler then filters stored contacts with `input.status_list.contains(&contact.contact_status)`, which is `false` for every contact, and the `AVAILABLE` branch is likewise not entered — so an empty `statusList` answers an empty `contactList` and no `nextToken`, hiding every contact the account holds.

The crate has already decided, twice, what an empty list-valued filter means, and both times it means "narrows nothing":

* `ListEphemerides` — `statusList`, also modelled `len 0..500` — explicitly treats an empty or absent list as selecting every status, and says so in a comment plus a pinned test row "an empty status list narrows nothing".
* `ListGroundStationReservations` — `reservationTypes` — treats an empty list the same way (`types.is_empty() || ...`), with the comment "A request that names no type filters on none: every reservation type is reported."

`ListContacts` is the only listing in the crate that reads an empty list filter as "select nothing", so the same request shape means opposite things across sibling handlers of one service.

## Evidence

The handler that excludes everything — `src/ops/list_contacts.rs:326`:
```
            .filter(|contact| input.status_list.contains(&contact.contact_status))
```
(and the `AVAILABLE` branch at `src/ops/list_contacts.rs:397-401` is guarded by `input.status_list.iter().any(|status| status == crate::state::CONTACT_AVAILABLE)`, so an empty list computes no windows either.)

The sibling rule, stated in full — `src/ops/list_ephemerides.rs:259-267`:
```
            // An empty (or absent) status list selects every status: the member is
            // modelled `len 0..500` and documented as "the list of ephemeris status to
            // return", so naming none narrows nothing.
            .filter(|ephemeris| {
                input
                    .status_list
                    .as_ref()
                    .is_none_or(|wanted| wanted.is_empty() || wanted.contains(&ephemeris.status))
            })
```

The same rule again — `src/ops/list_ground_station_reservations.rs:234-239`:
```
        // A request that names no type filters on none: every reservation type is
        // reported. A value outside the modelled `ReservationType` enum never reaches
        // here — the model's own rules refuse it before this handler runs.
        let wanted = |reservation_type: &str| {
            types.is_empty() || types.iter().any(|named| named == reservation_type)
        };
```

The model confirming `[]` is valid input here — `smithy_query(query="operation", operation="ListContacts")`: `"statusList": {"target": "StatusList", "required": true, "length": {"min": 0, "max": 500}}`.

Test coverage: the sibling rule is pinned at `tests/all/ephemeris_catalog.rs:849-853`:
```
        (
            "an empty status list narrows nothing",
            json!({"statusList": []}),
            everything.clone(),
        ),
```
No test anywhere under `tests/all/` sends `ListContacts` an empty `statusList` (every call in `available_contacts.rs`, `scheduling_loop.rs`, `search_window_rule.rs`, `contact_window_rules.rs`, `page_boundaries.rs` names at least one status), so nothing pins the current behaviour.

## Steps to Reproduce

1. `ReserveContact` one contact, and read it back with `GET /contact/{contactId}` until `contactStatus` is `SCHEDULED`.
2. `POST /contacts` with `{"startTime": <before the contact>, "endTime": <after the contact>, "statusList": []}` → 200 with `contactList: []`.
3. `POST /ephemerides` (ListEphemerides) with `{"startTime": ..., "endTime": ..., "statusList": []}` → 200 listing EVERY ephemeris, and `POST /groundstation/{id}/reservations` with `reservationTypes: []` → 200 listing EVERY reservation type.
4. The same "empty list filter" input narrows nothing in two listings of this service and narrows everything away in the third.

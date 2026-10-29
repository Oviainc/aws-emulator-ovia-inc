Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 96b9d79b1b8395723cfce0b7aa8e9f6fed8726ec

# ReserveContact accepts a groundStation this account does not know and invents an antenna for it, while the crate's two sibling operations on that same name refuse it

_Filed: 2026-10-29T04:32:12Z_
_Operation: ReserveContact | Class: validation_

> _Note: this issue was identified against the code base at commit `96b9d79b1b8395723cfce0b7aa8e9f6fed8726ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states a single rule for "is this a ground station this account can address?" — `ComAmazonawsGroundstationGroundstationState::knows_ground_station`, which reads the account's own inventory plus the stations its satellite onboardings reach. Two handlers call it and refuse a name it rejects with `crate::state::ground_station_not_found_message` and `InvalidParameterException` (431): `ListAntennas` and `ListGroundStationReservations`.

`ReserveContact` — the only WRITE that takes a `groundStation` — never asks. It goes straight to `guard.free_antenna(&input.ground_station, ...)`, and `free_antenna` -> `placement_antennas` falls back to `antenna_name(1)` for a station the store holds nothing for. So a reservation at a name this account cannot address is accepted: a contact is stored holding antenna time at a non-existent ground station, on an antenna the emulator invented.

The two sides then contradict each other on the same stored state:
* `GET /groundstation/{thatName}/reservation` — the operation whose whole job is to report the antenna time held at a station — answers 431 `InvalidParameterException`, so the reservation this service just accepted can never be listed.
* `GET /groundstation/{thatName}/antenna` answers 431 for the very antenna the contact was placed on.

Nothing in `reserve_contact.rs` states a reason for skipping the check (its only comment about the station is about a station's antennas being FINITE), and the helper it needs already exists and is already called by two siblings. This is the same shape of gap the crate already closed for `ListGroundStationReservations` (fixed issue `...-groundstation-listgroundstationreservations-validation.md`): the write path was simply never brought under the rule.

## Evidence

The one rule, in `src/state.rs:4298-4302`:
```
    /// Whether this account knows a ground station called `name`. A name neither place
    /// records is a ground station this account cannot address.
    pub fn knows_ground_station(&self, name: &str) -> bool {
        self.ground_station_names().any(|station| station == name)
    }
```

Sibling #1 — `src/ops/list_antennas.rs:197-211`:
```
        // A `groundStationId` naming no ground station this account knows references
        // nothing, so the request is answered as the bad parameter it carries rather than
        // with the empty page the lookup would otherwise select — which a caller could not
        // tell from "this ground station has no antenna".
        if !guard.knows_ground_station(&input.ground_station_id) {
            return Err(ListAntennasErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some(crate::state::ground_station_not_found_message(
                        &input.ground_station_id,
                    )),
                    parameter_name: Some("groundStationId".to_string()),
                },
            )
            .into());
        }
```

Sibling #2 — `src/ops/list_ground_station_reservations.rs:258-277`:
```
        // A `groundStationId` naming no ground station this account knows references
        // nothing, so the request is answered as the bad parameter it carries rather than
        // with the empty page the lookup would otherwise select — which a caller could not
        // tell from "this ground station holds no antenna time". The same rule the sibling
        // listing bound under this id reads, under the same text
        // (`crate::state::ground_station_not_found_message`), and refused before anything
        // is written.
        if !guard.knows_ground_station(&input.ground_station_id) {
            return Err(
                ListGroundStationReservationsErrorModeled::InvalidParameterException(
```

The write that skips it — `src/ops/reserve_contact.rs:262-290` (the ONLY use it makes of `input.ground_station` before storing):
```
        // The antenna this pass is placed on. A reservation holds an antenna for its
        // window and an antenna serves one pass at a time, so the scheduler places the
        // contact on the first antenna of the ground station nothing else holds for that
        // window ...
        let Some(antenna_name) = guard.free_antenna(
            &input.ground_station,
            (
                input.start_time.as_secs_f64(),
                input.end_time.as_secs_f64(),
            ),
        ) else {
```
(no `knows_ground_station` anywhere in the file; `grep -n knows_ground_station src/ops` matches only `list_antennas.rs:201` and `list_ground_station_reservations.rs:265`.)

Why that silently succeeds — `src/state.rs:5223-5230`:
```
    pub fn placement_antennas(&self, ground_station: &str) -> Vec<String> {
        let recorded = self.antennas_at(ground_station);
        if recorded.is_empty() {
            vec![antenna_name(1)]
        } else {
            recorded
        }
    }
```
and `src/state.rs:4329-4338`, which says itself that the lookup cannot tell the two cases apart:
```
    /// The antennas at the ground station `name` ... empty when
    /// that ground station has none and when this account knows no station by that name.
    pub fn antennas_at(&self, name: &str) -> Vec<String> {
        self.ground_stations
            .get(name)
            .map(|station| station.antennas.clone())
            .unwrap_or_default()
    }
```

Test coverage: the refusal is pinned for the two listings — `tests/all/ground_station_reservations.rs:1145` `the_listing_refuses_a_ground_station_this_account_does_not_know`, and `tests/all/antenna_inventory.rs:342-362`. No test asks `ReserveContact` about an unknown station deliberately, but `tests/all/scheduling_loop.rs:1326-1343` (`reserve_contact_enforces_the_window_the_mission_profile_requires`) uses `harness()`, which seeds NO ground-station inventory and NO satellite onboarding (`scheduling_loop.rs:128-157`), and still expects 200 for `"groundStation": "Ohio-1"` — so it incidentally depends on the missing check and must seed the station once the check is added. `tests/all/contact_window_rules.rs` already seeds the station through `stock_inventory` and is unaffected.

## Steps to Reproduce

1. Start the emulator with no ground-station inventory and no satellite onboardings (the crate's own `harness()` state: `ground_stations` empty, `satellites` empty).
2. `POST /config` + `POST /missionprofile` to get a mission profile ARN (minimumViableContactDurationSeconds = 180).
3. `POST /contact` with `{"missionProfileArn": <arn>, "groundStation": "Nowhere-1", "startTime": <future>, "endTime": <future+600>}` → 200 with a `contactId`; the contact is stored holding `Antenna 1` at `Nowhere-1`.
4. `GET /groundstation/Nowhere-1/reservation?startTime=...&endTime=...` → 431 `InvalidParameterException`, message `ground_station_not_found_message("Nowhere-1")` — the reservation just created cannot be reported.
5. `GET /groundstation/Nowhere-1/antenna` → 431 for the antenna the contact was placed on.
6. Compare with `GET /contact/{contactId}` → 200 reporting `groundStation = "Nowhere-1"`: the service holds a reservation at a ground station it tells the caller the account cannot address.

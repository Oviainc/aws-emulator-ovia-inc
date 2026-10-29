Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b45fe0b1030ed2b932d59ccf686ccc68d031b528

# A `{groundStationId}` naming no ground station this account knows is answered with an empty reservation page, while the sibling listing on the same path refuses it

_Filed: 2026-10-28T15:23:56Z_
_Operation: ListGroundStationReservations | Class: validation_

> _Note: this issue was identified against the code base at commit `b45fe0b1030ed2b932d59ccf686ccc68d031b528`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListGroundStationReservations` takes the same required `{groundStationId}` `@httpLabel` as `ListAntennas` (`GET /groundstation/{groundStationId}/reservation` vs `GET /groundstation/{groundStationId}/antenna`), and both model exactly the same two client errors (`DependencyException`, `InvalidParameterException`).

`ListAntennas` resolves that label against the account's ground-station inventory and refuses a name nothing records, and its comment states the rule and the alternative it rejects. `ListEphemerides` and `ListGroundStations` apply the same rule to their `satelliteId` filter. `ListGroundStationReservations` applies no such gate: the handler takes the lock, settles due contacts and then filters maintenance windows and contacts by `maintenance.ground_station != input.ground_station_id` / `contact.ground_station != input.ground_station_id`, so a station this account was never told about resolves to the empty page the lookup selects — the very answer the sibling handler's comment says a caller cannot tell from "this ground station holds nothing".

## Evidence

Check present in the sibling, with its rationale — `services/service-com-amazonaws-groundstation-groundstation/src/ops/list_antennas.rs:202-214`:
"// A `groundStationId` naming no ground station this account knows references
 // nothing, so the request is answered as the bad parameter it carries rather than
 // with the empty page the lookup would otherwise select — which a caller could not
 // tell from \"this ground station has no antenna\".
 if !guard.knows_ground_station(&input.ground_station_id) { return Err(... InvalidParameterException ...) }"

Same rule on the other resolvable filters: `src/ops/list_ephemerides.rs:210-223` ("A satellite filter names a satellite of this account; one this account is not on-boarded to references nothing, so the request is answered as the missing resource it names rather than with the empty page it would otherwise select") and `src/ops/list_ground_stations.rs:201-218`.

The helper is crate-wide and cheap: `src/state.rs:4051-4055` `pub fn knows_ground_station(&self, name: &str) -> bool` — "Whether this account knows a ground station called `name`. A name neither place records is a ground station this account cannot address."

No such call in this handler: `src/ops/list_ground_station_reservations.rs:243-307` goes `let mut guard = state.lock(); guard.settle_due_contacts();` straight into `if maintenance.ground_station != input.ground_station_id` (line 256) and `if contact.ground_station != input.ground_station_id` (line 286); `grep -n 'knows_ground_station' src/` matches only `list_antennas.rs` and `state.rs`.

Test note for the fixer: `tests/all/ground_station_reservations.rs:443-461` currently exercises this path with `route("Hawaii-1", WINDOW, &[])` while the suite's inventory is only `Ohio-1`/`Oregon-1` (`inventory_setting`, lines 206-213, seeded at line 349), asserting 200 + `[]` under the stated reason "a ground station holding nothing must list nothing". That reason is about a KNOWN station that holds nothing, so the case should name a station the inventory records (or assert the 431 the sibling gives) once the gate lands.

## Steps to Reproduce

1. Start the emulator with `GROUNDSTATION_GROUND_STATIONS=Ohio-1=Antenna 1` (so `Hawaii-1` is a station this account was never told about).
2. GET /groundstation/Hawaii-1/antenna?startTime=...: refused 431 InvalidParameterException naming `groundStationId` (ListAntennas).
3. GET /groundstation/Hawaii-1/reservation?startTime=<t>&endTime=<t+3600>: answers 200 with `reservationList: []` and no token — indistinguishable from a real ground station that holds no antenna time, even though the account cannot address that station at all.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7432a15a84544829253538d508958ad90c210154

# ReserveContact accepts windows AWS cannot reserve: a window entirely in the past and a window overlapping an existing reservation at the same ground station

_Filed: 2026-10-23T23:53:16Z_
_Operation: ReserveContact | Class: validation_

> _Note: this issue was identified against the code base at commit `7432a15a84544829253538d508958ad90c210154`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler validates only that `endTime > startTime` and that the window is at least the mission profile's `minimumViableContactDurationSeconds`. Two classes of request that real AWS refuses are accepted with 200 and committed to state:

(a) A window entirely in the past. Antenna time that has already elapsed cannot be reserved — AWS refuses such a reservation with `InvalidParameterException` (HTTP 431, modelled on this operation) naming the time member. The emulator mints a contact id and ARN, stores the contact in `SCHEDULING`, and then settles it to `SCHEDULED` 300 ms later, so a caller holds a reservation for a pass that is over. This also pollutes GetMinuteUsage, which counts the elapsed window's minutes as scheduled antenna time for its month.

(b) A window overlapping a contact already reserved at the SAME ground station. A ground-station antenna serves one contact at a time, which is why contacts are reserved out of `AVAILABLE` passes: once a window is taken, the same slot is no longer reservable and AWS refuses the second reservation. The emulator keeps an unbounded set of contacts with no overlap check at all, so N identical reservations on one ground station all succeed and all count their minutes in GetMinuteUsage.

Expected: both requests are refused with the operation's modelled `InvalidParameterException` (431) and nothing is written. Actual: HTTP 200 with a `contactId`/`versionId`, and the contact is visible to DescribeContact/ListContacts/GetMinuteUsage.

## Evidence

Spec: `smithy_query(query="operation", operation="ReserveContact")` — input `startTime`/`endTime` (required timestamps), `groundStation` (required), modelled errors `InvalidParameterException` (431), `ResourceNotFoundException` (434), `ResourceLimitExceededException` (429). Well-established Ground Station semantics: a contact is a reservation of a specific antenna window selected from the AVAILABLE passes of a satellite over a ground station, so a window in the past, or one already held by another contact at that ground station, is not reservable.
Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/reserve_contact.rs` lines ~169-210 — the only checks are `duration_seconds <= 0.0`, `duration_seconds < minimum`, and the emulator store bound; no comparison against `now` and no scan of `guard.contacts` for an overlapping window at the same `ground_station`.
Tests: `tests/all/scheduling_loop.rs` reserves only future windows (`start = 1_800_000_000`) and never two overlapping windows on one ground station (rows use Ohio-1 at offset 0 and 86_400), and the store-bound suite bulk-creates configs/mission profiles rather than contacts — so neither rule is asserted anywhere and adding them breaks no existing assertion.

## Steps to Reproduce

1. CreateConfig a tracking config and CreateMissionProfile with minimumViableContactDurationSeconds=60.
2. ReserveContact with startTime = now - 7200 and endTime = now - 3600 (a window entirely in the past): the emulator answers 200 with a contactId where AWS refuses with InvalidParameterException; DescribeContact then reports it as SCHEDULED and GetMinuteUsage counts its minutes.
3. ReserveContact with groundStation="Ohio-1", startTime=T, endTime=T+600 (T in the future) — 200, as expected.
4. ReserveContact again with the SAME groundStation and an overlapping window (startTime=T+60, endTime=T+660): the emulator answers 200 and stores a second contact on the same antenna window, where AWS refuses because that slot is no longer available.

Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# An AVAILABLE search's page offsets address a sequence the handler itself recomputes per page, so reserving (or the clock passing) a window makes the next page skip one

_Filed: 2026-10-28T11:51:31Z_
_Operation: ListContacts | Class: pagination_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListContacts` pages an AVAILABLE search by positional offset: `paginate(&scope, &items, max_results, offset)` cuts the combined sequence of stored contacts followed by the windows `available_contact_windows` computes, and the `nextToken` carries only that offset. Both the handler and `crate::state` assert in comments that this is safe because the computed sequence is the same on every call.

It is not. `available_contact_windows` is recomputed from scratch on every page and filters each candidate pass on `end > now` (with `now` read from the clock at the top of this very call) and on `free_antenna(...)` (read from the live contact store). So a window that drops out of the sequence between page N and page N+1 shifts every later window one position earlier, and the offset in the token then lands past it: the caller never sees that window.

Two ordinary things drop a window:
- the caller reserves one. That is exactly the "discovery-then-reserve loop" the handler says it exists to serve, and `crate::state` states outright that "a window already reserved stops being offered".
- the clock passes a window's `end` while the caller is paging.

The defect is the contradiction between the stated rule (the offset means the same thing on every call) and the code under it (the sequence is recomputed from `now` and from the mutable contact store), with a concrete lost window as the consequence.

## Evidence

The rule the handler states, services/service-com-amazonaws-groundstation-groundstation/src/ops/list_contacts.rs:367-370:
```
                // The sequence of windows a search resolves is the same for every page of
                // it (`available_contact_windows` is bounded by constants, not by what the
                // request asks for), so the offset a token carries addresses the same
                // window on every call.
```
and again at list_contacts.rs:263-266:
```
        // The contacts the store holds, ordered by contact id: a page boundary is only
        // stable over a stable order. The windows an availability search computes are
        // appended after them, in start-time order, so the whole sequence a token offsets
        // into is stable too.
```
and in the shared constant's doc, services/service-com-amazonaws-groundstation-groundstation/src/state.rs:3384-3387:
```
/// It is a CONSTANT rather than something the request names, so every page of one search
/// resolves the SAME sequence of windows and an offset into it means the same thing on
/// every call.
```

The code under it, list_contacts.rs:371-380 — `now` is read per call and handed to the filter:
```
                let offered = guard.available_contact_windows(
                    satellite_arn,
                    ground_station,
                    &profile,
                    (window_start, window_end),
                    crate::state::now_timestamp().as_secs_f64(),
```
and the filter itself, state.rs:5025-5030:
```
        self.pointing_passes(satellite_arn, ground_station, |start, end| {
            windows_overlap((start, end), window)
                && end > now
                && end - start >= minimum
                && self.free_antenna(ground_station, (start, end)).is_some()
        })
```
whose own doc states the sequence changes, state.rs:5004-5006:
```
    /// Those are exactly the conditions `ReserveContact` refuses a reservation on, so a
    /// window this answers is a window that can then be reserved, and a window already
    /// reserved stops being offered.
```
The token is a bare offset into that sequence, state.rs:1052-1059 (`encode_page_token` = `"{scope}.{offset}"`, `decode_page_token` parses the offset) and state.rs:1088-1092 (`let start = offset.min(items.len()); ... let next = (end < items.len()).then(|| encode_page_token(scope, end));`).

Test coverage: the only paging test for availability, services/service-com-amazonaws-groundstation-groundstation/tests/all/available_contacts.rs:1043 `an_availability_search_pages_its_offered_windows_and_bounds_what_it_computes`, builds every window in the future (`let base = now_seconds() + 3_600;`, days 0..5) and reserves nothing while paging, so it never exercises a window leaving the sequence mid-walk.

## Steps to Reproduce

1. Start the emulator with an on-boarded satellite reaching a ground station, and a mission profile whose `minimumViableContactDurationSeconds` the offered passes clear.
2. CreateEphemeris with az/el pointing data for that ground station describing at least four future passes, `enabled: true`; wait for validation to settle.
3. ListContacts with `statusList: ["AVAILABLE"]`, the satellite ARN, the ground station, the mission profile ARN, a window covering all the passes, and `maxResults: 2`. Record the two windows and the returned `nextToken`.
4. ReserveContact over the FIRST window of page 1 (the discovery-then-reserve loop the handler documents).
5. ListContacts again with the same filters and the `nextToken` from step 3.
6. Page 2 begins at offset 2 of a sequence that is now one window shorter (the reserved window is no longer offered — state.rs:5029 `free_antenna`), so the third offered window is never returned by any page of the walk. The same skip occurs with no reservation at all if the clock passes the end of an offered window between the two calls (state.rs:5027 `end > now`).

Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# A nextToken is invalidated when the monitor it names leaves the result set, refusing the resume with 400

_Filed: 2026-10-24T09:06:18Z_
_Operation: ListMonitors | Class: pagination_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a pagination cursor vended by ListMonitors stays usable for the follow-up call. AWS resumes the listing at the position the token encodes (the next key at or after the cursor) even when the underlying set has shifted between calls — a monitor deleted at the page boundary, or one whose state changed, drops out of the results but the caller still receives the remaining monitors with HTTP 200. A vended token is never answered with a 400.

Actual (emulator): the cursor is the hex-encoded monitorName of the first item of the next page, and the resume resolves it by an EXACT position lookup inside the freshly recomputed, freshly filtered result list (`matching.iter().position(|m| m.monitor_name == key)`). When that exact monitor is no longer a member of the recomputed set the lookup yields None and the whole call is refused with ValidationException (HTTP 400, message "nextToken does not name a page of this result set"), losing every remaining page. Two reachable ways to hit it:

1. Page 1 of a listing, then DeleteMonitor on the monitor the returned nextToken names, then page 2 with that token -> 400 instead of the remaining monitors.
2. Page 1 of a FILTERED listing (e.g. state=PENDING) where the boundary monitor settles PENDING -> ACTIVE before page 2 is requested (this emulator flips a monitor to ACTIVE ~500ms after its first probe, so the window is tiny and the race is routine) -> the boundary monitor no longer matches the filter, so page 2 is refused with 400.

A resume should fall forward to the first monitor whose name is >= the cursor key rather than demanding the cursor's own monitor still be present.

## Evidence

Spec: ListMonitors is @paginated(inputToken=nextToken, outputToken=nextToken, items=monitors) with ValidationException (HTTP 400) as its only input-refusal error; the token is modeled as an opaque string (len 0..4096), so a caller cannot repair or recompute it. Well-established AWS pagination semantics: a token the service vended is accepted on the next call and resumes the listing; items deleted or mutated in the meantime simply do not appear, they do not turn the token into a bad request. Handler: `src/ops/list_monitors.rs` — the filter builds `matching` at lines ~188-197, and the cursor resolution at lines ~199-215 returns `validation("nextToken does not name a page of this result set")` whenever `position()` on `matching` is None; `src/state.rs::encode_page_token`/`decode_page_token` show the cursor is just the hex of a monitorName. Tests: `tests/all/monitor_and_probe.rs::list_monitors_pages_the_stored_monitors` pages a static set (nothing is deleted or re-stated between pages) and only asserts that a cursor the service never vended is refused; `list_monitors_filters_on_state` never paginates a filtered listing. No test covers a resume across a mutation, so this is uncovered. (The separately filed open issue 20261024T052627Z-networkmonitor-listmonitors-error-response covers only the WORDING of these refusals, not the fact that a valid vended token is refused at all.)

## Steps to Reproduce

1. CreateMonitor for monitorName m1, m2, m3 (no probes, so all three are stored INACTIVE).
2. ListMonitors?maxResults=1 -> 200 with monitors=[m1] and a nextToken (it hex-encodes "m2").
3. DeleteMonitor(monitorName=m2) -> 200.
4. ListMonitors?maxResults=1&nextToken=<token from step 2>: observe HTTP 400 ValidationException "nextToken does not name a page of this result set"; expected HTTP 200 with the rest of the listing (m3).
5. Filter variant: CreateMonitor m4 with one inline probe (stored PENDING) plus another PENDING monitor; ListMonitors?state=PENDING&maxResults=1 -> 200 plus a nextToken naming the second PENDING monitor; wait ~1s so that monitor settles to ACTIVE; replay the token with state=PENDING: observe HTTP 400 instead of a 200 page.

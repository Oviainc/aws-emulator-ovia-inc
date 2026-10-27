Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# The "window must run forwards" refusal is hand-rolled in all three window-filtered listings

_Filed: 2026-10-26T06:53:03Z_
_Operation: ListContacts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same rule — a listing whose `endTime` precedes its `startTime` describes no interval, so it is refused as `InvalidParameterException` naming `endTime` with the message "endTime must not precede startTime" — is written out three times, once in each window-filtered listing, instead of once beside the other shared listing helpers in `state.rs` (`page_scope`, `decode_page_token`, `paginate`, `windows_overlap`). `ListGroundStationReservations` even records that the copies are meant to be identical ("Same rule, same wording as `ListContacts`"), which is exactly the invariant three independent copies cannot keep: a change to the comparison (e.g. to refuse an equal pair, or to name a different member) has to be made in three places. A single `fn inverted_window(start, end) -> Option<InvalidParameterException>` in `state.rs`, mapped into each operation's own error enum the way `validate_tags` and `validate_config_data` already are, leaves every response byte-identical.

## Evidence

Copy 1 — `src/ops/list_contacts.rs:185-196`:
```
        // The window a listing searches runs forwards: a request whose `endTime`
        // precedes its `startTime` describes no interval at all, so it is refused rather
        // than answered with the empty page the overlap test would produce.
        if input.end_time.as_secs_f64() < input.start_time.as_secs_f64() {
            return Err(ListContactsErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some("endTime must not precede startTime".to_string()),
                    parameter_name: Some("endTime".to_string()),
```
Copy 2 — `src/ops/list_ephemerides.rs:167-178`:
```
        if input.end_time.as_secs_f64() < input.start_time.as_secs_f64() {
            return Err(ListEphemeridesErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some("endTime must not precede startTime".to_string()),
                    parameter_name: Some("endTime".to_string()),
```
Copy 3 — `src/ops/list_ground_station_reservations.rs:193-204`, which names the duplication outright:
```
        // The window a listing searches runs forwards: ... Same rule, same
        // wording as `ListContacts`, which searches a window the same way.
        if input.end_time.as_secs_f64() < input.start_time.as_secs_f64() {
            return Err(
                ListGroundStationReservationsErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some("endTime must not precede startTime".to_string()),
```
The crate already single-homes its other shared listing rules in `state.rs` (`pub fn windows_overlap` at `state.rs:2202`, `pub fn paginate` at `state.rs:818`), with `windows_overlap`'s doc noting "the two call sites ... read one rule, and a half-open reading would have to replace it in both rather than in one" — the argument that applies here too.

Existing tests that pin all three refusals unchanged: `tests/all/scheduling_loop.rs:1551-1560` (`"window-runs-backwards"`, expecting 431 naming `endTime`, for `ListContacts`), `tests/all/ephemeris_catalog.rs:944-962` ("case backwards-window: expected 431", for `ListEphemerides`), and `tests/all/ground_station_reservations.rs:883-892` (`a_window_that_runs_backwards_is_refused`, rows `backwards-by-a-second` / `backwards-by-an-hour`, 431).

## Steps to Reproduce

1. Open `src/ops/list_contacts.rs:185`, `src/ops/list_ephemerides.rs:167` and `src/ops/list_ground_station_reservations.rs:193` side by side: the same comparison, the same message literal and the same `parameter_name` appear three times.
2. Note `src/ops/list_ground_station_reservations.rs:195-196` asserting the copies must stay identical ("Same rule, same wording as `ListContacts`") with nothing enforcing it.
3. Compare with the shared helpers the same handlers call for their other common rules (`crate::state::page_scope`, `crate::state::paginate`, `crate::state::windows_overlap`), which are defined once in `src/state.rs`.

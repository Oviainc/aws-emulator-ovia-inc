Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3ef01111206c6bd53a092ac9201ab1d4cb843bf0

# A retried identical UpdateFeed restarts the transient UPDATING span instead of having the effect of one request

_Filed: 2026-10-28T15:11:43Z_
_Operation: UpdateFeed | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3ef01111206c6bd53a092ac9201ab1d4cb843bf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateFeed` is `@idempotent`, and this crate states everywhere what that means for it: "several identical requests have the effect of ONE" — the request an SDK repeats when it never saw the first response. Every other `@idempotent` operation of this crate implements that: `CreateFeed`/`CreateDictionary` recognise a replay (`feed_created_from`/`dictionary_created_from`), `AssociateFeed` appends nothing for an output already in place (`outputs_to_append`), `DisassociateFeed` replays the served release (`has_served_release_of`), and — the closest analogue — `DeleteFeed` pins the due instant of its transient status so that "polling DeleteFeed cannot postpone it".

`UpdateFeed` is the one `@idempotent` operation whose repeat does NOT have the effect of one call. The handler unconditionally calls `feed.begin_reconfiguration(now)`, which re-enters `UPDATING` and sets `transition_due_at = now + TRANSITION_SPAN`, and `begin_reconfiguration`'s own doc says it "restarts the span". So a bit-identical replay — one that asks for the payload the feed already stores, so there is nothing left to reconfigure — pushes the feed back into the transient `UPDATING` status and postpones its return to the settled status (`AVAILABLE`/`ACTIVE`/`ARCHIVED`) by another full span. N identical retries produce N windows instead of one, and a client that keeps retrying (as an SDK does for an `@idempotent` operation whose response was lost) keeps the feed in `UPDATING`, where `AssociateFeed` refuses it with `FEED_NOT_ASSOCIABLE_MESSAGE` — so a lost response makes the feed unassociable for longer than the call that actually changed it did.

The defect is the missing replay recognition, not the length of the span: the same `(status, transition_due_at)` pinning `DeleteFeed` already performs is the shape the crate uses for exactly this situation.

## Evidence

The trait: `OPERATIONS.md:130-131` — "## UpdateFeed" / "- **Route:** `PUT /v1/feed/{id} -> 200`" / "- **Traits:** @idempotent".

What this crate says `@idempotent` means, `src/ops/create_feed.rs:346-352`: "The operation is `@idempotent` and carries no `@idempotencyToken`, so the request's own members are its key: a request this Region has already served is that same create once more — what an SDK retries when it never saw the first response — and several identical requests have the effect of ONE."

The sibling that pins its transient due instant, `src/ops/delete_feed.rs:195-202`:
```
// 202: the deletion is accepted, not completed. The feed reports DELETING
// until the clock makes the deletion due. Repeating the call inside that window
// is idempotent: the deletion stays due at the instant the first call set, so
// polling DeleteFeed cannot postpone it.
let due = match (feed.status, feed.transition_due_at) {
    (crate::state::FeedStatus::Deleting, Some(due)) => due,
    _ => feed.enter_transient(crate::state::FeedStatus::Deleting, now),
};
```

The path that skips it, `src/ops/update_feed.rs:393`: `let due = feed.begin_reconfiguration(now);` — with no comparison against the stored payload and no reuse of an in-flight due instant, over `src/state.rs:721-731`:
```
/// A reconfiguration that arrives while another is still in flight restarts the
/// span and keeps the SETTLED status already remembered: ...
pub fn begin_reconfiguration(&mut self, now: Instant) -> Instant {
    let settled = self.settles_into.unwrap_or(self.status);
    let due = self.enter_transient(FeedStatus::Updating, now);
    self.settles_into = Some(settled);
    due
}
```
and `src/state.rs:705-712`, which is what moves the clock on every call: `let due = now + TRANSITION_SPAN; self.status = status; self.transition_due_at = Some(due);`.

The consequence while the window is open, `src/ops/associate_feed.rs:328-335`: the `_` arm of the status match refuses an association with `crate::state::FEED_NOT_ASSOCIABLE_MESSAGE`.

Test coverage: no test exercises an UpdateFeed replay. `tests/all/feed_reconfiguration_status.rs:328-376` (`a_second_update_in_flight_still_settles_back_into_the_suspended_status`) sends two DIFFERENT payloads (`"first-replacement"` and `"second-replacement"`, lines 335-345) and only asserts the settled status afterwards, so it pins nothing about an identical repeat or the due instant; `tests/all/create_replay_is_idempotent.rs` covers the creates only, and `tests/all/feed_lifecycle.rs:651` ("Repeating DeleteFeed inside the DELETING window is idempotent: it re-reports …") covers the delete.

## Steps to Reproduce

1. POST /v1/feed with {"name":"replay-update","outputs":[{"name":"crop1","outputConfig":{"cropping":{...}},"status":"ENABLED"}]} and note the returned id F; GetFeed reports AVAILABLE
2. PUT /v1/feed/F with a payload P (name + outputs) — observe HTTP 200 with status UPDATING; the feed's transition is now due at t0 + TRANSITION_SPAN
3. Resend the BIT-IDENTICAL PUT /v1/feed/F with the same payload P before the span elapses — what an SDK retries when it never saw the first response — observe HTTP 200 with status UPDATING again, and the transition now due at t1 + TRANSITION_SPAN (begin_reconfiguration restarted the span) although the stored payload already equals P and there is nothing left to reconfigure
4. Keep resending the identical PUT just inside each span: GET /v1/feed/F keeps reporting UPDATING, and POST /v1/feed/F/associate keeps being refused 400 with FEED_NOT_ASSOCIABLE_MESSAGE
5. Contrast with the sibling: DELETE /v1/feed/F twice inside the DELETING window — the second call re-reports the first call's due instant (delete_feed.rs:199-202), so retrying cannot postpone the transition
6. Expected: as for every other @idempotent operation of this crate, the identical repeat has the effect of ONE request — the feed's transition stays due at the instant the first call set rather than being pushed back by each retry

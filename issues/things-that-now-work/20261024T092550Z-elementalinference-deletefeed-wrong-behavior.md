Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 19809ff85dacb8a5011e45ecfee3e76273a4f2a9

# The modeled DELETED status is unreachable: a deleted feed vanishes from the store, so a repeated DeleteFeed answers 404 instead of reporting DELETED

_Filed: 2026-10-24T09:25:50Z_
_Operation: DeleteFeed | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `19809ff85dacb8a5011e45ecfee3e76273a4f2a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteFeedResponse$status` is documented "The current status of the feed. When deletion of the feed has succeeded, the status will be DELETED", and `DELETED` is one of the values `FeedStatus` declares. The emulator can never produce it: `DeleteFeed` parks the feed in `DELETING` with a 400 ms transition due, and the settle pass (`settle_due_transitions`) then REMOVES the record outright. From that moment on the feed is simply gone.

Observable consequences:
* `DeleteFeed` is modeled `@idempotent`, so an SDK retry (or a client polling the delete to completion, which is exactly what the 202 invites) repeats the call. Inside the 400 ms window it correctly answers 202/DELETING; after it, the same call answers HTTP 404 `ResourceNotFoundException` instead of the documented success carrying `status: DELETED`.
* No request of this service ever reports `FeedStatus.DELETED`, so a client state machine that waits for `DELETED` (via DeleteFeed or a follow-up GetFeed) never sees it and instead has to treat a 404 as the terminal state.

Expected: once deletion has succeeded the service reports the feed as `DELETED` (the delete answers 202/200 with `status: DELETED` rather than refusing). Actual: the record is dropped and every later call about it is a 404.

## Evidence

Spec: `smithy_query(query="doc",shape="DeleteFeedResponse",member="status")` -> "When deletion of the feed has succeeded, the status will be DELETED"; `FeedStatus` enum includes `DELETED`; DeleteFeed carries `@idempotent` (OPERATIONS.md, "DeleteFeed").
Handler: `src/ops/delete_feed.rs` `handle()` sets `feed.enter_transient(FeedStatus::Deleting, now)` and never assigns a DELETED status; `src/state.rs` `settle_due_transitions()` removes the feed from `state.feeds` when the due instant passes, and `FeedStatus` in `src/state.rs` declares no `Deleted` variant at all ("the FeedStatus values this slice of the service produces").
Tests: `tests/all/feed_lifecycle.rs` asserts the DELETING window and then a 404 from GetFeed/DeleteFeed after `past_the_transition_span()`, i.e. it asserts the current behavior; no test exercises a DELETED report.

## Steps to Reproduce

1. POST /v1/feed with one output -> note the feed id
2. DELETE /v1/feed/{id} -> HTTP 202 with status DELETING
3. Wait past the emulator's transition span (~400 ms)
4. DELETE /v1/feed/{id} again (an idempotent retry of the same accepted delete) -> observe HTTP 404 ResourceNotFoundException instead of a success carrying status DELETED
5. GET /v1/feed/{id} -> 404; the modeled FeedStatus value DELETED is never observable from any operation

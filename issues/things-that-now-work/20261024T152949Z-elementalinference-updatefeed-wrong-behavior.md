Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# The modeled UPDATING feed status is unreachable: an output-list overwrite never reports UPDATING

_Filed: 2026-10-24T15:29:49Z_
_Operation: UpdateFeed | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`FeedStatus` models seven values (`CREATING | AVAILABLE | ACTIVE | UPDATING | DELETING | DELETED | ARCHIVED`). The emulator's internal `FeedStatus` carries only four (`Available`, `Active`, `Deleting`, `Archived`), so `UPDATING` is a value no response of this emulator can ever carry. UpdateFeed — the only operation that could produce it, since it reconfigures a live media feed's whole output list — leaves `feed.status` untouched and reports the feed's prior settled status.

Expected: a feed whose outputs are being replaced reports `UPDATING` while the reconfiguration is in flight and then settles back to `AVAILABLE` (no resource associated) or `ACTIVE`, which is what the modeled enum value exists for and what a client polling GetFeed after an UpdateFeed waits on. Actual: UpdateFeed answers 200 with `AVAILABLE`/`ACTIVE` and GetFeed never shows `UPDATING`, so a client that waits for the feed to leave `UPDATING` never observes the transition the real API models, and any client-side status table driven by the modeled enum has an unreachable branch.

The emulator already has the machinery for exactly this shape of transient status (`Feed::enter_transient` + `transition_due_at` + `settle_due_transitions`, used for DELETING and for a dictionary's CREATING), so reporting UPDATING for the span of the overwrite needs no new mechanism. This is the UpdateFeed counterpart of the open `DeleteFeed` issue about the unreachable `DELETED` value.

## Evidence

Spec: `smithy_query(query="shape",shape="FeedStatus")` -> values `CREATING, AVAILABLE, ACTIVE, UPDATING, DELETING, DELETED, ARCHIVED`; `UpdateFeedResponse$status` is "The status of the feed" and `UpdateFeed` is documented as the PUT that "completely overwrites the existing payload" of a feed's outputs.
Handler: `src/ops/update_feed.rs` `handle()` lines ~325-348 — the comment states the feed "reports the status it HOLDS" and no assignment to `feed.status` is made; `src/state.rs` lines ~127-151 define only four `FeedStatus` variants, so `UPDATING` is not expressible anywhere in the crate.
Tests: no test asserts UPDATING; `tests/all/error_message_rule.rs:212-215` and `tests/all/feed_association.rs:397` only record in comments that "neither CREATING nor UPDATING is ever reported", i.e. the omission is a design choice, not an asserted requirement.

## Steps to Reproduce

1. POST /v1/feed with one output -> note the feed id (status AVAILABLE)
2. PUT /v1/feed/{id} with a changed outputs array -> observe status AVAILABLE in the response
3. GET /v1/feed/{id} immediately afterwards -> observe AVAILABLE; UPDATING is never reported by any call, although the modeled FeedStatus enum declares it

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# A retried identical DisassociateFeed answers 400 ValidationException instead of replaying the success

_Filed: 2026-10-24T15:29:49Z_
_Operation: DisassociateFeed | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DisassociateFeed is modeled `@idempotent` and its `associatedResourceName` member carries `@idempotencyToken`, so a replay of the very same call (same feed id, same `associatedResourceName`) — exactly what an SDK does after a timeout or a dropped connection — must answer success and must not be treated as a new request.

The emulator keeps no record of a served request/token. The first call releases the resource, disables the outputs and parks the feed in the terminal `ARCHIVED` status. The second, identical call then fails the `feed.status != Active` precondition and is refused with HTTP 400 `ValidationException` carrying `feed_not_associated_message(ARCHIVED)` ("It has status ARCHIVED which indicate that it's not currently associated with a resource").

Expected: the replay answers HTTP 200 with the same `arn`/`id` body and leaves the feed exactly as the first call left it. Actual: HTTP 400 ValidationException, so a client that never saw the first response concludes the release failed when it in fact succeeded.

This is the DisassociateFeed leg of the same missing idempotency-token machinery already filed for AssociateFeed (`20261024T012105Z-elementalinference-associatefeed-wrong-behavior`); fixing it needs a served-token record shared by both operations, so the two are complementary rather than duplicate.

## Evidence

Spec: `smithy_query(query="operation",operation="DisassociateFeed")` — the operation is `@idempotent` and `associatedResourceName` is `@idempotencyToken` (len 1..2048, required); well-established AWS semantics for an idempotency token is that a repeat under the same token is a no-op that answers the original success.
Handler: `src/ops/disassociate_feed.rs` `handle()` lines ~220-227 refuse any feed whose status is not `Active`; lines ~259-269 set `association = None` and `status = Archived` (terminal), so after one successful call the replay can only land on the refusal path. Nothing stores the token that was served.
Tests: `tests/all/refusals_from_state.rs` builds an `ARCHIVED` feed precisely by associating and then disassociating, and asserts the 400 for the next disassociate — i.e. the replay path is currently asserted to refuse, which is the divergence being reported (the emulator's own comment calls ARCHIVED "terminal"). No test replays a request under the same idempotency token and expects success.

## Steps to Reproduce

1. POST /v1/feed with one output -> note the feed id
2. POST /v1/feed/{id}/associate with {"associatedResourceName": "source-media", "dryRun": false, "outputs": []} -> HTTP 200, the feed becomes ACTIVE
3. POST /v1/feed/{id}/disassociate with {"associatedResourceName": "source-media", "dryRun": false} -> HTTP 200, the feed becomes ARCHIVED
4. Replay the exact same disassociate request (same idempotency token) as an SDK retry would -> observe HTTP 400 ValidationException instead of the idempotent HTTP 200 repeating the same arn/id

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# A retried identical AssociateFeed answers 409 ConflictException instead of being idempotent

_Filed: 2026-10-24T01:21:05Z_
_Operation: AssociateFeed | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AssociateFeed is modeled `@idempotent` and its `associatedResourceName` member carries `@idempotencyToken`, so the service de-duplicates a repeated request by that token: replaying the very same call (same feed id, same `associatedResourceName`, same `outputs`) — which is exactly what an SDK does after a timeout or a connection reset — must answer success and must not apply the effect twice.

The emulator instead treats the replay as a fresh append. The first call appends the submitted outputs to the feed; the second call finds those same output names already on the feed and refuses with HTTP 409 `ConflictException` ("Feed … already has an output named …"). So a client that retries a request whose response it never saw is told the association conflicts, even though the association it asked for is exactly the one in place and was made under the same idempotency token.

Expected: the second, identical call answers HTTP 200 with the same `arn`/`id` body and leaves the feed with ONE copy of each output. Actual: HTTP 409 ConflictException. (The complementary hazard is the same bug seen from the other side: with the token ignored, the only thing that keeps a replay from appending a second copy of every output is the name-collision check, so for outputs whose names differ nothing dedupes at all.)

## Evidence

Spec: `OPERATIONS.md`/`smithy_query(query="operation",operation="AssociateFeed")` — AssociateFeed is `@idempotent`, `associatedResourceName` is `@idempotencyToken` (len 1..2048); well-established AWS semantics for an idempotency token is that a repeat under the same token is a no-op that answers success.
Handler: `src/ops/associate_feed.rs` `handle()` — the token value is only compared with the feed's current association for the ACTIVE precondition and then stored as `FeedAssociation`; no record of a previously-served token/request exists, and the "already has an output named" check (lines ~314-327) turns the replay into a ConflictException.
Tests: `tests/all/feed_association.rs:579 an_output_name_that_already_exists_is_a_conflict` covers a collision on a never-associated feed, and `:265 re_associating_the_same_resource_appends_more_outputs` covers a re-association with DIFFERENT output names. Neither replays an identical request, so the idempotent-retry path is unasserted.

## Steps to Reproduce

1. POST /v1/feed with one output named "crop1" → note the feed id
2. POST /v1/feed/{id}/associate with {"associatedResourceName": "source-media", "dryRun": false, "outputs": [{"name": "subs1", "outputConfig": {"subtitling": {"language": "eng"}}, "status": "ENABLED"}]} → HTTP 200, feed becomes ACTIVE with outputs crop1 + subs1
3. Replay the exact same POST (same resource name, same outputs) as an SDK retry would → observe HTTP 409 ConflictException instead of the idempotent HTTP 200 with the same arn/id and an unchanged output list

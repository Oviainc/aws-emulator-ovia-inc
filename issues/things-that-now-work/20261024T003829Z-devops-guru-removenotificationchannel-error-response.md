Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# Removing a channel id that names no channel returns 200 instead of ResourceNotFoundException (404)

_Filed: 2026-10-24T00:38:29Z_
_Operation: RemoveNotificationChannel | Class: error-response_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RemoveNotificationChannel treats an unknown (or already removed) `Id` as a successful no-op: the handler calls `remove` on the map and ignores the result, answering HTTP 200 with an empty body. The operation models `ResourceNotFoundException` (HTTP 404) and the only resource it names is the channel in the `{Id}` path label, so the not-found error has no other trigger and is currently unreachable. Expected: the first removal of an existing channel succeeds (200) and any subsequent removal of that id — or of an id that never named a channel — is refused with `ResourceNotFoundException` (404) carrying the channel id/type. Actual: every removal succeeds, so a client cannot detect a stale or mistyped channel id, and delete-then-verify flows cannot distinguish 'deleted now' from 'never existed'.

## Evidence

Spec: smithy_query(operation="RemoveNotificationChannel") — input `Id` (@httpLabel, required, NotificationChannelId uuid pattern), errors include ResourceNotFoundException(404) and ConflictException(409); no other resource participates in the request. Handler: src/ops/remove_notification_channel.rs line ~166 `guard.notification_channels.remove(&input.id);` with the return value discarded and `Ok(RemoveNotificationChannelOutput::default())` unconditionally. No ResourceNotFoundException is constructed in the file. The repo's own fixed bootstrap issue (20261021T215816Z-bootstrap-...md, step 5) states: "a second delete of the same Id, or an unknown Id, must return `ResourceNotFoundException` (404)". Tests: tests/all/mutation_idempotency_and_tenancy.rs `removing_a_channel_is_idempotent` asserts the current 200 no-op behavior, so no test asserts the modeled 404.

## Steps to Reproduce

1. PUT /channels with a valid Config to create a channel and note its Id.
2. DELETE /channels/{Id} — HTTP 200 (correct).
3. DELETE /channels/{Id} again with the same id.
4. Observe HTTP 200; expected ResourceNotFoundException with HTTP 404.
5. DELETE /channels/00000000-0000-0000-0000-000000000000 (an id this account never minted) and observe HTTP 200 where 404 is expected.

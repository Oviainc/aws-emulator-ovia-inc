Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# Re-adding an already-registered SNS topic returns 200 with the existing channel id instead of ConflictException (409)

_Filed: 2026-10-24T00:38:29Z_
_Operation: AddNotificationChannel | Class: error-response_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A second AddNotificationChannel for an SNS topic that already backs a channel is answered with HTTP 200 and the FIRST channel's `Id`. DevOps Guru allows a topic to back at most one notification channel, and the operation models `ConflictException` (HTTP 409) — a duplicate registration is the conflict that error exists for. Expected: the duplicate add is refused with `ConflictException` (409) and the store is unchanged. Actual: HTTP 200 carrying the pre-existing channel id, so a client that intended to create a second, distinct registration cannot tell that nothing was created, and a client probing for conflict never sees one. Note that `ConflictException` is currently unreachable in this handler: no code path constructs it, so the modeled 409 can never be observed for this operation.

## Evidence

Spec: smithy_query(operation="AddNotificationChannel") declares errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); the only conflict an add of a channel can have is a topic that is already registered (`NotificationChannelConfig.Sns.TopicArn` is the channel's identity, as `src/state.rs` NotificationChannelRecord itself documents: "the identity of the channel: a topic may back at most one channel"). Handler: services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/add_notification_channel.rs lines ~196-211 — `if let Some(existing) = guard.notification_channels.values().find(|channel| channel.sns_topic_arn == topic_arn) { return Ok(AddNotificationChannelOutput { id: existing.id.clone() }) }`; no ConflictException is constructed anywhere in the file. The repo's own (already fixed) bootstrap issue 20261021T215816Z-bootstrap-service-com-amazonaws-devopsguru-capstonecontrolplaneservice.md states the rule: "Adding the same `Sns.TopicArn` twice is a `ConflictException` (409)". Tests: tests/all/mutation_idempotency_and_tenancy.rs `registering_an_already_registered_topic_reports_the_one_channel` and `a_repeat_registration_reports_the_channel_as_registered` pin the current 200-with-existing-id behavior, i.e. the divergent behavior is asserted, not the spec one.

## Steps to Reproduce

1. Start the emulator and point a devops-guru client at it.
2. PUT /channels with {"Config":{"Sns":{"TopicArn":"arn:aws:sns:us-east-1:123456789012:guru-alerts"}}} — note the returned Id (HTTP 200, correct).
3. PUT /channels again with exactly the same Config.
4. Observe HTTP 200 with the same Id; expected ConflictException with HTTP 409 and no change to the channel store.
5. POST /channels (ListNotificationChannels) and observe one channel either way, so the client cannot distinguish 'created' from 'already existed'.

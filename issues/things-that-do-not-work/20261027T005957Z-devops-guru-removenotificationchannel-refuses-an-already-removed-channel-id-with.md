Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-guru.notification-channel-add-list-remove-lifecycle devops-guru.notification-channel-removal-enables-topic-reuse
Issue-Shape-Id: com.amazonaws.devopsguru#CapstoneControlPlaneService
Issue-Key: devops-guru|RemoveNotificationChannel|ResourceNotFoundException
Issue-Commit: 02e2c56a8a1bc6426078af793fc5f7b555907bec

# RemoveNotificationChannel refuses an already-removed channel id with ResourceNotFoundException instead of succeeding idempotently

> _Note: this issue was identified against the code base at commit `02e2c56a8a1bc6426078af793fc5f7b555907bec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RemoveNotificationChannel

## Description
One root cause, grounded in two failing behavioral checks.

Cause 1 of 1: RemoveNotificationChannel is not idempotent for an id that no longer names a stored channel.

AWS: RemoveNotificationChannel (Smithy: `RemoveNotificationChannel`, HTTP DELETE /channels/{Id}, success code 200, output shape `RemoveNotificationChannelResponse` which models no members; input `Id` -> NotificationChannelId, required, httpLabel, @length 36..36, pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$). The model does declare ResourceNotFoundException (404) among the operation's errors, but it does not state that a repeated removal of an already-removed channel is one of its triggers — the modeled error list is permissive, not a requirement to refuse. Recorded behavior only, and confirmed against real AWS by both failing checks: a second RemoveNotificationChannel call carrying an id whose channel was already removed SUCCEEDS — HTTP 200 with the empty `RemoveNotificationChannelResponse` body — rather than raising ResourceNotFoundException. The removal is a real deletion (the id stops resolving through ListNotificationChannels, and the SNS topic becomes available to register again under a brand-new Id), and the delete is a terminal idempotent no-op once the channel is gone.

Emulator: the handler deletes-then-decides and turns a miss into a refusal. The replay shows the second call answered with the modeled 404 refusal:
  code `ResourceNotFoundException`, message `no notification channel with id <Id> exists`.
In that message, `no notification channel with id ` and ` exists` are the emulator's own fixed text and `<Id>` is the channel id echoed from the request — neither the fixed text nor the refusal itself is AWS behavior for this call, so the whole response is the divergence, not just the variable part. AWS returns 200 and an empty body here and emits no error code and no message at all. The source comment at services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/remove_notification_channel.rs:163-168 states the intent explicitly ("an id that was never minted and an id an earlier removal already consumed are the same case" -> refuse), so this is a deliberate but wrong contract rather than an oversight.

Why: a client relying on AWS's behavior gets a spurious 404 on a benign repeat delete. This breaks the two normal shapes of caller code: (a) idempotent teardown/cleanup and retry-after-timeout paths, where a redelivered or retried DELETE now fails the whole teardown instead of being absorbed; and (b) "ensure absent" reconciliation loops (CloudFormation-style deleters, test fixtures, cleanup handlers), which must now special-case and swallow ResourceNotFoundException to work against the emulator while needing no such guard against real AWS. Because 404 is a client error the AWS SDKs do not retry, the failure surfaces to application code as a hard error on an operation AWS considers successful, so emulator-backed tests diverge from production exactly on the error-handling branch they are meant to validate.

Fix: the operation's handler — RemoveNotificationChannel's `handle` body, in its post-state-lookup decision step (the `if guard.notification_channels.remove(&input.id).is_none()` branch). The miss case must stop constructing ResourceNotFoundException and instead return the empty success output, making the deletion terminal and the repeat a no-op; the removal of the entry and the freeing of the SNS topic for re-registration under a new Id already behave correctly and must stay unchanged. No change is needed in input validation (the length/pattern constraints on `Id` still reject malformed ids with ValidationException) or in the serializer.

Traces: devops-guru.notification-channel-add-list-remove-lifecycle (add with no Filters, list, remove, then repeat remove -> got ResourceNotFoundException); devops-guru.notification-channel-removal-enables-topic-reuse (remove, confirm gone, repeat remove -> got ResourceNotFoundException before it could assert that the same topic re-registers under a new Id). Handler source: services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/remove_notification_channel.rs:169-181.

Not covered by any open issue: the three open devops-guru issues concern a ListAnomaliesForInsight scaffold stub, NextToken staleness handling in paginated reads, and UpdateResourceCollection AppBoundaryKey validation — none touches the notification-channel removal contract.

## Evidence
Expected (real AWS, per both checks): second RemoveNotificationChannel(Id=<already-removed id>) -> HTTP 200, empty RemoveNotificationChannelResponse body, no error code, no message.

Actual (emulator): HTTP 404, Error.Code=`ResourceNotFoundException`, Error.Message=`no notification channel with id <Id> exists` (fixed text `no notification channel with id ` + ` exists`; `<Id>` is the request's Id value), plus ResourceId=<Id> and ResourceType=<notification-channel resource type> from state.

Check assertions:
- devops-guru.notification-channel-add-list-remove-lifecycle: AssertionError: expected RemoveNotificationChannel to be idempotent on an already-removed channel '<id>' (confirmed against real AWS: a repeat call succeeds rather than raising ResourceNotFoundException), but it raised ResourceNotFoundException(...)
- devops-guru.notification-channel-removal-enables-topic-reuse: AssertionError: removing an already-removed channel should be an idempotent no-op (empirically confirmed against real AWS), got ResourceNotFoundException

Code confirming the deliberate refusal: services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/remove_notification_channel.rs:169-181 returns ResourceNotFoundException whenever the map removal yields None.

## Steps to Reproduce
1. sns.CreateTopic(Name=<unique name>) -> TopicArn.
2. devops-guru.AddNotificationChannel(Config={Sns:{TopicArn:<TopicArn>}}) -> Id.
3. Poll devops-guru.ListNotificationChannels until a channel with that Id appears.
4. devops-guru.RemoveNotificationChannel(Id=<Id>) -> 200, empty body.
5. Poll ListNotificationChannels until that Id no longer appears.
6. devops-guru.RemoveNotificationChannel(Id=<Id>) again (same, now-removed Id).
   Expected: HTTP 200 with an empty body.
   Observed: HTTP 404 ResourceNotFoundException, message `no notification channel with id <Id> exists`.
7. (second check only) After step 6 succeeds, AddNotificationChannel with the SAME TopicArn must mint a NEW, different Id and the old Id must still not resolve.

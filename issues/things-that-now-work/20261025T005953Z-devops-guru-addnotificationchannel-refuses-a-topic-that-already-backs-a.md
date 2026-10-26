Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-guru.add-notification-channel-duplicate-topic-is-idempotent devops-guru.notification-channel-duplicate-topic-race-exactly-one-wins
Issue-Shape-Id: com.amazonaws.devopsguru#CapstoneControlPlaneService
Issue-Key: devops-guru|AddNotificationChannel|ConflictException
Issue-Key: devops-guru.notification-channel-duplicate-topic-race-exactly-one-wins|run#10|AssertionError|assert len(ok_ids) == racers, f"expected all {racers} racers to succeed, got: {results}"
Issue-Commit: a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3

# AddNotificationChannel refuses a topic that already backs a channel with ConflictException (409) instead of being idempotent and returning the existing channel Id (200)

> _Note: this issue was identified against the code base at commit `a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddNotificationChannel

## Description
Cause 1 of 1: AddNotificationChannel treats "this SNS topic already backs a channel" as a creation conflict and refuses the call, where real AWS treats the target topic as the channel's idempotency key and returns the already-registered channel's Id with HTTP 200.

AWS: recorded/empirical behavior only. The Smithy model (AddNotificationChannel: PUT /channels, success 200, output Id required, NotificationChannelId @length 36..36 @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$) does declare ConflictException (HTTP 409) among the operation's errors, but says nothing about duplicate-topic registration being the trigger. Both failing checks state that real AWS behavior was empirically confirmed: a second AddNotificationChannel for a topic that is already registered returns the SAME channel Id as the first call, with no error, and exactly one channel for that topic exists afterwards (ListNotificationChannels shows one entry whose Config.Sns.TopicArn is that topic). Under concurrency this holds too: N simultaneous AddNotificationChannel calls for one topic ALL succeed and ALL resolve to the same Id. So the duplicate add is a successful idempotent re-registration, not a refusal.

Emulator: the duplicate add is refused with ConflictException, HTTP 409. Replay of devops-guru.add-notification-channel-duplicate-topic-is-idempotent: botocore.errorfactory.ConflictException — "An error occurred (ConflictException) when calling the AddNotificationChannel operation: the Amazon SNS topic <topic-arn> already backs notification channel <existing-channel-id>" (message quoted as an example only; the diverging requirements are the 409 status and the ConflictException code, which should not be emitted at all for this input). Replay of devops-guru.notification-channel-duplicate-topic-race-exactly-one-wins: of 3 concurrent racers for one topic, exactly 1 returned an Id and the other 2 were refused with the same ConflictException, so only one of N callers gets a usable channel Id.

Why: a client relying on AWS's behavior loses idempotency on a control-plane registration. Re-running a setup/bootstrap script, a retried SDK call, a CloudFormation-style converge loop, or any N-worker fan-out that each ensure the same SNS topic is registered will see a hard 409 instead of the existing channel Id. Because ConflictException is not a retryable error, the caller aborts; and because the emulator's refusal carries no Id to the loser, callers that legitimately need the channel Id (to later call RemoveNotificationChannel, or to assert their own setup) cannot obtain it without an extra ListNotificationChannels round trip that real AWS never requires. Conversely, code written against the emulator learns to catch 409, which is dead code against AWS.

Fix: the operation's handler — services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/add_notification_channel.rs, the "CREATION CONFLICT" branch that searches notification_channels for a record whose sns_topic_arn equals the request's topic and returns ConflictException. That lookup should instead short-circuit to a successful response carrying the existing record's Id (no new id minted, no second record inserted, no mutation of the stored config), leaving the store with exactly one channel per topic. The lookup-and-insert must stay inside the single state lock the handler already holds so the concurrent case collapses to the same Id rather than racing. The module doc comment at the top of the same file also asserts the refusal contract ("an add whose topic already backs one is refused with ConflictException (HTTP 409)") and needs to be corrected with it. ConflictException remains modeled for the operation but has no duplicate-topic trigger. Note: this is distinct from the already-open issue 20261024T003829Z-devops-guru-removenotificationchannel-error-response.md, which concerns RemoveNotificationChannel's missing 404; that issue does not cover this add-path refusal.

Traces: devops-guru.add-notification-channel-duplicate-topic-is-idempotent (sequential second add refused), devops-guru.notification-channel-duplicate-topic-race-exactly-one-wins (2 of 3 concurrent racers refused).

## Evidence
Smithy (services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/smithy-model/com-amazonaws-devopsguru-capstonecontrolplaneservice.json): AddNotificationChannel http PUT /channels code 200; input Config (NotificationChannelConfig, required); output Id (NotificationChannelId, required, @length 36..36, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$); errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400). The model does not state that a duplicate target topic triggers ConflictException.

Expected (per both checks, which state real-AWS behavior was empirically confirmed):
- AddNotificationChannel(Config.Sns.TopicArn=T) twice => both HTTP 200, second Id == first Id.
- ListNotificationChannels afterwards => exactly ONE channel with Config.Sns.TopicArn == T, whose Id is that shared Id.
- 3 concurrent AddNotificationChannel(T) => all 3 HTTP 200, all 3 return the same Id, one channel for T afterwards.

Actual (emulator replay):
- devops-guru.add-notification-channel-duplicate-topic-is-idempotent [error]: second add => HTTP 409 ConflictException. botocore: "An error occurred (ConflictException) when calling the AddNotificationChannel operation: the Amazon SNS topic arn:aws:sns:us-west-2:000000000000:sim2real-devopsguru-topic-<suffix> already backs notification channel <existing-id>" (message text is an example; status 409 + code ConflictException are the requirement violated).
- devops-guru.notification-channel-duplicate-topic-race-exactly-one-wins [mismatch]: results were [('ok', '<id>'), ('err', ConflictException ...), ('err', ConflictException ...)] — assertion "expected all 3 racers to succeed" failed with only 1 ok.

Code: src/ops/add_notification_channel.rs — `if let Some(existing) = guard.notification_channels.values().find(|channel| channel.sns_topic_arn == topic_arn) { return Err(...ConflictException { message: format!("the Amazon SNS topic {topic_arn} already backs notification channel {}", existing.id), resource_id: existing.id.clone(), resource_type: NOTIFICATION_CHANNEL_RESOURCE_TYPE } ...) }` (the "CREATION CONFLICT" block, ~lines 197-225). This is the only place the duplicate-topic 409 is produced; the success path below it mints a fresh UUID unconditionally.

## Steps to Reproduce
1. sns.create_topic(Name=<unique>) -> note TopicArn T.
2. devops-guru.add_notification_channel(Config={'Sns': {'TopicArn': T}}) -> HTTP 200, returns Id1 (correct).
3. Poll list_notification_channels until Id1 appears.
4. devops-guru.add_notification_channel(Config={'Sns': {'TopicArn': T}}) again with the SAME topic ARN.
5. Observe HTTP 409 ConflictException. Expected: HTTP 200 with Id == Id1.
6. Concurrency variant: create a fresh topic T2 and fire 3 simultaneous add_notification_channel(Config={'Sns': {'TopicArn': T2}}) calls from a thread pool.
7. Observe 1 success and 2 ConflictException refusals. Expected: 3 successes all returning the same Id, and exactly one channel for T2 in list_notification_channels.

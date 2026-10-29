Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.add-notification-channels-duplicate-uri-tolerated codeguruprofiler.add-notification-channels-duplicate-uri-and-quota-rejected codeguruprofiler.add-notification-channels-quota-exceeded-rejected codeguruprofiler.add-notification-channels-batch-of-two-in-single-call
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.add-notification-channels-duplicate-uri-tolerated|run#18|AssertionError|assert len(dup_channels) == 1, dup_channels
Issue-Key: codeguruprofiler|AddNotificationChannels|ServiceQuotaExceededException
Issue-Key: codeguruprofiler.add-notification-channels-quota-exceeded-rejected|run#15|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.add-notification-channels-batch-of-two-in-single-call|run#17|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# AddNotificationChannels: duplicate-uri add is not idempotent (second entry stored, counts against the 2-channel quota) and the quota refusal message is not AWS's wording

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddNotificationChannels

## Description
Two net-new root causes in the AddNotificationChannels handler of service-com-amazonaws-codeguruprofiler-codeguruprofiler. (Both are distinct from the already-open LOW issue 20261029T022323Z-codeguruprofiler-addnotificationchannels-wrong-behavior.md, which concerns only the ORDER in which channel ids are minted relative to the refusals and asserts every response is unchanged by its fix.)

Cause 1 of 2: a re-add of a uri that already has a channel creates a SECOND channel entry for that uri instead of being idempotent.
AWS: recorded behavior only (the Smithy model constrains AddNotificationChannels.channels to @length min 1 max 2 with @uniqueItems, and documents only ConflictException/ServiceQuotaExceededException/ValidationException, saying nothing about uri dedupe). Replay against real AWS shows AddNotificationChannels with a uri the stored configuration already has a channel for is idempotent at the URI level: the call succeeds (200), the configuration still holds exactly ONE channel for that uri (the existing channel is replaced, and the replacement is issued a freshly generated channel id, while channels for other uris keep their ids), and the re-add does NOT consume a quota slot — it is accepted even when the group is already at the maximum of two channels.
Emulator: dedupe is keyed only on the channel id (src/ops/add_notification_channels.rs:263-278 refuses only when `group.has_notification_channel(&channel.id)`), and a request that names no id gets a freshly minted id (:220-228), so the uri is never compared. The re-add is stored as an additional entry: GetNotificationConfiguration then reports two channels with the same uri and different ids (replay shows `[{id: 787058f0-…, uri: arn:aws:sns:…:sim2real-behav-topic-…}, {id: 4dff947d-…, uri: <same arn>}]`). Because that extra entry counts, the same re-add performed while the group is at two channels is refused with 402 ServiceQuotaExceededException (replay: `profiling group <name> already has 2 of 2 notification channels; remove one before adding 1`) where AWS accepts it.
Why: a client that re-asserts its desired notification configuration (a common converge/retry pattern) silently gets duplicate channels — every anomaly notification is published twice to the same SNS topic — and once the group has been grown that way it hits a spurious quota refusal on an operation AWS accepts, so the converge loop can never succeed.
Fix: the duplicate detection in the AddNotificationChannels handler (and the channel-set mutation it calls in state): a requested channel whose uri matches a stored channel must replace that stored channel (with a newly minted id) rather than be appended, and must not be counted when the quota check is applied.
Traces: codeguruprofiler.add-notification-channels-duplicate-uri-tolerated (mismatch: `assert len(dup_channels) == 1` saw two entries for one uri), codeguruprofiler.add-notification-channels-duplicate-uri-and-quota-rejected (error: the idempotent re-add at the max was rejected with ServiceQuotaExceededException instead of succeeding).

Cause 2 of 2: the ServiceQuotaExceededException message for exceeding the two-channel limit is not AWS's message.
AWS: recorded behavior only for the text (Smithy: AddNotificationChannels errors include ServiceQuotaExceededException, httpError 402, documented as "You have exceeded your service quota. To perform the requested action, remove some of the relevant resources, or use Service Quotas to request a service quota increase."). The recorded response carries the fixed sentence `Channel limit exceeded: Only 2 channels allowed per profiling group` — the whole sentence is fixed text for this error class; its only variable part is the channel maximum (2), which is the service limit, and it names neither the profiling group nor the requested/current counts.
Emulator: answers ServiceQuotaExceededException with a message it composes itself from request and state values — `profiling group {profilingGroupName} already has {current} of {max} notification channels; remove one before adding {requested}` (src/ops/add_notification_channels.rs:286-301); replay saw `profiling group sim2real-behav-cgp-… already has 2 of 2 notification channels; remove one before adding 1` and, for a batch of two with one free slot, `… already has 1 of 2 notification channels; remove one before adding 2`. The error code (ServiceQuotaExceededException) and the refusal itself are correct; only the message diverges.
Why: a client or test that matches on the quota message (the only place AWS states which limit was hit) does not recognise the emulator's text, and operators reading emulator logs see a sentence AWS never emits.
Fix: the message construction for the quota refusal in the AddNotificationChannels handler's validation — emit the fixed sentence for this error class rather than a per-request composed one.
Traces: codeguruprofiler.add-notification-channels-quota-exceeded-rejected, codeguruprofiler.add-notification-channels-batch-of-two-in-single-call (both assert the message exactly; the batch check also confirms the correct atomic refusal and correct code).

## Evidence
Cause 1 — codeguruprofiler.add-notification-channels-duplicate-uri-tolerated:
  AssertionError: [{'eventPublishers': ['AnomalyDetection'], 'id': '787058f0-be14-7e89-7240-343805107961', 'uri': 'arn:aws:sns:us-west-2:000000000000:sim2real-behav-topic-47097031be9f'}, {'eventPublishers': ['AnomalyDetection'], 'id': '4dff947d-daef-4567-9a45-27332e405c86', 'uri': 'arn:aws:sns:us-west-2:000000000000:sim2real-behav-topic-47097031be9f'}]
  expected: exactly one channel for that uri (AWS replaces, assigning a fresh id).

Cause 1 — codeguruprofiler.add-notification-channels-duplicate-uri-and-quota-rejected:
  botocore.errorfactory.ServiceQuotaExceededException: An error occurred (ServiceQuotaExceededException) when calling the AddNotificationChannels operation: profiling group sim2real-cgp-dupquota-… already has 2 of 2 notification channels; remove one before adding 1
  expected: HTTP 200 — at the max, re-adding an already-present uri does not count against the quota.

Emulator source (dedupe keyed on id only, uri never compared): src/ops/add_notification_channels.rs:263-278; id minted per request at :220-228; quota check counts the duplicate at :283-301.

Cause 2 — codeguruprofiler.add-notification-channels-quota-exceeded-rejected:
  actual message:   'profiling group sim2real-behav-cgp-91d9248d417f already has 2 of 2 notification channels; remove one before adding 1'
  expected message: 'Channel limit exceeded: Only 2 channels allowed per profiling group'
Cause 2 — codeguruprofiler.add-notification-channels-batch-of-two-in-single-call:
  actual message:   'profiling group sim2real-cgp-3063cd3e2b0e already has 1 of 2 notification channels; remove one before adding 2'
  expected message: 'Channel limit exceeded: Only 2 channels allowed per profiling group'
  (code ServiceQuotaExceededException and the all-or-nothing batch refusal both matched; only the message diverged.)

Smithy (services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/smithy-model/com-amazonaws-codeguruprofiler-codeguruprofiler.json): AddNotificationChannels POST /profilingGroups/{profilingGroupName}/notificationConfiguration, 200; channels @required @length(1..2) @uniqueItems; errors ConflictException 409, ServiceQuotaExceededException 402, ValidationException 400, ResourceNotFoundException 404.

## Steps to Reproduce
1. CreateProfilingGroup with a fresh name.
2. CreateTopic twice in SNS to get uri_x and uri_y.
3. AddNotificationChannels(profilingGroupName=name, channels=[{uri: uri_x, eventPublishers: [AnomalyDetection]}]) — succeeds, one channel.
4. Repeat the IDENTICAL call. Emulator returns two channels, both with uri_x and different ids; AWS returns one channel for uri_x with a freshly minted id. GetNotificationConfiguration confirms the duplicate entry is stored.
5. Fill the group to two distinct uris (uri_x, uri_y), then re-add uri_x. Emulator refuses with 402 ServiceQuotaExceededException; AWS accepts it (an idempotent re-add consumes no slot).
6. With two distinct uris stored, AddNotificationChannels a third distinct uri_z. The emulator correctly refuses with ServiceQuotaExceededException but the message is 'profiling group <name> already has 2 of 2 notification channels; remove one before adding 1' instead of 'Channel limit exceeded: Only 2 channels allowed per profiling group'.
7. With one slot free, submit a batch of two channels: the refusal is correctly atomic and correctly coded, but carries the same non-AWS message.

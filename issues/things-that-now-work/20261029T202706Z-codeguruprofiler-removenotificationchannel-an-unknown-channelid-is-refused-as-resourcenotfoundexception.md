Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.remove-notification-channel-unknown-channel-not-found-rejected codeguruprofiler.notification-channel-add-remove-lifecycle
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.remove-notification-channel-unknown-channel-not-found-rejected|run#12|AssertionError|assert code == "ValidationException", code
Issue-Key: codeguruprofiler.notification-channel-add-remove-lifecycle|run#24|AssertionError|assert err["Code"] == "ValidationException", err
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# RemoveNotificationChannel: an unknown channelId is refused as ResourceNotFoundException (404) instead of AWS's ValidationException (400) with the "Channel not found with channelId: …" message

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RemoveNotificationChannel

## Description
One root cause in the RemoveNotificationChannel handler of service-com-amazonaws-codeguruprofiler-codeguruprofiler.

Cause 1 of 1: a well-formed channelId that the existing profiling group holds no channel for is classified as a missing RESOURCE rather than as an invalid request parameter.
AWS: Smithy models RemoveNotificationChannel (DELETE /profilingGroups/{profilingGroupName}/notificationConfiguration/{channelId}, 200) with ValidationException httpError 400 ("The parameter is not valid.") and ResourceNotFoundException httpError 404 ("The resource specified in the request does not exist."); channelId is a @required httpLabel with pattern [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}. Recorded behavior: when the named profiling group EXISTS but holds no channel with that id (including an id that was just successfully removed), AWS answers 400 ValidationException with the message `Channel not found with channelId: {channelId}, profilingGroupName: {profilingGroupName} and AccountId: {accountId}`. Fixed text: `Channel not found with channelId: `, `, profilingGroupName: `, ` and AccountId: `. Variable parts: channelId from the request, profilingGroupName from the request, and the caller's account id from the request's credentials — the recorded ids/names in a single run only fill that template. ResourceNotFoundException (404) is reserved for the profiling group itself not existing.
Emulator: src/ops/remove_notification_channel.rs:173-185 answers `ResourceNotFoundException` (HTTP 404) with the self-composed message `profiling group {profilingGroupName} has no notification channel {channelId}` whenever `group.remove_notification_channel(&channel_id)` returns false — i.e. it uses the same error class for "this group has no such channel" as for "this group does not exist". Replay saw `{'Code': 'ResourceNotFoundException', 'Message': 'profiling group sim2real-cgp-9821fe521548 has no notification channel 24a6a9dc-2965-63fa-3216-a58b9118161c'}` where 400/ValidationException with the template message above is required.
Why: a client cannot distinguish "the profiling group is gone" (404, retire/recreate the group) from "that channel id is stale" (400, re-read the configuration), so error handling and retry logic branch the wrong way; botocore's typed exception is ResourceNotFoundException rather than ValidationException, so except-blocks written against AWS do not catch it at all, and the HTTP status differs (404 vs 400), which also changes proxy/retry treatment. The message also omits the account id AWS reports.
Fix: the channel-lookup refusal in the RemoveNotificationChannel handler — classify an unknown channelId on an existing group as the operation's ValidationException with AWS's message template (keeping ResourceNotFoundException for the group-not-found case above it at :154-167).
Traces: codeguruprofiler.remove-notification-channel-unknown-channel-not-found-rejected (fresh random uuid as channelId), codeguruprofiler.notification-channel-add-remove-lifecycle (second removal of an id already removed). Note the open maintainability issue 20261029T170543Z-codeguruprofiler-addnotificationchannels-maintainability.md discusses the shared group_not_found_message comment; it does not cover this error-class divergence.

## Evidence
codeguruprofiler.remove-notification-channel-unknown-channel-not-found-rejected:
  assert code == "ValidationException"  ->  AssertionError: ResourceNotFoundException
  expected message: 'Channel not found with channelId: {unknown_channel_id}, profilingGroupName: {name} and AccountId: {account}'

codeguruprofiler.notification-channel-add-remove-lifecycle (second removal of an already-removed id):
  AssertionError: {'Message': 'profiling group sim2real-cgp-9821fe521548 has no notification channel 24a6a9dc-2965-63fa-3216-a58b9118161c', 'Code': 'ResourceNotFoundException'}
  expected: Code 'ValidationException' (HTTP 400), Message 'Channel not found with channelId: <channelId>, profilingGroupName: <name> and AccountId: <account>'
  (everything else in that lifecycle — add, GetNotificationConfiguration, the first removal and its empty channel list — matched.)

Emulator source: src/ops/remove_notification_channel.rs:173-185 returns RemoveNotificationChannelErrorModeled::ResourceNotFoundException with the composed message; the group-not-found refusal at :154-167 already uses that same class.

Smithy (services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/smithy-model/com-amazonaws-codeguruprofiler-codeguruprofiler.json): RemoveNotificationChannel errors = InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400; channelId @required httpLabel pattern [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}.

## Steps to Reproduce
1. CreateProfilingGroup with a fresh name; CreateTopic in SNS for a uri.
2. AddNotificationChannels(profilingGroupName=name, channels=[{uri, eventPublishers:[AnomalyDetection]}]) and note the returned channel id.
3. RemoveNotificationChannel(profilingGroupName=name, channelId=<a fresh random uuid>). Emulator: 404 ResourceNotFoundException 'profiling group <name> has no notification channel <id>'. AWS: 400 ValidationException 'Channel not found with channelId: <id>, profilingGroupName: <name> and AccountId: <account>'.
4. Alternatively RemoveNotificationChannel with the real channel id twice — the second call reproduces the same divergence, while the profiling group still exists.
5. Confirm the group-not-found case is unaffected: RemoveNotificationChannel against a non-existent profilingGroupName must stay 404 ResourceNotFoundException.

Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-submit-feedback-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-submit-feedback-error codeguruprofiler-notification-channels-lifecycle
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|SubmitFeedback|4xx->2xx|$
Issue-Key: codeguru-profiler|SubmitFeedback|4xx->2xx|status
Issue-Key: codeguru-profiler|AddNotificationChannels|4xx->4xx|$.message
Issue-Key: codeguru-profiler|RemoveNotificationChannel|4xx->4xx|$.__type
Issue-Key: codeguru-profiler|RemoveNotificationChannel|4xx->4xx|$.message
Issue-Key: codeguru-profiler|RemoveNotificationChannel|4xx->4xx|status
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# Client-error refusals diverge from AWS: wrong error class, wrong message text, or no refusal at all

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitFeedback

## Description
Three independent causes in the codeguru-profiler crate make a refusal the recorded service answers come back with the wrong status, the wrong error class, or not at all. All three must be closed before this issue is submitted.

Cause 1 of 3 — SubmitFeedback accepts feedback about an anomaly instance the group does not have (fixture codeguruprofiler-submit-feedback-error, step task#2).
AWS: Smithy models SubmitFeedback (POST /internal/profilingGroups/{profilingGroupName}/anomalies/{anomalyInstanceId}/feedback, success 204, no output members) with errors ValidationException httpError 400 ("The parameter is not valid."), ResourceNotFoundException 404, ThrottlingException 429, InternalServerException 500; anomalyInstanceId is a @required httpLabel with pattern [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}. Recorded behavior: when the profiling group EXISTS but holds no anomaly instance with that (well-formed) id, the service refuses with HTTP 400 and body {"__type":"ValidationException","message":"Anomaly with id: <anomalyInstanceId> not found for accountId: <accountId>"}. Fixed text of that message: "Anomaly with id: ", " not found for accountId: ". Variable parts: the anomalyInstanceId from the request's URI label and the caller's account id from the request's credentials — the uuid and account digits in the recording only fill that template.
Emulator: answers the success status 204 with an empty body and stores the feedback anyway. The handler looks up only the profiling group (ResourceNotFoundException when the group is missing) and then calls group.submit_anomaly_feedback(..) unconditionally for whatever anomalyInstanceId arrived; no anomaly-existence check exists on the path.
Why: a client that submits feedback against a stale or mistyped anomaly id is told it succeeded. The feedback is silently attached to an anomaly that does not exist, so later reads never show it, and code that branches on botocore's ValidationException (e.g. to re-list anomalies and retry with a current id) never runs. The HTTP status differs too (204 vs 400), so retry/proxy handling differs.
Fix: the SubmitFeedback handler's validation — resolve the anomalyInstanceId against the anomalies the named group actually holds and refuse an unknown one with the operation's modelled ValidationException (400) and AWS's message template, before any feedback is recorded. Keep ResourceNotFoundException (404) for the group-not-found case.
Traces: codeguruprofiler-submit-feedback-error#task#2. Source: src/ops/submit_feedback.rs:180-200 (group lookup then unconditional submit_anomaly_feedback).

Cause 2 of 3 — AddNotificationChannels refuses a non-ARN channel uri with its own sentence instead of AWS's (fixture codeguruprofiler-notification-channels-lifecycle, step task#3).
AWS: Smithy models AddNotificationChannels (POST /profilingGroups/{profilingGroupName}/notificationConfiguration, 200) with ValidationException httpError 400; Channel.uri is @required and documented "Unique arn of the resource to be used for notifications. We support a valid SNS topic arn as a channel uri." Recorded behavior: a uri that is not an SNS topic ARN is refused with HTTP 400 and {"__type":"ValidationException","message":"Channel Validation failed: Invalid topic ARN: <uri>"}. Fixed text: "Channel Validation failed: Invalid topic ARN: "; the single variable part is the rejected uri exactly as the request sent it (the recording's "not-an-sns-arn" is just that request's value).
Emulator: same status (400) and same error class, but a self-composed message: "channels[0].uri must be an Amazon SNS topic ARN (arn:<partition>:sns:<region>:<account>:<topic>), got <uri>" — it names a member path and a grammar AWS never mentions, and omits AWS's "Channel Validation failed: Invalid topic ARN:" prefix.
Why: the message is part of the contract for this error class. Clients, tests and log scrapers that match on AWS's wording (or surface it to users) see text that never appears in AWS, and the emulator's index-qualified member path invites clients to parse a path AWS does not provide.
Fix: the channel-uri validation in the AddNotificationChannels handler — emit AWS's message template for an invalid topic ARN instead of the locally composed sentence. Note the open issues 20261029T022323Z-codeguruprofiler-addnotificationchannels-wrong-behavior.md (channel ids minted before the refusal) and 20261029T202706Z-codeguruprofiler-addnotificationchannels-duplicate-uri-add-is-not-idempotent-second.md (duplicate-uri idempotency and the quota message) touch this handler but neither covers the invalid-topic-ARN message.
Traces: codeguruprofiler-notification-channels-lifecycle#task#3.

Cause 3 of 3 — RemoveNotificationChannel classifies an unknown channelId as a missing resource (fixture codeguruprofiler-notification-channels-lifecycle, step task#4).
AWS: Smithy models RemoveNotificationChannel (DELETE /profilingGroups/{profilingGroupName}/notificationConfiguration/{channelId}, 200, idempotent) with ValidationException httpError 400 and ResourceNotFoundException httpError 404 ("The resource specified in the request does not exist."); channelId is a @required httpLabel with the uuid pattern. Recorded behavior: when the named profiling group EXISTS but holds no channel with that id, the service answers HTTP 400 {"__type":"ValidationException","message":"Channel not found with channelId: <channelId>, profilingGroupName: <profilingGroupName> and AccountId: <accountId>"}. Fixed text: "Channel not found with channelId: ", ", profilingGroupName: ", " and AccountId: "; variable parts: channelId and profilingGroupName from the request and the caller's account id. 404/ResourceNotFoundException is reserved for the profiling group itself not existing.
Emulator: answers HTTP 404 with {"__type":"ResourceNotFoundException","message":"profiling group <name> has no notification channel <channelId>"} — the same error class it uses for a missing group, with a different message and no account id.
Why: a client cannot distinguish "the group is gone" from "that channel id is stale", so retry logic branches the wrong way; botocore raises ResourceNotFoundException where except-blocks written against AWS catch ValidationException, and the status differs (404 vs 400), changing proxy and retry treatment.
Fix: the channel-lookup refusal in the RemoveNotificationChannel handler — classify an unknown channelId on an existing group as the operation's ValidationException (400) with AWS's message template, keeping ResourceNotFoundException for the group-not-found branch above it.
Traces: codeguruprofiler-notification-channels-lifecycle#task#4. Source: src/ops/remove_notification_channel.rs:173-185. The open behavioral issue 20261029T202706Z-codeguruprofiler-removenotificationchannel-an-unknown-channelid-is-refused-as-resourcenotfoundexception.md describes the same error-class fault from the behavioral suite, but the replay reports this wire step as unowned (it carries no wire replay key), so the fixture binding is recorded here.

## Evidence
Cause 1 — codeguruprofiler-submit-feedback-error#task#2 (SubmitFeedback, unknown anomalyInstanceId on an existing group):
  expected: HTTP 400, body {"message": "Anomaly with id: <anomalyInstanceId> not found for accountId: <accountId>", "__type": "ValidationException"}
  actual:   HTTP 204, empty body (feedback stored)
  replay detail: "status: expected 400, got 204 / $: empty_body"

Cause 2 — codeguruprofiler-notification-channels-lifecycle#task#3 (AddNotificationChannels, channels[0].uri = a non-ARN string):
  status matched (400) and __type matched (ValidationException); $.message diverged
  expected: 'Channel Validation failed: Invalid topic ARN: <uri>'
  actual:   '<member-path> must be an Amazon SNS topic ARN (arn:<partition>:sns:<region>:<account>:<topic>), got <uri>'

Cause 3 — codeguruprofiler-notification-channels-lifecycle#task#4 (RemoveNotificationChannel, unknown channelId on an existing group):
  expected: HTTP 400, __type 'ValidationException', message 'Channel not found with channelId: <channelId>, profilingGroupName: <name> and AccountId: <accountId>'
  actual:   HTTP 404, __type 'ResourceNotFoundException', message 'profiling group <name> has no notification channel <channelId>'

Smithy (services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/smithy-model/com-amazonaws-codeguruprofiler-codeguruprofiler.json): SubmitFeedback success 204, errors ValidationException 400 / ResourceNotFoundException 404 / ThrottlingException 429 / InternalServerException 500, anomalyInstanceId @required httpLabel pattern [a-f0-9]{8}-...; AddNotificationChannels errors include ValidationException 400, Channel.uri @required ("We support a valid SNS topic arn as a channel uri"); RemoveNotificationChannel errors ValidationException 400 and ResourceNotFoundException 404; ValidationException = {message: String required}, httpError 400, doc "The parameter is not valid."

## Steps to Reproduce
Cause 1 (SubmitFeedback):
1. CreateProfilingGroup with a fresh name.
2. SubmitFeedback(profilingGroupName=<name>, anomalyInstanceId=<a well-formed uuid the group has no anomaly for>, type=Positive). Emulator: 204, empty body. AWS: 400 ValidationException 'Anomaly with id: <id> not found for accountId: <account>'.
3. Confirm the group-not-found branch still answers 404 ResourceNotFoundException for a non-existent profilingGroupName.

Cause 2 (AddNotificationChannels):
1. CreateProfilingGroup with a fresh name.
2. AddNotificationChannels(profilingGroupName=<name>, channels=[{uri: 'not-an-sns-arn', eventPublishers: ['AnomalyDetection']}]). Status/class already match; compare $.message against 'Channel Validation failed: Invalid topic ARN: not-an-sns-arn'.

Cause 3 (RemoveNotificationChannel):
1. With that group existing, RemoveNotificationChannel(profilingGroupName=<name>, channelId=<a well-formed uuid the group holds no channel for>). Emulator: 404 ResourceNotFoundException. AWS: 400 ValidationException with the 'Channel not found with channelId: ...' template.
2. Re-replay fixtures codeguruprofiler-submit-feedback-error (task#2) and codeguruprofiler-notification-channels-lifecycle (task#3, task#4) to confirm all three causes.

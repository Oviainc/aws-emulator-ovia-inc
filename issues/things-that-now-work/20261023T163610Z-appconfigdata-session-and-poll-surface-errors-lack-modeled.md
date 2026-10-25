Issue-Source: trace-sweep-divergence
Issue-Trace: appconfigdata-error-invalid-configuration-token
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfigdata-error-invalid-configuration-token appconfigdata-session-and-poll-error-envelopes appconfigdata-error-unknown-application appconfigdata-poll-interval-enforcement appconfigdata-session-and-latest-configuration
Issue-Shape-Id: com.amazonaws.appconfigdata#AppConfigData
Issue-Key: appconfigdata|GetLatestConfiguration|4xx->4xx|$.Details.InvalidParameters.ConfigurationToken
Issue-Key: appconfigdata|GetLatestConfiguration|4xx->4xx|$.Message
Issue-Key: appconfigdata|StartConfigurationSession|4xx->2xx|$.Message
Issue-Key: appconfigdata|StartConfigurationSession|4xx->2xx|$.ReferencedBy
Issue-Key: appconfigdata|StartConfigurationSession|4xx->2xx|$.ResourceType
Issue-Key: appconfigdata|StartConfigurationSession|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appconfigdata|StartConfigurationSession|4xx->2xx|status
Issue-Key: appconfigdata|GetLatestConfiguration|2xx->4xx|$.probe
Issue-Key: appconfigdata|GetLatestConfiguration|2xx->4xx|$.revision
Issue-Key: appconfigdata|GetLatestConfiguration|2xx->4xx|$headers.next-poll-configuration-token
Issue-Key: appconfigdata|GetLatestConfiguration|2xx->4xx|$headers.next-poll-interval-in-seconds
Issue-Key: appconfigdata|GetLatestConfiguration|2xx->4xx|$headers.version-label
Issue-Key: appconfigdata|GetLatestConfiguration|2xx->4xx|status
Issue-Key: appconfigdata|StartConfigurationSession|4xx->4xx|$.Message
Issue-Key: appconfigdata|GetLatestConfiguration|4xx->5xx|$.Message
Issue-Key: appconfigdata|GetLatestConfiguration|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: appconfigdata|GetLatestConfiguration|4xx->5xx|status
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# AppConfigData session and poll surface: errors lack modeled envelopes and a deployed configuration is unreachable (5 causes)

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLatestConfiguration

## Description
Five distinct root causes in the bootstrapped appconfigdata emulator, every one grounded in a replayed wire fixture step. They are numbered Cause 1 of 5 .. Cause 5 of 5; close ALL FIVE before submitting, and re-replay every fixture#step listed in Issue-Trace/Issue-Step and Issue-Trace-Ids to confirm.

AppConfigData has only two operations (Smithy sdkId AppConfigData, protocol restJson1): StartConfigurationSession (POST /configurationsessions, success code 201) and GetLatestConfiguration (GET /configuration, success code 200). Both model exactly BadRequestException (400), ResourceNotFoundException (404), InternalServerException (500) and ThrottlingException (429). Four of the five causes below are refusal-envelope defects on that tiny surface, and the fifth is the happy path of the whole service.

=== Cause 1 of 5: BadRequestException for an invalid ConfigurationToken carries no Reason/Details discriminator ===
AWS: a GetLatestConfiguration call whose ConfigurationToken is not a usable token is refused with BadRequestException, HTTP 400, and the envelope says WHY in structured form (Smithy: BadRequestException members Message, Reason -> BadRequestReason enum {InvalidParameters}, Details -> BadRequestDetails, a union whose InvalidParameters member is a map of parameter name -> InvalidParameterDetail{Problem}; InvalidParameterProblem enum values are Corrupted, Expired, PollIntervalNotSatisfied). The recording shows the token-rejection envelope as Details.InvalidParameters.ConfigurationToken = {"Problem": "Corrupted"} (message example: 'Token not valid').
Emulator: the status 400 is right, but the response body carries NO Details member at all ($.Details.InvalidParameters.ConfigurationToken missing, expected {'Problem': 'Corrupted'}), and the message is the emulator's own prose ('The configuration token is not valid for any configuration session.') rather than a per-parameter problem code.
Why: the modeled way for a client to tell a corrupted token from an expired one from a too-early poll is Details.InvalidParameters[<param>].Problem. With Details absent, every bad-token case looks identical: a polling loop cannot distinguish 'restart the session' (Corrupted/Expired) from 'back off and retry the same token' (PollIntervalNotSatisfied), and SDK code reading Reason/Details gets None. The message text itself is an example, not the requirement - the structured discriminator is.
Fix: the GetLatestConfiguration token-validation path and the appconfigdata error serializer - populate BadRequestException.Reason='InvalidParameters' and Details.InvalidParameters with an entry keyed by the offending input member ('ConfigurationToken') whose Problem is the modeled enum value for the failure actually detected.
Traces: appconfigdata-error-invalid-configuration-token#task#1 (also appconfigdata-session-and-poll-error-envelopes#task#5).

=== Cause 2 of 5: StartConfigurationSession CREATES a session for an application that does not exist ===
AWS: the three identifiers are required and are resolved against the appconfig control plane; a nonexistent ApplicationIdentifier is refused with ResourceNotFoundException, HTTP 404 (Smithy: StartConfigurationSession errors include ResourceNotFoundException httpError 404, members Message, ResourceType -> ResourceType enum {Application, ConfigurationProfile, Deployment, Environment, Configuration}, ReferencedBy -> StringMap mapping the request parameters that referenced the missing resource). The recording shows ResourceType='Application', ReferencedBy={'ApplicationIdentifier': '<the bogus id>'}, message example 'Application not found', and header x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.appconfigdata/'.
Emulator: the call SUCCEEDS with HTTP 201 and hands back an InitialConfigurationToken - no 404, no x-amzn-errortype header, no Message/ResourceType/ReferencedBy.
Why: the emulator mints a token for a hierarchy that does not exist, so the failure surfaces later (or never) instead of at session start. A client testing its not-found handling sees success, and any negative test against a bogus application passes in the emulator and fails against AWS.
Fix: StartConfigurationSession's handler must resolve ApplicationIdentifier, EnvironmentIdentifier and ConfigurationProfileIdentifier in appconfig state before creating a session and refuse an unresolvable one with the modeled ResourceNotFoundException (404), filling ResourceType with the modeled enum value for the missing level and ReferencedBy with the request member that named it.
Traces: appconfigdata-error-unknown-application#task#1 (also appconfigdata-session-and-poll-error-envelopes#task#1).

=== Cause 3 of 5: GetLatestConfiguration answers 404 instead of serving the deployed configuration - the service's happy path ===
AWS: after a configuration version is deployed, GetLatestConfiguration with the session's token returns HTTP 200 with the configuration bytes as the HTTP payload plus the poll headers (Smithy: output members Configuration @httpPayload (blob), NextPollConfigurationToken @httpHeader Next-Poll-Configuration-Token, NextPollIntervalInSeconds @httpHeader Next-Poll-Interval-In-Seconds, ContentType @httpHeader Content-Type, VersionLabel @httpHeader Version-Label; http code 200).
Emulator: the poll answers HTTP 404 with no body and none of the poll headers - the recorded 200 carried the deployed document (example body {"probe": "appconfigdata", "revision": 1}), Next-Poll-Interval-In-Seconds, a Next-Poll-Configuration-Token and Version-Label. So the emulator serves no configuration at all: the single operation the service exists for never succeeds.
Why: this is a total failure of the data plane. A client that starts a session and polls gets a not-found instead of its configuration, so nothing downstream of AppConfigData can be exercised; and because no NextPollConfigurationToken is returned, the poll chain cannot even continue.
Hypothesis: the session created in Cause 2 is not bound to real appconfig deployment state, so the poll finds no deployed version to serve; the fixture's setup also hits the two already-filed appconfig defects (deployment documents report State 'DEPLOYING' instead of 'COMPLETE' - see open issue 20261023T145538Z-appconfig-deployment-documents-use-epoch-timestamps-omit-modeled.md), and a deployment that never reaches a completed state would leave the data plane with nothing to serve. Not directly observed; the 404 and the missing headers are.
Fix: the GetLatestConfiguration handler plus the session state it reads - resolve the token to its session, look up the latest completed deployment for that application/environment/profile in appconfig state, and serve the stored content as the httpPayload with ContentType, VersionLabel, a fresh NextPollConfigurationToken and NextPollIntervalInSeconds.
Traces: appconfigdata-poll-interval-enforcement#task#8 (also appconfigdata-session-and-latest-configuration#task#8).

=== Cause 4 of 5: StartConfigurationSession's 400 for a missing required member carries no Message ===
AWS: ApplicationIdentifier, EnvironmentIdentifier and ConfigurationProfileIdentifier are @required (each target Identifier, @length 1..128), and omitting one is refused with BadRequestException, HTTP 400, whose message is the standard request-derived validation text naming the member and the constraint - recorded example: "1 validation error detected: Value null at 'configurationProfileIdentifier' failed to satisfy constraint: Member must not be null". Because that text is built mechanically from the request's member name and the violated constraint, the rule - not the sentence - is the requirement.
Emulator: the status 400 is right but $.Message is ABSENT from the body, for all three omitted-member variants replayed.
Why: a client gets a 400 with nothing saying which member was wrong. SDK error rendering, logs and any assertion on the message see an empty refusal, so a caller cannot tell a missing application identifier from a missing profile identifier.
Fix: the appconfigdata required-member/constraint validation layer - emit BadRequestException with the 'N validation error detected: Value <v> at '<memberName>' failed to satisfy constraint: ...' message built from the request, for every required member and every @length violation on this input.
Traces: appconfigdata-session-and-poll-error-envelopes#task#2 (cluster also covers #task#3 and #task#4, the other two omitted identifiers).

=== Cause 5 of 5: GetLatestConfiguration with NO configuration_token answers 501 NotImplemented instead of a modeled 400 ===
AWS: ConfigurationToken is @required and bound to the query string (Smithy: GetLatestConfiguration input ConfigurationToken @httpQuery configuration_token, @required, @pattern ^\\S{1,8192}$), so a GET /configuration with no configuration_token parameter is a client error: BadRequestException, HTTP 400, header x-amzn-errortype 'BadRequestException:http://internal.amazon.com/coral/com.amazon.appconfigdata/' (recorded message example: 'ConfigurationToken must be provided. Use the StartConfigurationSession API to obtain an InitialConfigurationToken').
Emulator: the request answers HTTP 501 with x-amzn-errortype 'NotImplemented' and no Message. This is not a scaffold stub for the operation (the replay reports no stub_path and the same operation answers 400/404 on other steps) - the tokenless request simply does not match the route, so it falls through to the unimplemented handler.
Why: 501 is a server-fault class an SDK does not treat as a client error, so a caller that forgot the token (or whose token was dropped) gets 'this service does not exist' instead of a retryable/actionable 400, and no retry or validation logic keyed on BadRequestException fires.
Fix: route GET /configuration without the configuration_token query parameter into GetLatestConfiguration's input binding and let required-member validation refuse it with the modeled BadRequestException (400) - the same validation layer as Cause 4; the operation must never reach the not-implemented fallback.
Traces: appconfigdata-session-and-poll-error-envelopes#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200; 5 fixtures replayed, 36 steps, 12 passed, 22 mismatched, 2 blocked, state_reset_failures=0, so none of these ran against a stale fixture's state).

Cause 1 - appconfigdata-error-invalid-configuration-token#task#1, GetLatestConfiguration, verdict mismatch, expected 400 / actual 400:
  $.Details.InvalidParameters.ConfigurationToken: missing (expected {'Problem': 'Corrupted'}, got None)
  $.Message: value_mismatch (expected 'Token not valid', got 'The configuration token is not valid for any configuration session.')
  cluster also contains appconfigdata-session-and-poll-error-envelopes#task#5.

Cause 2 - appconfigdata-error-unknown-application#task#1, StartConfigurationSession, verdict mismatch, expected 404 / actual 201:
  status: expected 404, got 201
  $.Message: missing (expected 'Application not found', got None)
  $.ReferencedBy: missing (expected {'ApplicationIdentifier': 'zzzzzzz'}, got None)
  $.ResourceType: missing (expected 'Application', got None)
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.appconfigdata/', got None)
  cluster also contains appconfigdata-session-and-poll-error-envelopes#task#1.

Cause 3 - appconfigdata-poll-interval-enforcement#task#8, GetLatestConfiguration, verdict mismatch, expected 200 / actual 404:
  status: expected 200, got 404
  $.probe: missing (expected 'appconfigdata', got None)
  $.revision: missing (expected 1, got None)
  $headers.next-poll-interval-in-seconds: missing (expected '60', got None)
  $headers.next-poll-configuration-token: missing (expected '<shaped-not-asserted>', got None)
  $headers.version-label: missing (expected a version label, got None)
  cluster also contains appconfigdata-session-and-latest-configuration#task#8.

Cause 4 - appconfigdata-session-and-poll-error-envelopes#task#2, StartConfigurationSession, verdict mismatch, expected 400 / actual 400:
  $.Message: missing (expected "1 validation error detected: Value null at 'configurationProfileIdentifier' failed to satisfy constraint: Member must not be null", got None)
  cluster also contains #task#3 and #task#4 (the other two omitted required identifiers).

Cause 5 - appconfigdata-session-and-poll-error-envelopes#task#6, GetLatestConfiguration, verdict mismatch, expected 400 / actual 501:
  status: expected 400, got 501
  $.Message: missing (expected 'ConfigurationToken must be provided. Use the StartConfigurationSession API to obtain an InitialConfigurationToken', got None)
  $headers.x-amzn-errortype: shape_mismatch (expected 'BadRequestException:http://internal.amazon.com/coral/com.amazon.appconfigdata/', got 'NotImplemented')

No open issue owns any of these five replay keys (the replay reports owned_by=[] and stub_path='' for each). The open appconfig issues 20261023T145538Z-appconfig-deployment-documents-use-epoch-timestamps-omit-modeled.md and 20261023T145538Z-appconfig-environmentstate-is-served-as-the-enum-member.md own the control-plane setup divergences in the same fixtures, but neither covers any appconfigdata response.

## Steps to Reproduce
Boot the emulator and point an appconfigdata client (and an appconfig client for setup) at its data listener; region and credentials are irrelevant.

Cause 1: start a session, then call get_latest_configuration(ConfigurationToken='<a token the emulator never issued>'). Status 400 is correct; assert the body carries Details.InvalidParameters.ConfigurationToken.Problem (one of Corrupted/Expired/PollIntervalNotSatisfied) and Reason='InvalidParameters'. Emulator returns neither. Replay appconfigdata-error-invalid-configuration-token#task#1.

Cause 2: start_configuration_session(ApplicationIdentifier='zzzzzzz', EnvironmentIdentifier='<any>', ConfigurationProfileIdentifier='<any>') with no such application. Expected ResourceNotFoundException / HTTP 404 with ResourceType='Application' and ReferencedBy={'ApplicationIdentifier': 'zzzzzzz'}; emulator returns 201 with an InitialConfigurationToken. Replay appconfigdata-error-unknown-application#task#1.

Cause 3: create an application, environment, configuration profile and hosted configuration version via appconfig, start a deployment, then start_configuration_session on that triple and get_latest_configuration with the InitialConfigurationToken. Expected HTTP 200 with the deployed content as the body plus Next-Poll-Interval-In-Seconds, Next-Poll-Configuration-Token and Version-Label headers; emulator returns 404 with no body or headers. Replay appconfigdata-poll-interval-enforcement#task#8.

Cause 4: POST /configurationsessions with a body omitting ConfigurationProfileIdentifier (then separately ApplicationIdentifier, then EnvironmentIdentifier). Expected HTTP 400 BadRequestException whose Message names the omitted member and its constraint; emulator returns 400 with no Message. Replay appconfigdata-session-and-poll-error-envelopes#task#2 (and #task#3, #task#4).

Cause 5: raw GET /configuration with no configuration_token query parameter. Expected HTTP 400 with x-amzn-errortype 'BadRequestException:http://internal.amazon.com/coral/com.amazon.appconfigdata/'; emulator returns 501 with x-amzn-errortype 'NotImplemented'. Replay appconfigdata-session-and-poll-error-envelopes#task#6.

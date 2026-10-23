Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs.import-playback-key-pair-duplicate-conflict-rejected ivs.import-playback-key-pair-duplicate-rejected ivs.import-playback-key-pair-round-trip ivs.import-playback-key-pair-wrong-key-type-rejected ivs.list-playback-key-pairs-reflects-imported ivs.playback-key-pair-fingerprint-format ivs.native-rtmps-live-stream-lifecycle ivs.start-viewer-session-revocation-ignores-broadcast-state ivs.insert-ad-break-channel-not-broadcasting ivs.channel-not-broadcasting-across-operations ivs.create-channel-missing-permission-rejected ivs.create-recording-configuration-invalid-bucket-create-failed ivs.create-recording-configuration-custom-rendition-empty-list-rejected ivs.list-tags-for-resource-not-found ivs.rtmps-ingest-live-stream-lifecycle ivs.rtmps-ingest-native-protocol-round-trip
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs.import-playback-key-pair-duplicate-conflict-rejected|run#10|AssertionError|assert error["Message"] == (
Issue-Key: ivs.import-playback-key-pair-duplicate-rejected|run#13|AssertionError|assert error["Message"] == (
Issue-Key: ivs.import-playback-key-pair-round-trip|run#20|AssertionError|assert error["Message"] == f"Resource: {arn} not found", repr(error["Message"])
Issue-Key: ivs.import-playback-key-pair-wrong-key-type-rejected|run#9|AssertionError|assert error["Message"] == (
Issue-Key: ivs.list-playback-key-pairs-reflects-imported|run#18|AssertionError|assert get_error["Message"] == f"Resource: {arn} not found", repr(get_error["Message"])
Issue-Key: ivs|GetPlaybackKeyPair|ResourceNotFoundException
Issue-Key: ivs.native-rtmps-live-stream-lifecycle|run#15|AssertionError|assert msg == offline_msg, repr(msg)
Issue-Key: ivs.start-viewer-session-revocation-ignores-broadcast-state|run#8|AssertionError|assert before["Message"] == offline_msg, repr(before["Message"])
Issue-Key: ivs.insert-ad-break-channel-not-broadcasting|run#8|AssertionError|assert error["Code"] == "ChannelNotBroadcasting", (
Issue-Key: ivs.channel-not-broadcasting-across-operations|run#10|AssertionError|assert code == "ChannelNotBroadcasting", (
Issue-Key: ivs.create-channel-missing-permission-rejected|_client_error#13|AssertionError|raise AssertionError(
Issue-Key: ivs.create-recording-configuration-invalid-bucket-create-failed|_client_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: ivs.create-recording-configuration-custom-rendition-empty-list-rejected|run#6|AssertionError|assert error["Message"] == (
Issue-Key: ivs|ListTagsForResource|NotFoundException
Issue-Key: ivs.rtmps-ingest-live-stream-lifecycle|_poll_until_live#16|Timeout|time.sleep(interval_s)
Issue-Key: ivs.rtmps-ingest-native-protocol-round-trip|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# IVS: 7 behavioural divergences — error messages dropped on the wire, InsertAdBreak unimplemented, no IAM denial, missing S3-bucket and tagging resolution, no RTMPS ingest data plane

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStream

## Description
Seven distinct root causes found by replaying the behavioural suite against the IVS emulator (service token `ivs`, crate `services/service-com-amazonaws-ivs-amazoninteractivevideoservice`). They are bundled because they all belong to one service; each must be closed separately. The one open IVS issue (20261022T071923Z-ivs-createplaybackrestrictionpolicy-stub.md, CreatePlaybackRestrictionPolicy stub) covers none of them, and the auto-filed stub issues for ListAdConfigurations / DeleteAdConfiguration / DeleteRecordingConfiguration / CreatePlaybackRestrictionPolicy are excluded here.

=== Cause 1 of 7: modelled error bodies carry the message under `exceptionMessage`, so every SDK sees an EMPTY error message ===
Failing checks: ivs.import-playback-key-pair-duplicate-conflict-rejected, ivs.import-playback-key-pair-duplicate-rejected, ivs.import-playback-key-pair-round-trip, ivs.import-playback-key-pair-wrong-key-type-rejected, ivs.list-playback-key-pairs-reflects-imported, ivs.playback-key-pair-fingerprint-format, ivs.native-rtmps-live-stream-lifecycle, ivs.start-viewer-session-revocation-ignores-broadcast-state.
AWS: Smithy (com.amazonaws.ivs) — every error shape (ChannelNotBroadcasting @httpError 404, ResourceNotFoundException 404, ConflictException 409, ValidationException 400) names its detail member `exceptionMessage` targeting `errorMessage`, with NO @jsonName. Recorded behavior only for the wire: the recorded IVS responses arrive with the SDK's `Error.Message` populated, i.e. the body AWS really sends carries the message under the `message` key (botocore reads `message`/`Message` and reports '' when neither is present). The recorded texts, each built from the request or fixed per rule, are: ResourceNotFoundException -> "Resource: <arn> not found"; ChannelNotBroadcasting -> "Channel: <channelArn> is not currently online"; ImportPlaybackKeyPair ConflictException on duplicate material -> "A playback-key resource already exists with the given publicKeyMaterial"; ImportPlaybackKeyPair ValidationException on non-EC/undecodable material -> "Request member: publicKeyMaterial failed to satisfy the constraints: could not decode as a PEM file".
Emulator: the HTTP status and error code are correct, but `Error.Message` is the empty string on every one of these failures. Handlers return `Err(<Op>ErrorModeled::X(X { exception_message: Some(..), .. }).into())`, which lands in `Scaffolded::Modeled` and renders the body as `{"exceptionMessage": …}` — a key no SDK error parser reads. The service's own `src/scaffolded.rs` documents this and provides `Scaffolded::stated(modeled, message)` / `crate::state::validation(..)` for exactly this reason (handlers that use those DO surface a message). The handler texts also differ in wording from the recorded AWS texts above (e.g. the conflict text is "a playback key pair with fingerprint … is already imported as …"), so fixing only the body key still diverges.
Why: a client cannot show or parse the reason for any IVS failure — error-message assertions, log triage and any code keying off the message text all see an empty string.
Fix: route modelled failures through the `Scaffolded::stated` channel in every implemented IVS handler (and word them with the rules above) — the operation handlers plus the shared message rules in `crate::state`.
Traces: ivs.import-playback-key-pair-round-trip (GetPlaybackKeyPair after delete), ivs.native-rtmps-live-stream-lifecycle (GetStream before ingest); file/line citations: services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/get_stream.rs:177-193, .../src/ops/import_playback_key_pair.rs:172-203, .../src/scaffolded.rs:180-219 and 336-342.

=== Cause 2 of 7: InsertAdBreak is an unimplemented scaffold stub (HTTP 501 NotImplemented) ===
Failing checks: ivs.insert-ad-break-channel-not-broadcasting, ivs.channel-not-broadcasting-across-operations.
AWS: Smithy InsertAdBreak(channelArn: ChannelArn required, durationSeconds: AdDurationSeconds range 1..300 required) -> adBreakId; modelled errors include ChannelNotBroadcasting (HTTP 404) "The stream is offline for the given channel ARN" and ValidationException (HTTP 400). Recorded behavior: the ad-configuration precondition is checked unconditionally first — a channel with no ad configuration gets 400 ValidationException with "Request member: channelArn failed to satisfy the constraints: valid ad configuration is required to be associated with the channel requested for the ad break with ARN: <channelArn>" whether the channel is idle or LIVE; with an ad configuration attached and no active stream it is 404 ChannelNotBroadcasting.
Emulator: answers HTTP 501 with code `NotImplemented` and message "operation InsertAdBreak is scaffolded but not implemented in this emulator".
Why: a client exercising SSAI ad insertion gets a non-retryable 501 and can never observe either the precondition failure or the broadcast-state failure; error-handling branches keyed on ValidationException/ChannelNotBroadcasting are dead.
Fix: implement the handler (its precondition check against the channel's adConfigurationArn, then broadcast state from the stream store).
Traces: ivs.insert-ad-break-channel-not-broadcasting; services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/insert_ad_break.rs:175 (the `Scaffolded::not_implemented` return).

=== Cause 3 of 7: CreateChannel accepts a caller with no IVS permissions (no AccessDeniedException) ===
Failing check: ivs.create-channel-missing-permission-rejected.
AWS: Smithy CreateChannel models AccessDeniedException (HTTP 403) "User does not have sufficient access to perform this action". Recorded behavior: a caller using credentials from AssumeRole on a role with NO attached policy is refused with 403 AccessDeniedException and nothing is created; the message is built from the request principal, "User: <assumed-role-arn> is not authorized to perform: ivs:CreateChannel on resource: arn:aws:ivs:<region>:<account>:channel/* because no identity-based policy allows the ivs:CreateChannel action".
Emulator: returns HTTP 200 and creates the channel plus its stream key for the unpermissioned assumed-role credentials (observed response carried channel arn …:channel/35015f84159f and a stream key).
Why: an overly-permissive emulator lets authorization regressions pass locally — code paths that must handle AccessDenied are never exercised, and least-privilege policy mistakes are invisible until production.
Fix: authorization for the operation (the IVS handlers' request-principal check, or the shared auth layer the service mounts) must refuse an unpermissioned principal before CreateChannel mutates state.
Traces: ivs.create-channel-missing-permission-rejected; services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_channel.rs.

=== Cause 4 of 7: CreateRecordingConfiguration accepts a non-existent S3 bucket ===
Failing check: ivs.create-recording-configuration-invalid-bucket-create-failed.
AWS: Smithy CreateRecordingConfiguration models ValidationException (HTTP 400). Recorded behavior: a syntactically valid but non-existent/unowned destination bucket is refused synchronously with 400 ValidationException and nothing is created; the recorded message follows the service's validation template naming the member — "Request member: recordingConfiguration.destinationConfiguration.s3 failed to satisfy the constraints: recordingConfiguration.destinationConfiguration.s3 is invalid".
Emulator: returns HTTP 200 with state `CREATING` for bucket `sim2real-ivs-missing-bucket-…` and persists the configuration; the handler never consults S3.
Why: a client that mistypes or forgets to create its recording bucket gets a successful create locally and silently loses recordings, instead of the up-front rejection real AWS gives.
Fix: validation in the operation's handler — resolve the destinationConfiguration.s3 bucket against the emulator's S3 state before minting the configuration.
Traces: ivs.create-recording-configuration-invalid-bucket-create-failed; services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_recording_configuration.rs:184-235.

=== Cause 5 of 7: CUSTOM-rendition ValidationException message names an internal snake_case member path ===
Failing check: ivs.create-recording-configuration-custom-rendition-empty-list-rejected.
AWS: Smithy RenditionConfiguration documents "If CUSTOM is specified, a set of renditions must be specified"; the refusal is ValidationException (HTTP 400). Recorded behavior: the message follows the service's fixed template over the WIRE member path — "Request member: recordingConfiguration.renditionConfiguration failed to satisfy the constraints: recordingConfiguration.renditionConfiguration is invalid". That template (Request member: <wire path> failed to satisfy the constraints: …) is the known rule here, so the member path is a requirement, not an example.
Emulator: correct status/code (400 ValidationException) but the message is "Request member: rendition_configuration.renditions failed to satisfy the constraints: invalid value for rendition configuration rendition selection and renditions" — it leaks the Rust snake_case field name and names the wrong member.
Why: a client parsing the member path out of IVS validation messages (a common pattern for mapping a rejection back to a form field) is pointed at a member name that does not exist in the API.
Fix: the cross-member validation message rule in `crate::state::check_rendition_configuration`, which must emit the wire member path.
Traces: ivs.create-recording-configuration-custom-rendition-empty-list-rejected; services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_recording_configuration.rs:186-193 and the rule in .../src/state.rs.

=== Cause 6 of 7: the tagging surface cannot resolve playback-key ARNs, so ListTagsForResource 404s on an EXISTING key pair ===
Failing check: ivs.list-tags-for-resource-not-found.
AWS: Smithy ListTagsForResource(resourceArn) -> tags; the tagging surface models its own NotFoundException (distinct from the service's ResourceNotFoundException). Recorded behavior: for a live resource it returns 200 with that resource's tags; only a fabricated or deleted ARN gets NotFoundException ("Resource was not found").
Emulator: ListTagsForResource on a playback key pair that was just created by an implemented ImportPlaybackKeyPair (tags {"probe":"sim2real"} accepted and stored) answers NotFoundException "Resource was not found". `crate::state::resource_tags` / `resource_tags_entry` match only "channel", "stream-key" and "recording-configuration" and fall through to `None` for "playback-key" (and for other ARN types the service now creates), so TagResource/UntagResource on a key pair are affected the same way.
Why: tags set at import are unreadable and unmodifiable through the tagging API; a client that tags IVS resources sees a resource it just created reported as non-existent.
Fix: the shared ARN->resource resolution in `crate::state` (resource_tags / resource_tags_entry) must cover every resource type the service can now create, starting with playback-key.
Traces: ivs.list-tags-for-resource-not-found; services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/state.rs:1182-1223, .../src/ops/list_tags_for_resource.rs:167-171.

=== Cause 7 of 7: the announced RTMPS ingest endpoint / HLS playback URL are not served, so a channel never goes LIVE ===
Failing checks: ivs.rtmps-ingest-live-stream-lifecycle, ivs.rtmps-ingest-native-protocol-round-trip (both "Check timed out after 60.0s").
AWS: recorded behavior only — IVS's data plane is RTMPS ingest plus HLS playback. CreateChannel returns an `ingestEndpoint` that accepts `rtmps://<ingestEndpoint>:443/app/<streamKey>` and a `playbackUrl` that serves an HLS master playlist (body starting `#EXTM3U`); once ingest is pushing, GetStream reports a stream with state `LIVE` (and streamId), PutMetadata succeeds, and StopStream forces the channel back to ChannelNotBroadcasting.
Emulator: CreateChannel hands out an ingestEndpoint (`<id>.global-contribute.live-video.net`) and a playbackUrl (`https://<id>.<region>.playback.live-video.net/api/video/v1/…m3u8`) that this emulator does not serve — it announces addresses other than the one it is reached on. A genuine ffmpeg RTMPS push therefore never produces a stream: GetStream keeps answering ChannelNotBroadcasting, the LIVE poll never converges and both checks hit the harness timeout with no LIVE state and no HLS manifest ever observed.
Why: every client of the IVS data plane (broadcaster SDKs, players, anything polling GetStream for LIVE) is untestable against the emulator: the control plane reports a channel that can never broadcast, and the published URLs point away from the emulator.
Fix: the service's data-plane listener plus the stream state it writes — the endpoint/URL minted by CreateChannel must address the emulator, an RTMPS ingest must create the LIVE Stream record in `crate::state` (streams map) that GetStream reads, and the playback URL must serve an HLS manifest.
Traces: ivs.rtmps-ingest-live-stream-lifecycle, ivs.rtmps-ingest-native-protocol-round-trip; services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_channel.rs (endpoint minting) and .../src/state.rs (streams / live_stream).

## Evidence
Suite: 51/79 passed (64.6%), ivs 51/79, 28 failed; 12 of those failures are already-filed scaffold stubs (ListAdConfigurations, CreatePlaybackRestrictionPolicy, DeleteAdConfiguration, DeleteRecordingConfiguration) and are excluded.

Cause 1 — expected vs actual error message:
  expected ConflictException message "A playback-key resource already exists with the given publicKeyMaterial"; actual Error.Message == '' (AssertionError: '') [ivs.import-playback-key-pair-duplicate-conflict-rejected, ivs.import-playback-key-pair-duplicate-rejected]
  expected ResourceNotFoundException message "Resource: <arn> not found"; actual '' [ivs.import-playback-key-pair-round-trip, ivs.list-playback-key-pairs-reflects-imported, ivs.playback-key-pair-fingerprint-format]
  expected ValidationException message "Request member: publicKeyMaterial failed to satisfy the constraints: could not decode as a PEM file"; actual '' [ivs.import-playback-key-pair-wrong-key-type-rejected]
  expected ChannelNotBroadcasting message "Channel: <channelArn> is not currently online"; actual '' [ivs.native-rtmps-live-stream-lifecycle, ivs.start-viewer-session-revocation-ignores-broadcast-state]
  Status and error code were correct in every one of these; only the body's message key diverges (`exceptionMessage` instead of `message`).

Cause 2 — InsertAdBreak:
  expected 400 ValidationException (no ad configuration) or 404 ChannelNotBroadcasting (ad configuration present, channel idle); actual HTTP 501, Code 'NotImplemented', Message 'operation InsertAdBreak is scaffolded but not implemented in this emulator'.
  "AssertionError: expected ChannelNotBroadcasting on a channel with no active stream, got 'NotImplemented'"; "AssertionError: InsertAdBreak on an idle channel returned 'NotImplemented', expected ChannelNotBroadcasting".

Cause 3 — CreateChannel without permissions:
  expected 403 AccessDeniedException and nothing created; actual HTTPStatusCode 200 with {'channel': {'arn': 'arn:aws:ivs:us-west-2:000000000000:channel/35015f84159f', 'name': 'sim2real-ivs-noperm-chan-…', …}, 'streamKey': {…}} — "AssertionError: expected the call to be rejected with AccessDeniedException, but it succeeded".

Cause 4 — CreateRecordingConfiguration with a missing bucket:
  expected 400 ValidationException, message "Request member: recordingConfiguration.destinationConfiguration.s3 failed to satisfy the constraints: recordingConfiguration.destinationConfiguration.s3 is invalid", and no configuration created; actual HTTPStatusCode 200, {'recordingConfiguration': {'arn': '…:recording-configuration/cb9f3353304b', 'destinationConfiguration': {'s3': {'bucketName': 'sim2real-ivs-missing-bucket-a6d45408467c'}}, 'state': 'CREATING'}}.

Cause 5 — CUSTOM rendition with an empty renditions list:
  expected 400 ValidationException "Request member: recordingConfiguration.renditionConfiguration failed to satisfy the constraints: recordingConfiguration.renditionConfiguration is invalid"; actual 400 ValidationException 'Request member: rendition_configuration.renditions failed to satisfy the constraints: invalid value for rendition configuration rendition selection and renditions'.

Cause 6 — ListTagsForResource on an existing playback key pair:
  expected 200 with {'probe': 'sim2real'}; actual ClientError "An error occurred (NotFoundException) when calling the ListTagsForResource operation: Resource was not found" (uncaught, check errored). state.rs resource_tags matches only channel / stream-key / recording-configuration.

Cause 7 — RTMPS ingest / HLS playback:
  expected GetStream to report state 'LIVE' within 90s of an ffmpeg RTMPS push to rtmps://<ingestEndpoint>:443/app/<streamKey>, and playbackUrl to serve a body starting '#EXTM3U'; actual no LIVE state and no manifest ever observed — both checks ended with "Error: Check timed out after 60.0s". ffmpeg was present (the checks raise explicitly when it is not).

## Steps to Reproduce
Boot the emulator from this checkout and point an AWS SDK (botocore) at it with region us-west-2, then:

Cause 1: ivs.create_channel(name=…); ivs.get_stream(channelArn=<arn>) -> inspect ClientError response['Error']: Code 'ChannelNotBroadcasting' but Message ''. Same for ivs.get_playback_key_pair on a deleted ARN (ResourceNotFoundException) and for a duplicate ivs.import_playback_key_pair of the same EC public key PEM (ConflictException). Inspect the raw body: the message is under 'exceptionMessage'.
Cause 2: ivs.insert_ad_break(channelArn=<idle channel arn>, durationSeconds=30) -> HTTP 501 NotImplemented instead of ValidationException / ChannelNotBroadcasting.
Cause 3: iam.create_role(...) with a trust policy and NO attached permission policy; sts.assume_role(...); build an ivs client from those temporary credentials; create_channel(name=…) -> succeeds with 200 instead of AccessDeniedException.
Cause 4: ivs.create_recording_configuration(name=…, destinationConfiguration={'s3': {'bucketName': '<a bucket that was never created>'}}) -> 200 with state CREATING instead of 400 ValidationException.
Cause 5: create an S3 bucket, then ivs.create_recording_configuration(name=…, destinationConfiguration={'s3':{'bucketName':bucket}}, renditionConfiguration={'renditionSelection':'CUSTOM','renditions':[]}) -> read the ValidationException message.
Cause 6: ivs.import_playback_key_pair(publicKeyMaterial=<EC P-384 PUBLIC KEY PEM>, name=…, tags={'probe':'sim2real'}); then ivs.list_tags_for_resource(resourceArn=<returned keyPair arn>) -> NotFoundException instead of the tags.
Cause 7: ivs.create_channel(...); push with `ffmpeg -re -f lavfi -i testsrc -f lavfi -i sine -vcodec libx264 -acodec aac -f flv rtmps://<ingestEndpoint>:443/app/<streamKey>`; poll ivs.get_stream(channelArn=…) -> never reaches LIVE; GET the channel's playbackUrl -> no HLS manifest.

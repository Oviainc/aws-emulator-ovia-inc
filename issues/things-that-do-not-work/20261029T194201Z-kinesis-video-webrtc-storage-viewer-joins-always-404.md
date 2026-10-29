Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-webrtc-storage.join-storage-session-as-viewer-denied-for-wrong-channel-arn-scope kinesis-video-webrtc-storage.viewer-join-before-master-then-master-join-succeeds kinesis-video-webrtc-storage.viewer-rejoin-same-client-id-does-not-consume-new-slot kinesis-video-webrtc-storage.viewer-slot-limit-enforced-sequential kinesis-video-webrtc-storage.viewer-slot-limit-enforced-concurrent kinesis-video-webrtc-storage.viewer-slot-limit-independent-per-channel kinesis-video-webrtc-storage.join-storage-session-access-denied-without-permission kinesis-video-webrtc-storage.delete-signaling-channel-while-linked-rejected kinesis-video-webrtc-storage.join-storage-session-requires-media-storage-enabled-rejected kinesis-video-webrtc-storage.join-storage-session-as-viewer-requires-media-storage-enabled-rejected kinesis-video-webrtc-storage.update-media-storage-configuration-stream-already-mapped-rejected kinesis-video-webrtc-storage.update-media-storage-configuration-zero-retention-rejected kinesis-video-webrtc-storage.join-storage-session-malformed-channel-arn-rejected kinesis-video-webrtc-storage.webrtc-signaling-sdp-offer-round-trip kinesis-video-webrtc-storage.join-storage-session-repeated-calls-not-deduplicated
Issue-Shape-Id: com.amazonaws.kinesisvideowebrtcstorage#AWSAcuityRoutingServiceLambda
Issue-Key: kinesis-video-webrtc-storage|JoinStorageSessionAsViewer|ResourceNotFoundException
Issue-Key: kinesis-video-webrtc-storage.viewer-slot-limit-enforced-concurrent|run#10|AssertionError|assert 1 <= len(successes) <= 3, (
Issue-Key: kinesis-video-webrtc-storage.join-storage-session-access-denied-without-permission|_client_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: kinesis-video-webrtc-storage.delete-signaling-channel-while-linked-rejected|_client_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: kinesis-video-webrtc-storage.join-storage-session-requires-media-storage-enabled-rejected|run#7|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: kinesis-video-webrtc-storage.join-storage-session-as-viewer-requires-media-storage-enabled-rejected|run#8|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: kinesis-video-webrtc-storage.update-media-storage-configuration-stream-already-mapped-rejected|run#7|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: kinesis-video-webrtc-storage.update-media-storage-configuration-zero-retention-rejected|run#6|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: kinesis-video-webrtc-storage.join-storage-session-malformed-channel-arn-rejected|run#7|AssertionError|assert error.get("Code") == "BadRequestException", error
Issue-Key: kinesis-video-webrtc-storage.webrtc-signaling-sdp-offer-round-trip|run#12|InvalidStatus|with ws_connect(target_url, additional_headers=route_headers, ssl=tls, open_timeout=20) as ws:
Issue-Key: kinesis-video-webrtc-storage.join-storage-session-repeated-calls-not-deduplicated|run#11|InvalidStatus|with ws_connect(target_url, additional_headers=route_headers, ssl=tls, open_timeout=20) as ws:
Issue-Commit: 954781cef49baf7406aef764139782df619c6255

# Kinesis Video WebRTC Storage: viewer joins always 404, JoinStorageSession unauthorized/unguarded, WSS signaling unserved, and five wrong refusal messages

> _Note: this issue was identified against the code base at commit `954781cef49baf7406aef764139782df619c6255`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
JoinStorageSessionAsViewer

## Description
This issue bundles EIGHT distinct root causes, all in the kinesis-video-webrtc-storage behavioral surface (operations JoinStorageSessionAsViewer, JoinStorageSession, and the Kinesis Video Streams control operations those checks drive: DeleteSignalingChannel, UpdateMediaStorageConfiguration, GetSignalingChannelEndpoint, plus the native WSS signaling route). All eight must be closed before submitting; each cause below names its own failing check ids.

=== Cause 1 of 8: JoinStorageSessionAsViewer resolves the channel only against the WebRTC-storage crate's private registry, so a viewer can never initiate a session ===

AWS: Smithy (services/service-com-amazonaws-kinesisvideowebrtcstorage-awsacuityroutingservicelambda, sdkId 'Kinesis Video WebRTC Storage', restJson1, sigv4): JoinStorageSessionAsViewer POST /joinStorageSessionAsViewer, input channelArn (ChannelArn, @required, @pattern ^arn:(aws[a-zA-Z-]*):kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$) and clientId (@required, @length 1..256, @pattern ^[a-zA-Z0-9_.-]+$); output Unit with HTTP 200; errors AccessDeniedException(403), ClientLimitExceededException(400), InvalidArgumentException(400), ResourceNotFoundException(404). The operation's own model documentation states: "If there's no existing session for the channel, create a new streaming session and provide the Amazon Resource Name (ARN) of the signaling channel (channelArn) and client id (clientId)" and "Both a video producing device and viewers can join a session first and wait for other participants." So a viewer join against a signaling channel that EXISTS (created with kinesisvideo CreateSignalingChannel and linked with UpdateMediaStorageConfiguration Status=ENABLED) must return 200 with an empty body even when no master has ever joined; ResourceNotFoundException is reserved for a channelArn that names no signaling channel at all.

Emulator: every viewer join of a freshly created, storage-enabled channel is refused HTTP 404 ResourceNotFoundException with message 'No signaling channel '<channel-name>' found in account <account-id> for region <region>' (fixed text: "No signaling channel '", "' found in account ", " for region "; variable parts: the channel name, account id and region parsed out of the request's channelArn). The handler gates on a registration held in the WebRTC-storage crate's OWN store and that store is only ever populated by JoinStorageSession's producer join — it is a different registry from the one kinesisvideo CreateSignalingChannel writes to, so a channel that demonstrably exists on the control plane is invisible here. This single refusal is what fails all six viewer checks: the three slot-limit checks, the per-channel independence check and the rejoin check never reach their ClientLimitExceededException assertion at all, and the IAM-ARN-scope check never reaches its AccessDeniedException assertion.

Why: a client gets 404 for a resource it just created and read back as ACTIVE. The entire viewer half of WebRTC ingestion is unusable: viewer-initiated ingestion (the documented Multiviewer behaviour) is impossible, and because the refusal is ResourceNotFoundException a caller concludes the channel does not exist and will delete/recreate it instead of retrying.

Fix: the JoinStorageSessionAsViewer handler and this crate's state — services/service-com-amazonaws-kinesisvideowebrtcstorage-awsacuityroutingservicelambda/src/ops/join_storage_session_as_viewer.rs (the knows_channel guard) together with src/state.rs. Channel existence must be resolved against the shared Kinesis Video Streams signaling-channel registry that CreateSignalingChannel populates, and an absent session for an existing channel must be CREATED by the viewer join rather than refused. Note the viewer-quota and IAM behaviour asserted by the five dependent checks is only reachable once this guard is fixed, and must be re-verified then.

Traces: kinesis-video-webrtc-storage.viewer-join-before-master-then-master-join-succeeds, kinesis-video-webrtc-storage.viewer-rejoin-same-client-id-does-not-consume-new-slot, kinesis-video-webrtc-storage.viewer-slot-limit-enforced-sequential, kinesis-video-webrtc-storage.viewer-slot-limit-enforced-concurrent, kinesis-video-webrtc-storage.viewer-slot-limit-independent-per-channel, kinesis-video-webrtc-storage.join-storage-session-as-viewer-denied-for-wrong-channel-arn-scope.

=== Cause 2 of 8: JoinStorageSession performs no authorization — an explicitly denied caller is served 200 ===

AWS: Smithy: JoinStorageSession models AccessDeniedException with httpError 403, documented "You do not have required permissions to perform this operation." A caller whose identity-based policy carries {"Effect":"Deny","Action":"kinesisvideo:JoinStorageSession","Resource":"*"} is refused 403 AccessDeniedException with the standard IAM explicit-deny message. Fixed text of that message: "User: ", " is not authorized to perform: ", " on resource: ", " with an explicit deny in an identity-based policy". Variable parts, taken from the request's credentials and body: the caller's assumed-role principal ARN (arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>), the action name kinesisvideo:JoinStorageSession, and the channelArn from the request.

Emulator: the denied caller is answered HTTP 200 with a zero-length body (content-length: 0) — the call succeeds and joins the storage session.

Why: an authorization bypass. Least-privilege policies cannot be tested against the emulator: a deny that would stop ingestion on AWS is silently ignored, so a policy regression ships green. The same gap applies to the ARN-scoped implicit-deny class asserted by the viewer check in Cause 1 (message fixed text "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action").

Fix: the authorization step of the JoinStorageSession handler (services/service-com-amazonaws-kinesisvideowebrtcstorage-awsacuityroutingservicelambda/src/ops/join_storage_session.rs — handle() goes straight from ARN parsing to join_as_producer and never consults the caller's policy), and the equivalent step in join_storage_session_as_viewer.rs.

Traces: kinesis-video-webrtc-storage.join-storage-session-access-denied-without-permission, kinesis-video-webrtc-storage.join-storage-session-as-viewer-denied-for-wrong-channel-arn-scope.

=== Cause 3 of 8: DeleteSignalingChannel deletes a channel whose MediaStorageConfiguration is still ENABLED ===

AWS: Smithy (kinesisvideo): DeleteSignalingChannel POST /deleteSignalingChannel models ResourceInUseException with httpError 400; the shape's documentation covers exactly this class ("When the input StreamARN or ChannelARN in CLOUD_STORAGE_MODE is already mapped to a different Kinesis Video Stream resource, or if the provided input ... is not in Active status"). Recorded behavior: deleting a signaling channel that is still linked to a stream is refused HTTP 400 ResourceInUseException with the message "Disable MediaStorageConfiguration to delete the channel" — that message is entirely fixed text with no part taken from the request or state — and the channel remains ACTIVE. After UpdateMediaStorageConfiguration sets Status=DISABLED the identical delete succeeds.

Emulator: the delete of a still-linked channel is answered HTTP 200 with body '{}' and the channel is actually removed.

Why: silently destructive and overly permissive. A channel that is ingesting into a stream can be torn down by a call AWS would refuse, so an operator/IaC flow that relies on the refusal as a safety interlock destroys a live ingestion path, and the stream is left orphaned.

Fix: the DeleteSignalingChannel handler's pre-delete validation in the kinesisvideo service crate (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/delete_signaling_channel.rs) — it must consult the channel's MediaStorageConfiguration and refuse with ResourceInUseException while Status is ENABLED.

Traces: kinesis-video-webrtc-storage.delete-signaling-channel-while-linked-rejected.

=== Cause 4 of 8: wrong InvalidArgumentException message for GetSignalingChannelEndpoint Protocols=[WEBRTC] without media storage ===

AWS: Smithy (kinesisvideo): GetSignalingChannelEndpoint models InvalidArgumentException with httpError 400 ("The value for this input parameter is invalid."). Recorded behavior: requesting the WEBRTC protocol endpoint for a SINGLE_MASTER channel that has no MediaStorageConfiguration enabling storage is refused 400 InvalidArgumentException with the message "MediaStorageConfiguration is required for WEBRTC protocol" — entirely fixed text, nothing interpolated from the request. The emulator already returns the correct code and status; only the message diverges.

Emulator: 400 InvalidArgumentException with the message "The WEBRTC protocol is available only for a signaling channel configured for ingestion. Use UpdateMediaStorageConfiguration to configure it."

Why: clients and test suites that assert on the refusal text (and operators reading logs) see a message AWS never emits; the message is a requirement of the error class, so this is a fidelity defect even though the code matches.

Fix: the message construction in the kinesisvideo GetSignalingChannelEndpoint handler's validation path (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/get_signaling_channel_endpoint.rs).

Traces: kinesis-video-webrtc-storage.join-storage-session-requires-media-storage-enabled-rejected, kinesis-video-webrtc-storage.join-storage-session-as-viewer-requires-media-storage-enabled-rejected.

=== Cause 5 of 8: wrong ResourceInUseException message for UpdateMediaStorageConfiguration when the stream is already mapped elsewhere ===

AWS: Smithy (kinesisvideo): UpdateMediaStorageConfiguration models ResourceInUseException with httpError 400 for exactly this class. Recorded behavior: linking a stream that is already mapped to a DIFFERENT signaling channel is refused 400 ResourceInUseException with the message "The channel/stream is currently not available for this operation because it is being updated." — entirely fixed text; AWS does NOT interpolate the stream or channel ARN into this message. The target channel stays DISABLED, and the identical call succeeds once the holding channel releases the stream.

Emulator: correct code, but message "The stream <stream-arn> is already mapped to <channel-arn>." — i.e. it interpolates the request's StreamARN and the holding channel's ARN from state into text AWS does not use.

Why: the message is part of the error-class contract; a caller matching the documented text does not recognise the refusal, and the emulator additionally leaks state (the holding channel's ARN) that AWS does not disclose in this fault.

Fix: the message construction in the UpdateMediaStorageConfiguration handler's already-mapped validation branch (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/update_media_storage_configuration.rs).

Traces: kinesis-video-webrtc-storage.update-media-storage-configuration-stream-already-mapped-rejected.

=== Cause 6 of 8: wrong NoDataRetentionException message — the Smithy shape's documentation string is being served as the runtime message ===

AWS: Smithy (kinesisvideo): UpdateMediaStorageConfiguration models NoDataRetentionException, httpError 400. Recorded behavior: enabling storage against a stream created with DataRetentionInHours=0 is refused 400 NoDataRetentionException with the message "The stream data retention is set to 0 hours." — entirely fixed text, no request or state value interpolated.

Emulator: correct code, but message "The Stream data retention in hours is equal to zero.", which is verbatim the DOCUMENTATION of com.amazonaws.kinesisvideo#NoDataRetentionException. The handler is emitting the shape's doc string as the error message instead of the message AWS returns; the same substitution should be audited for the other error shapes in this crate.

Why: the message differs from AWS's wording for this error class, so callers (and the behavioral contract) see text AWS never emits; and because the defect is a systematic "doc string as message" substitution it is likely to recur on other faults of the same service.

Fix: the message construction in the UpdateMediaStorageConfiguration handler's zero-retention branch (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/update_media_storage_configuration.rs), and the general rule that error messages are not taken from shape documentation.

Traces: kinesis-video-webrtc-storage.update-media-storage-configuration-zero-retention-rejected.

=== Cause 7 of 8: a malformed channelArn on JoinStorageSession is refused as ValidationException instead of BadRequestException ===

AWS: recorded behavior only — the WebRTC-storage Smithy model declares no BadRequestException (its common errors are AccessDeniedException, ClientLimitExceededException, InvalidArgumentException, ResourceNotFoundException), so this is the service front door's body-validation refusal rather than a modeled operation error. Recorded: POST /joinStorageSession with channelArn that fails the modeled @pattern is answered HTTP 400 with error code BadRequestException and the message "Invalid request body" — entirely fixed text; AWS does NOT echo the offending value, the member name or the regex.

Emulator: HTTP 400 with code ValidationException and the message "1 validation error detected: Value '<offending-value>' at 'channelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z-]*):kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$" — the generic constraint-violation envelope, with the member name, the submitted value and the pattern interpolated.

Why: the error CODE is what clients branch on. A caller catching BadRequestException (the only code this service emits for a malformed body) sees an unmodelled ValidationException it has no handler for and treats a permanent 400 as an unknown fault; retry/classification logic and error-code assertions both break.

Fix: the constraint-violation serializer / front-door validation refusal for this service — the restJson1 pattern/length rejection path must answer this service's BadRequestException envelope with the fixed message "Invalid request body" rather than the shared ValidationException envelope.

Traces: kinesis-video-webrtc-storage.join-storage-session-malformed-channel-arn-rejected.

=== Cause 8 of 8: the WSS signaling endpoint the emulator advertises 404s the WebSocket upgrade, so JoinStorageSession never pushes an SDP_OFFER ===

AWS: recorded behavior only — the WSS signaling wire protocol is the native Kinesis Video WebRTC signaling protocol and is not described by any Smithy model; the model only states that GetSignalingChannelEndpoint(SingleMasterChannelEndpointConfiguration={Protocols:["WSS"],Role:"MASTER"}) returns a ResourceEndpointList entry whose ResourceEndpoint is a wss:// URL. Recorded: that advertised endpoint accepts a SigV4-presigned WebSocket upgrade for the channel (query parameter X-Amz-ChannelARN) with HTTP 101, and JoinStorageSession then pushes a signaling message with messageType SDP_OFFER and a base64 messagePayload down that connection — once per call, since the operation is documented as not idempotent (each call counts as a new API call and sends its own offer), so two successive calls must produce two independent SDP_OFFERs.

Emulator: the WebSocket upgrade against the endpoint the emulator itself just advertised is answered HTTP 404 (websockets.exceptions.InvalidStatus: server rejected WebSocket connection: HTTP 404). No signaling connection can be established, so no SDP_OFFER is ever pushed and nothing distinguishes the 200-with-empty-body JoinStorageSession from a stub that does nothing. Related but distinct: open issue 20261029T173603Z-kinesisvideo-front-door-answers-an-unroutable-operation-with.md concerns a bare 404 for an UNROUTABLE modeled operation on the kinesisvideo control-plane endpoint; this cause is the WSS upgrade route on the signaling endpoint GetSignalingChannelEndpoint hands back, and that issue neither names it nor carries these check ids.

Why: the whole media/signaling plane is unreachable. A client follows the emulator's own endpoint discovery and is told the endpoint does not exist, so no WebRTC ingestion can be exercised end to end, and a caller cannot tell a non-functional backend from a working one because the REST join still answers 200.

Fix: the signaling WSS route of the Kinesis Video signaling endpoint (the handler serving the host/path returned by GetSignalingChannelEndpoint for Protocols=[WSS]) must accept the presigned upgrade and complete the handshake, and the JoinStorageSession handler must push one SDP_OFFER signaling message per call onto the joined channel's connected MASTER sessions.

Traces: kinesis-video-webrtc-storage.webrtc-signaling-sdp-offer-round-trip, kinesis-video-webrtc-storage.join-storage-session-repeated-calls-not-deduplicated.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: kinesis-video-webrtc-storage 4/19 passed (21%), 15 failed (8 mismatched, 7 errored). No failing check was marked ALREADY FILED or OWNED by an existing issue.

Cause 1 — JoinStorageSessionAsViewer 404 on an existing channel.
Expected (model doc: a viewer may initiate the session; 200 Unit): HTTP 200, empty body.
Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the JoinStorageSessionAsViewer operation: No signaling channel 'sim2real-kvsrtc-ch-70071ad1db3d' found in account 000000000000 for region us-west-2 (same refusal with a different generated channel name in viewer-rejoin-same-client-id-does-not-consume-new-slot, viewer-slot-limit-enforced-sequential, viewer-slot-limit-independent-per-channel and join-storage-session-as-viewer-denied-for-wrong-channel-arn-scope). The concurrent variant reports it five times over: "expected between 1 and 3 (inclusive) of 5 concurrent viewer joins to succeed, got ['ResourceNotFoundException', 'ResourceNotFoundException', 'ResourceNotFoundException', 'ResourceNotFoundException', 'ResourceNotFoundException']". Source grounding: src/ops/join_storage_session_as_viewer.rs refuses when `!store.knows_channel(&input.channel_arn)`, a registry only src/ops/join_storage_session.rs's `store.join_as_producer(...)` ever writes.

Cause 2 — JoinStorageSession ignores an explicit IAM deny.
Expected: 403 AccessDeniedException, Message "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: kinesisvideo:JoinStorageSession on resource: <channelArn> with an explicit deny in an identity-based policy".
Actual: "expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '0', ...}}}".

Cause 3 — DeleteSignalingChannel on a still-linked channel.
Expected: 400 ResourceInUseException, Message "Disable MediaStorageConfiguration to delete the channel"; channel stays ACTIVE.
Actual: "expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}}}".

Cause 4 — GetSignalingChannelEndpoint WEBRTC without media storage.
Expected Message: 'MediaStorageConfiguration is required for WEBRTC protocol'.
Actual Message: 'The WEBRTC protocol is available only for a signaling channel configured for ingestion. Use UpdateMediaStorageConfiguration to configure it.' (code InvalidArgumentException correct). Reported identically by both the MASTER and the VIEWER check.

Cause 5 — UpdateMediaStorageConfiguration stream already mapped.
Expected Message: 'The channel/stream is currently not available for this operation because it is being updated.'
Actual Message: 'The stream arn:aws:kinesisvideo:us-west-2:000000000000:stream/sim2real-kvsrtc-strm-af41ca646a0e/1793302561435 is already mapped to arn:aws:kinesisvideo:us-west-2:000000000000:channel/sim2real-kvsrtc-ch-018fc8e3348c/1793302561430.' (code ResourceInUseException correct; the two ARNs are per-run values, the divergence is the template).

Cause 6 — UpdateMediaStorageConfiguration zero retention.
Expected Message: 'The stream data retention is set to 0 hours.'
Actual Message: 'The Stream data retention in hours is equal to zero.' (code NoDataRetentionException correct; the actual text is verbatim the Smithy doc of com.amazonaws.kinesisvideo#NoDataRetentionException).

Cause 7 — JoinStorageSession malformed channelArn.
Expected: Code 'BadRequestException', Message 'Invalid request body'.
Actual: {'Code': 'ValidationException', 'Message': "1 validation error detected: Value 'not-a-valid-kinesisvideo-channel-arn' at 'channelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z-]*):kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$"}.

Cause 8 — WSS signaling upgrade.
Expected: HTTP 101 on the presigned upgrade to the ResourceEndpoint returned for Protocols=[WSS], Role=MASTER, then one SDP_OFFER signaling message per JoinStorageSession call.
Actual: websockets.exceptions.InvalidStatus: server rejected WebSocket connection: HTTP 404 (both checks; the 'websockets' package IS present — the failure is the emulator's 404 on the upgrade, not a missing runner dependency).

## Steps to Reproduce
Boot the emulator from this checkout and, with kinesisvideo + kinesis-video-webrtc-storage clients:

Setup used by every cause: kv.create_stream(StreamName=S, DataRetentionInHours=1); kv.create_signaling_channel(ChannelName=C); poll both to ACTIVE; kv.update_media_storage_configuration(ChannelARN=<chan>, MediaStorageConfiguration={'StreamARN': <stream>, 'Status': 'ENABLED'}); resolve the data endpoint with kv.get_signaling_channel_endpoint(ChannelARN=<chan>, SingleMasterChannelEndpointConfiguration={'Protocols': ['WEBRTC'], 'Role': 'VIEWER' | 'MASTER'}).

Cause 1: against the VIEWER WEBRTC endpoint call join_storage_session_as_viewer(channelArn=<chan>, clientId='viewer-1') without any prior JoinStorageSession. Observe 404 ResourceNotFoundException "No signaling channel '<name>' found in account <acct> for region <region>"; AWS returns 200. Then repeat with 3 distinct clientIds and a 4th: on AWS the first three succeed and the 4th is 400 ClientLimitExceededException 'Maximum number of viewers connected to the session!'.

Cause 2: create an IAM role whose policy allows kinesisvideo:* and explicitly denies kinesisvideo:JoinStorageSession, assume it, and call join_storage_session(channelArn=<chan>) against the MASTER WEBRTC endpoint. Observe HTTP 200; AWS returns 403 AccessDeniedException with the explicit-deny message.

Cause 3: with MediaStorageConfiguration still ENABLED, call kv.delete_signaling_channel(ChannelARN=<chan>). Observe HTTP 200 and the channel gone; AWS returns 400 ResourceInUseException 'Disable MediaStorageConfiguration to delete the channel' and leaves it ACTIVE. Then set Status=DISABLED and repeat — the delete must succeed.

Cause 4: create a channel WITHOUT linking storage and call kv.get_signaling_channel_endpoint(ChannelARN=<chan>, SingleMasterChannelEndpointConfiguration={'Protocols': ['WEBRTC'], 'Role': 'MASTER'}). Compare the InvalidArgumentException message against 'MediaStorageConfiguration is required for WEBRTC protocol'.

Cause 5: create channel A linked to stream A (ENABLED) and an unlinked channel B, then call kv.update_media_storage_configuration(ChannelARN=<B>, MediaStorageConfiguration={'StreamARN': <stream A>, 'Status': 'ENABLED'}). Compare the ResourceInUseException message against 'The channel/stream is currently not available for this operation because it is being updated.'

Cause 6: create a stream with DataRetentionInHours=0 and try to link it with Status=ENABLED. Compare the NoDataRetentionException message against 'The stream data retention is set to 0 hours.'

Cause 7: against the MASTER WEBRTC endpoint call join_storage_session(channelArn='not-a-valid-kinesisvideo-channel-arn'). Observe Code 'ValidationException'; AWS answers 400 Code 'BadRequestException', Message 'Invalid request body'.

Cause 8: call kv.get_signaling_channel_endpoint(ChannelARN=<chan>, SingleMasterChannelEndpointConfiguration={'Protocols': ['WSS'], 'Role': 'MASTER'}), SigV4-presign the returned wss:// URL with ?X-Amz-ChannelARN=<chan>, and open a WebSocket to it. Observe HTTP 404 on the upgrade; AWS completes the handshake (101). With the socket open, call join_storage_session(channelArn=<chan>) twice and expect one fresh messageType=SDP_OFFER (with a base64 messagePayload) pushed per call.

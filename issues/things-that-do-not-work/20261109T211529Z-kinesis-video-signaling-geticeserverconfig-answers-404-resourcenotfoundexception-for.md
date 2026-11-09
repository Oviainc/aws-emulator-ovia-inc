Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-signaling-ice-server-config-from-https-endpoint
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-signaling-ice-server-config-from-https-endpoint
Issue-Shape-Id: com.amazonaws.kinesisvideosignaling#AWSAcuitySignalingService
Issue-Key: kinesis-video-signaling|GetIceServerConfig|2xx->4xx|$.IceServerList
Issue-Key: kinesis-video-signaling|GetIceServerConfig|2xx->4xx|status
Issue-Commit: 38149bda58555f327ce43dfef323da069da00a52

# kinesis-video-signaling: GetIceServerConfig answers 404 ResourceNotFoundException for a channel the control plane created and reports ACTIVE, instead of 200 with an IceServerList

> _Note: this issue was identified against the code base at commit `38149bda58555f327ce43dfef323da069da00a52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIceServerConfig

## Description
One net-new wire cause, from the single diverging step of the wire corpus for this service (4/5 steps passed; the control-plane CreateSignalingChannel / DescribeSignalingChannel / GetSignalingChannelEndpoint steps that precede it all passed). The replay report marked this step neither ALREADY FILED (no stub_path) nor OWNED by an open issue, so it is filed here. Prior art, named because it does NOT own this step's replay key: open issue 20261029T173857Z-kinesis-video-signaling-4-net-new-root-causes.md describes the same class of defect as its Cause 1, but it is a BEHAVIORAL issue whose Issue-Keys bind suite check ids only, so a fixer replaying it never exercises this wire fixture; the wire binding below is what re-replays this surface. 20261109T162245Z-kinesis-video-signaling-the-channel-s-wss-signaling.md covers the WSS/SendAlexaOfferToMaster surface and not this operation.

=== Cause 1 of 1: the signaling data plane's channel registry is never seeded from the control plane, so GetIceServerConfig cannot resolve any channel that exists ===
AWS: Smithy (com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice.json): GetIceServerConfig is restJson1 POST /v1/get-ice-server-config, http code 200; input ChannelARN is required (target ResourceARN, @length 1..1024, @pattern ^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$) with optional ClientId, Service, Username; output is the single member IceServerList (list of IceServer{Uris, Username @length 1..256, Password @length 1..256, Ttl @range 30..86400}); modeled errors are ClientLimitExceededException 400, InvalidArgumentException 400, InvalidClientException 400, NotAuthorizedException 401, ResourceNotFoundException 404, SessionExpiredException 400. The model does not state resource lifetime, so for that part: recorded behavior only — the fixture creates a signaling channel on the kinesisvideo control plane, observes it ACTIVE, resolves its HTTPS data-plane endpoint with GetSignalingChannelEndpoint, and then calls GetIceServerConfig with that channel's ARN against that endpoint; AWS answers HTTP 200 with a non-empty IceServerList (one IceServer carrying Ttl 300, at least one TURN URI in Uris, and a Username and Password). ResourceNotFoundException is reserved for a well-formed ChannelARN naming a channel that does not exist (never created, or deleted).
Emulator: the same call against the endpoint the control plane just advertised answers HTTP 404 instead of 200, with no IceServerList member at all ($.IceServerList expected a one-element list, got None). The channel demonstrably exists in the emulator: the preceding steps of the same fixture created it, saw ChannelStatus ACTIVE and got an endpoint back. The signaling crate resolves the ARN against its own private channel registry, whose only writer is the crate's own register_channel, and no control-plane path calls it, so the registry is permanently empty and every lookup reports NotFound.
Why: this is the service's only ICE/TURN operation, so a WebRTC client can never obtain relay credentials for a channel it just created and no viewer can fall back to TURN. Worse, the failure mode is a not-found verdict on a resource the control plane reports as ACTIVE, which drives a client down a delete-and-recreate path instead of a retry, and the 404 is indistinguishable (by status and code) from a genuinely absent channel.
Fix: the signaling data plane's channel state and its seeding seam — the crate's channel store (state.rs register_channel / resolve_channel / resolve_channel_mut) must be fed by, or share one store with, the kinesisvideo control plane's CreateSignalingChannel and DeleteSignalingChannel, so channel create/delete is visible to the GetIceServerConfig handler's lookup (src/ops/get_ice_server_config.rs). With the lookup resolving, the handler must emit the 200 body: an IceServerList with at least one IceServer whose Uris hold TURN URIs for the channel's region and whose Username/Password/Ttl satisfy the modeled constraints (Ttl within @range 30..86400; the recording shows 300).
Traces: kinesis-video-signaling-ice-server-config-from-https-endpoint#task#4

## Evidence
Fixture kinesis-video-signaling-ice-server-config-from-https-endpoint, step task#4 (GetIceServerConfig), verdict mismatch; divergence key service=kinesis-video-signaling|GetIceServerConfig|status=200->404|keys=$.IceServerList,status. Corpus summary: 5 steps, 4 passed, 1 mismatched, 0 errored, 0 blocked, score 80.0%; emulator_up true, bootstrap_signal false.

Expected (recorded AWS response): HTTP 200 with body {"IceServerList": [{"Uris": [<TURN URI>], "Username": <username>, "Password": <password>, "Ttl": 300}]} — the fixture asserts the shape and Ttl 300; Uris/Username/Password are shaped-not-asserted (their concrete values are per-run and are NOT a requirement).
Actual (emulator): HTTP 404, and $.IceServerList is missing entirely (got None).
Replay detail, verbatim: "status: expected 200, got 404\n$.IceServerList: missing (expected [{'Password': '<shaped-not-asserted>', 'Ttl': 300, 'Uris': ['<shaped-not-asserted>'], 'Username': '<shaped-not-asserted>'}], got None)".

The 404 body is the service's ResourceNotFoundException (modeled httpError 404). Its message is only correct for a channel that does not exist; here it is returned for one that does, so the status/code itself is the divergence. Note the separately-filed message-wording defect for that exception (open issue 20261029T173857Z-..., Cause 2): once the lookup is fixed this step stops reaching that path at all.

Smithy used: GetIceServerConfig {http POST /v1/get-ice-server-config, code 200}; GetIceServerConfigResponse member IceServerList; IceServer.Ttl @range 30..86400; IceServer.Username/Password @length 1..256 @pattern ^[a-zA-Z0-9_.-]+$; ResourceNotFoundException httpError 404.

## Steps to Reproduce
1. Boot the emulator and replay fixture kinesis-video-signaling-ice-server-config-from-https-endpoint; steps 1-3 (kinesisvideo CreateSignalingChannel with ChannelType SINGLE_MASTER, DescribeSignalingChannel until ChannelInfo.ChannelStatus == ACTIVE, GetSignalingChannelEndpoint with SingleMasterChannelEndpointConfiguration={Protocols:[HTTPS], Role:VIEWER}) all pass and yield the channel ARN plus an HTTPS data-plane endpoint.
2. Step task#4: POST /v1/get-ice-server-config to that HTTPS endpoint with {"ChannelARN": <the ARN from step 1>} (optionally ClientId matching ^[a-zA-Z0-9_.-]+$).
3. Observe HTTP 404 with a ResourceNotFoundException body and no IceServerList member.
4. Expected: HTTP 200 with a non-empty IceServerList — one IceServer carrying Ttl 300, at least one TURN URI in Uris, and a Username and Password within @length 1..256 / @pattern ^[a-zA-Z0-9_.-]+$.
5. Confirm the state gap: the channel the call was rejected for is the one DescribeSignalingChannel reports ACTIVE in step 2, so the refusal comes from the signaling crate's unseeded channel registry, not from a missing resource. Re-replay this fixture/step after wiring the registry to the control plane to verify.

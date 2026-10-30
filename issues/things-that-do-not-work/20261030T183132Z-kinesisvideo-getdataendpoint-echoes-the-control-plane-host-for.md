Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-stream-get-data-endpoint
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-stream-get-data-endpoint kinesisvideo-get-data-endpoint
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|GetDataEndpoint|2xx->2xx|$.DataEndpoint
Issue-Commit: 4226368beaf76fbb89a0358ac027d2e5fc79db7f

# kinesisvideo GetDataEndpoint echoes the control-plane host for every APIName instead of a per-plane data endpoint

> _Note: this issue was identified against the code base at commit `4226368beaf76fbb89a0358ac027d2e5fc79db7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataEndpoint

## Description
One net-new root cause in the kinesisvideo control plane: GetDataEndpoint does not resolve a data-plane endpoint at all, it reflects the host the request arrived on, so the value is the same for every APIName and is not of the form Kinesis Video Streams hands out.

Cause 1 of 1: GetDataEndpoint returns the control-plane host it was called on, ignoring APIName.
AWS: Smithy com.amazonaws.kinesisvideo#GetDataEndpoint, POST /getDataEndpoint, HTTP 200. Input: APIName (required enum: GET_CLIP | GET_DASH_STREAMING_SESSION_URL | GET_HLS_STREAMING_SESSION_URL | GET_IMAGES | GET_MEDIA | GET_MEDIA_FOR_FRAGMENT_LIST | LIST_FRAGMENTS | PUT_MEDIA), plus StreamName (len 1..256, pattern ^[a-zA-Z0-9_.-]+$) or StreamARN (ResourceARN pattern ^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$). Output: DataEndpoint, documented as "The endpoint value. To read data from the stream or to write data to it, specify this endpoint in your application"; the operation doc is "Gets an endpoint for a specified stream for either reading or writing ... The returned endpoint does not have the API name appended." The operation exists precisely because the media planes are NOT on the control-plane host: DataEndpoint is a scheme+host base URL (no path, no API name appended) naming a kinesisvideo data host for the stream, it is DISTINCT from the control-plane endpoint the GetDataEndpoint call itself was made to, and it is selected by APIName — the ingest plane (PUT_MEDIA) and the read/archive plane (GET_MEDIA, GET_MEDIA_FOR_FRAGMENT_LIST, LIST_FRAGMENTS, GET_CLIP, GET_HLS_STREAMING_SESSION_URL, GET_DASH_STREAMING_SESSION_URL, GET_IMAGES) resolve to different hosts. The requirement is the distinctness and the APIName-dependence of the value, never any particular hostname: the concrete host, port and generated id differ per environment and are not part of the contract.
Emulator: status 200 is correct, but $.DataEndpoint is the control-plane listener the request arrived on, echoed back verbatim as a bare scheme://host:port of the emulator's own front door, and the SAME value is returned for every APIName the fixtures ask for across the whole cluster (8 steps, 4 distinct APIName requests in each of two fixtures, one identical value). The handler never reads APIName for routing: services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/get_data_endpoint.rs:191-193 returns `data_endpoint: Some(crate::state::request_endpoint(ctx.parts(), &region))` after only validating the enum and resolving the stream, so APIName is used solely to reject out-of-set values and has no influence on the returned endpoint.
Why: a client that follows the documented discovery flow — call GetDataEndpoint, then address the returned host for PutMedia/GetMedia/ListFragments — is sent straight back to the control plane, where no media operation is routed, so the producer and consumer halves of Kinesis Video Streams are unreachable through the only documented way to find them. Because the ingest and archive endpoints are byte-identical, nothing can distinguish the two planes, no client can detect a call aimed at the wrong plane, and producer SDKs (the KVS C/Java producer, GStreamer kvssink) that key their session on a per-plane host cannot work. The value is also not a discoverable endpoint at all for anything outside the emulator's own loopback, so a caller reached on one address is handed another.
Fix: the GetDataEndpoint handler plus the engine's endpoint mapping for the kinesisvideo family — resolve DataEndpoint from the requested APIName to the stream's ingest or archive/read data host, distinct from the control-plane host, instead of reflecting ctx.parts() through crate::state::request_endpoint. The handler's stream resolution, enum validation and modeled-error mapping (InvalidArgumentException 400 / ResourceNotFoundException 404) are correct and should not change.
Traces: kinesis-video-stream-get-data-endpoint#task#2 (and #task#3, #task#4, #task#5; kinesisvideo-get-data-endpoint#task#2, #task#3, #task#4, #task#5 — one cluster, 8 steps).

Related open issues (neither carries this wire replay key, so this divergence is unowned): 20261029T174024Z-kinesis-video-4-net-new-root-causes-the.md describes the same handler defect from the BEHAVIORAL corpus under service token kinesis-video (its Cause 1), and 20261029T194209Z-kinesisvideo-refusal-sentences-and-signaling-channel-endpoint-projections.md covers the sibling GetSignalingChannelEndpoint projection but explicitly excludes the endpoint values. Fixing this one should close all three views of it.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, /__admin/health 200; 147 steps, 89 passed, 58 mismatched, score 60.5%).

Divergence cluster service=kinesisvideo|GetDataEndpoint|status=200->200|keys=$.DataEndpoint — 8 steps, verdict mismatch, no stub_path and no owning issue.
  representative: kinesis-video-stream-get-data-endpoint#task#2
  affected: kinesis-video-stream-get-data-endpoint#task#3, #task#4, #task#5, kinesisvideo-get-data-endpoint#task#2, #task#3, #task#4, #task#5
  expected: $.DataEndpoint — a kinesisvideo data-plane base URL for the stream, shape-asserted ('<shaped-not-asserted>'), distinct from the control-plane endpoint the call was made to and selected by the requested APIName.
  actual:   $.DataEndpoint = the emulator's own control-plane listener URL, echoed from the inbound request ('https://127.0.0.1:35573' in this run), identical for every APIName in the cluster.
  (The literal host and port are per-environment values of this run's listener and are NOT the requirement; the divergence is that the value is the control-plane host and does not depend on APIName.)

Status 200 and the response shape are otherwise correct — only the DataEndpoint value diverges.
Code: services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/get_data_endpoint.rs:191-193 (`crate::state::request_endpoint(ctx.parts(), &region)`); input.api_name is read only at lines 169-179 for enum validation.
Smithy: GetDataEndpoint POST /getDataEndpoint -> 200, GetDataEndpointOutput.DataEndpoint; operation doc "Gets an endpoint for a specified stream for either reading or writing ... The returned endpoint does not have the API name appended."

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from the checkout.
2. Replay the wire corpus simparity-wire-aws-v1 and look at fixtures kinesis-video-stream-get-data-endpoint (steps task#2..task#5) and kinesisvideo-get-data-endpoint (steps task#2..task#5).
3. Manually: POST /createStream {"StreamName":"kv-probe","DataRetentionInHours":1} to the control-plane endpoint, then poll POST /describeStream until StreamInfo.Status is ACTIVE.
4. POST /getDataEndpoint {"StreamName":"kv-probe","APIName":"PUT_MEDIA"} and record $.DataEndpoint. Repeat with APIName GET_MEDIA, LIST_FRAGMENTS and GET_HLS_STREAMING_SESSION_URL.
5. Observe: all four responses carry the identical DataEndpoint, and that value is the control-plane scheme://host:port the getDataEndpoint request itself was sent to. Expected: each APIName resolves to its plane's data host (ingest for PUT_MEDIA, archive/read for the others), and neither equals the control-plane endpoint.
6. Confirm the cause in services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/get_data_endpoint.rs:191-193 — the output is built from crate::state::request_endpoint(ctx.parts(), &region) and input.api_name is never consulted for routing.

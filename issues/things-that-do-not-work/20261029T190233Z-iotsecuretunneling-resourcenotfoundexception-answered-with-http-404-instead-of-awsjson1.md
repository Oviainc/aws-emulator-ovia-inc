Issue-Source: trace-sweep-divergence
Issue-Trace: iotsecuretunneling-resource-not-found-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsecuretunneling-resource-not-found-error-paths
Issue-Shape-Id: com.amazonaws.iotsecuretunneling#IoTSecuredTunneling
Issue-Key: iotsecuretunneling|DescribeTunnel|4xx->4xx|status
Issue-Key: iotsecuretunneling|CloseTunnel|4xx->4xx|status
Issue-Key: iotsecuretunneling|RotateTunnelAccessToken|4xx->4xx|status
Issue-Key: iotsecuretunneling|ListTagsForResource|4xx->4xx|status
Issue-Key: iotsecuretunneling|TagResource|4xx->4xx|status
Issue-Key: iotsecuretunneling|UntagResource|4xx->4xx|status
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# ResourceNotFoundException answered with HTTP 404 instead of awsJson1_1's 400 on every not-found path

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTunnel

## Description
This issue bundles SIX causes — one per operation that raises ResourceNotFoundException. All six share ONE root cause: the modeled error's HTTP status is taken from its @httpError(404) trait, but IoTSecuredTunneling is an awsJson1_1 service, where @http/@httpError bindings are inert on the wire and a modeled client error is serialized with HTTP 400 and the error type in the body's __type / x-amzn-errortype. A fixer must close all six before submitting.

Shared statement of the fault:
  AWS:      Smithy: service com.amazonaws.iotsecuretunneling#IoTSecuredTunneling, protocol awsJson1_1 (sdkId IoTSecureTunneling, version 2018-10-05). ResourceNotFoundException is errorKind=client with httpError 404, but the model query reports httpBindingsInert=true for every operation of this service ("RPC protocol (awsJson/awsQuery/ec2Query/rpcv2Cbor): @http/httpLabel/httpQuery/httpHeader are ignored on the wire"), and the recorded responses confirm it: every not-found response is HTTP 400 carrying error code ResourceNotFoundException. The error message is unchanged by this defect and already matches: fixed text "Tunnel " + " does not exist." with the tunnelId taken from the request (for the ARN-addressed tag paths, the tunnel id lifted out of the supplied resourceArn).
  Emulator: answers HTTP 404 with the same ResourceNotFoundException code and message. Only the status line diverges — every replayed step reports "status: expected 400, got 404".
  Why:      an SDK's error classifier and any client branching on status sees a 404 where AWS sends 400. Generic HTTP middleware and retry/error-mapping layers treat 404 as a transport/route failure ("no such endpoint/resource") rather than a modeled service exception, so a caller that handles ResourceNotFoundException by status, or a proxy/gateway that maps 404 specially, behaves differently against the emulator than against AWS. It also makes the emulator inconsistent with the awsJson1_1 framing it uses for every other response.
  Fix:      the modeled-error status mapping for this awsJson1_1 service's error channel — the http_status() the generated #[operation_error] enum derives from @httpError, surfaced through Scaffolded::http_status; under awsJson1_1 a modeled client error must render 400, not the inert @httpError value. One change in the error-status path fixes all six operations.

Cause 1 of 6: DescribeTunnel not-found. Fixture iotsecuretunneling-resource-not-found-error-paths, step task#3 — expected 400, got 404.
Cause 2 of 6: CloseTunnel not-found. Fixture iotsecuretunneling-resource-not-found-error-paths, step task#4 — expected 400, got 404.
Cause 3 of 6: RotateTunnelAccessToken not-found. Fixture iotsecuretunneling-resource-not-found-error-paths, step task#5 — expected 400, got 404.
Cause 4 of 6: ListTagsForResource not-found (ARN-addressed). Fixture iotsecuretunneling-resource-not-found-error-paths, step task#6 — expected 400, got 404.
Cause 5 of 6: TagResource not-found (ARN-addressed). Fixture iotsecuretunneling-resource-not-found-error-paths, step task#7 — expected 400, got 404.
Cause 6 of 6: UntagResource not-found (ARN-addressed). Fixture iotsecuretunneling-resource-not-found-error-paths, step task#8 — expected 400, got 404.

No open issue covers this. The nearest open one, "ARN-addressed not-found message lifts a 'tunnel id' out of a string that is not a tunnel ARN" (20261029T071638Z-iotsecuretunneling-listtagsforresource-error-response.md), is about the message TEXT of the ARN-addressed path and does not carry this status divergence or its replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture iotsecuretunneling-resource-not-found-error-paths (6 mismatch clusters, 6 steps):
  task#3 DescribeTunnel            — detail "status: expected 400, got 404"
  task#4 CloseTunnel               — detail "status: expected 400, got 404"
  task#5 RotateTunnelAccessToken   — detail "status: expected 400, got 404"
  task#6 ListTagsForResource       — detail "status: expected 400, got 404"
  task#7 TagResource               — detail "status: expected 400, got 404"
  task#8 UntagResource             — detail "status: expected 400, got 404"
Expected (recorded AWS): HTTP 400, error code ResourceNotFoundException, message "Tunnel <tunnelId> does not exist." (fixed text; <tunnelId> from the request).
Actual (emulator): HTTP 400 -> 404; code and message as above.
Model: smithy_query(query="service_info") => protocols ["awsJson1_1"]; smithy_query(query="shape", shape="ResourceNotFoundException") => errorKind client, httpError 404; smithy_query(query="operation") => httpBindingsInert: true for all six operations.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture iotsecuretunneling-resource-not-found-error-paths.
2. Steps task#3..task#8 each address a tunnel id / tunnel ARN that does not exist in the request's Region, via DescribeTunnel, CloseTunnel, RotateTunnelAccessToken, ListTagsForResource, TagResource and UntagResource respectively.
3. Observe each response line: HTTP 404 with __type ResourceNotFoundException, where the recording has HTTP 400 with the same code and message.

Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit
Issue-Shape-Id: com.amazonaws.partnercentralchannel#PartnerCentralChannel
Issue-Key: partnercentral-channel|ListChannelHandshakes|2xx->4xx|$.items
Issue-Key: partnercentral-channel|ListChannelHandshakes|2xx->4xx|$headers.content-type
Issue-Key: partnercentral-channel|ListChannelHandshakes|2xx->4xx|status
Issue-Commit: 09c29ed8e99ab10517eef5fd4d5c2097ded28d72

# PartnerCentral Channel ListChannelHandshakes returns 404 and awsJson1_1 content-type

> _Note: this issue was identified against the code base at commit `09c29ed8e99ab10517eef5fd4d5c2097ded28d72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

Both reported causes are fixed, and one residual cause is DECLINED as a vendor-level gap.

Fixed:
* `ListChannelHandshakes` is implemented in `services/service-com-amazonaws-partnercentralchannel-partnercentralchannel/src/ops/list_channel_handshakes.rs`: it pages over the channel handshakes in `src/state.rs` and always serializes `items` — the empty list when the catalog/handshakeType/participantType scope and the optional filters select nothing — with `nextToken` omitted on the last page. `CreateChannelHandshake` is implemented as the store's writer, so the list reports persisted state.
* The 404 itself was NOT the handler: the request was claimed by no service, so the ROUTER answered `404 UnknownService` in its own awsJson1_1 envelope — which is also where the wrong content-type came from (the framework already derives `application/x-amz-json-1.0` from the model's `aws.protocols#awsJson1_0`). `src/service.rs`'s `can_handle` now also claims a request whose `X-Amz-Target` resolves to this service's shape-name route key (`partnercentralchannel`), which its SigV4 signing name (`partnercentral-channel`) spells differently. Pinned by `tests/all/channel_handshakes.rs` and, in the assembled router, by `core/router/tests/all/partnercentral_channel_target_routing.rs`.
* Replay confirms the `$headers.content-type` key fixed.

Declined (vendor-level): the recorded request is **SigV4A-signed** (`Authorization: AWS4-ECDSA-P256-SHA256`), so with request authentication on it is now refused `403 AccessDenied` before any handler runs, and the `status` / `$.items` keys still diverge on replay for that reason alone. The emulator's only authenticator is `SigV4Authenticator` in `vendor/server/src/request_authentication.rs`, whose `recognizes` accepts `AWS4-HMAC-SHA256` only, registered in `vendor/smithy-server-runtime/src/multi_service_router.rs` (`MultiServiceRouter::new`); there is no service-level seam to add another, and `vendor/` is read-only here. Closing it needs a SigV4A authenticator in the core library (`AWS4-ECDSA-P256-SHA256`, `X-Amz-Region-Set`, `*` region scope) registered beside the SigV4 one, which would serve every `aws.auth#sigv4a` service, not just this one. With authentication off the same request is served 200 with `items: []`.

## Operation
ListChannelHandshakes

## Description
The PartnerCentral Channel emulator is serving (it answers the data listener and emits an AWS-JSON error envelope), but the ListChannelHandshakes operation is neither dispatched nor serialized with the service's wire protocol. Two distinct causes, both observed on the same replayed step:

Cause 1 of 2: ListChannelHandshakes is not routed/handled, so a well-formed list call 404s instead of returning an (empty) result page.
  AWS: Smithy model services/service-com-amazonaws-partnercentralchannel-partnercentralchannel/smithy-model/com-amazonaws-partnercentralchannel-partnercentralchannel.json — service PartnerCentralChannel (version 2024-03-18, endpointPrefix partnercentral-channel, protocol awsJson1_0). ListChannelHandshakes is @readonly and @paginated (inputToken/outputToken nextToken, pageSize maxResults, items items); its @http binding declares code 200 (bindings inert under awsJson1_0 — dispatch is by X-Amz-Target with the payload in the body). Output shape ListChannelHandshakesResponse has members items (ChannelHandshakeSummaries, a list) and nextToken. The only modeled errors are AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429) and ValidationException (400). A valid list request whose filters match nothing is a success: AWS returns 200 with items present as an empty list, not a 404 — ResourceNotFoundException is reserved for a named resource that does not exist, and a list operation over a catalog has no such subject. The recorded response for this step is 200 with items = [].
  Emulator: returns HTTP 404 and no body members at all — $.items is missing (expected [], got None).
  Why: every SDK and CLI client treats 404 on this operation as a terminal ResourceNotFoundException, so a client that should receive an empty first page instead raises an error. Paginators abort on the first call, and callers that enumerate handshakes to decide whether a channel has any (the empty-result case this fixture covers) cannot distinguish "none exist" from "gone" — the normal zero-result path is unusable.
  Fix: the ListChannelHandshakes handler in the partnercentral-channel service crate — register the operation on the X-Amz-Target dispatch table and have it return a 200 ListChannelHandshakesResponse whose items is always serialized (an empty list when the catalog/handshakeType/participantType filters match nothing) with nextToken omitted on the last page, reading from the service's handshake state.

Cause 2 of 2: responses are serialized with the awsJson1_1 content-type instead of awsJson1_0.
  AWS: the model binds PartnerCentralChannel to aws.protocols#awsJson1_0, which fixes the response Content-Type to application/x-amz-json-1.0 for both successes and errors.
  Emulator: answers with Content-Type: application/x-amz-json-1.1 (value_mismatch on $headers.content-type, expected 'application/x-amz-json-1.0').
  Why: the content-type is how a client confirms it is talking to the protocol it negotiated; a 1.1 header on a 1.0 service makes strict clients and protocol-conformance tooling reject or mis-parse the envelope, and it will mis-type error shapes too once Cause 1 is fixed, so fixing routing alone still leaves every response on this service off-protocol.
  Fix: the protocol/serializer layer of the partnercentral-channel service crate — derive the response content-type from the service's declared protocol (awsJson1_0) rather than defaulting to the awsJson1_1 constant.

Traces: both causes reproduce on fixture partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit, steps task#1 and task#2 (one divergence cluster, 2 affected steps). No open issue covers this service's fidelity; the only open partnercentral issues are bootstrap issues for the unrelated partnercentral-account and partnercentral-revenue-measurement services.

## Evidence
Replay of fixture partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit, step task#1 (verdict: mismatch; divergence_key service=partnercentral-channel|ListChannelHandshakes|status=200->404|keys=$.items,$headers.content-type,status):

Expected (recorded AWS response):
  status: 200
  $.items: [] (present, empty list)
  $headers.content-type: application/x-amz-json-1.0

Actual (emulator):
  status: 404
  $.items: missing (None)
  $headers.content-type: application/x-amz-json-1.1

Affected steps in the cluster (2): task#1, task#2. Corpus summary: steps_total 2, steps_passed 0, steps_mismatched 2, score_pct 0.0, state_reset_failures 0, bootstrap_signal false, emulator_up true (/__admin/health answered 200 and the data listener accepts). Model basis: protocols [awsJson1_0]; ListChannelHandshakes http {POST /ListChannelHandshakes, code 200}, output ListChannelHandshakesResponse{items: ChannelHandshakeSummaries, nextToken}, errors AccessDeniedException/InternalServerException/ResourceNotFoundException/ThrottlingException/ValidationException.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1, fixture partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit, step task#1 (and task#2) — i.e. POST an awsJson1_0 ListChannelHandshakes request (X-Amz-Target PartnerCentralChannel.ListChannelHandshakes, Content-Type application/x-amz-json-1.0) carrying the required catalog, handshakeType and participantType, with filters that match no handshakes.
3. Observe the response status is 404 with no items member, rather than 200 with items: [].
4. Observe the response Content-Type header is application/x-amz-json-1.1 rather than application/x-amz-json-1.0.

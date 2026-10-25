Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# PutOutboundRequestBatch stores expirationTime but never enforces it: an already-expired request is reported successful and buffered requests never expire

_Filed: 2026-10-24T14:34:54Z_
_Operation: PutOutboundRequestBatch | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`OutboundRequest$expirationTime` is `@required`, and the handler passes it into the store (`StoredOutboundRequest.expires_at_epoch_seconds`), but nothing in the crate ever reads that field again. Two consequences are observable on the wire:

1. A request whose `expirationTime` is already in the past is accepted: it is reported in `successfulRequests` with a freshly minted `DialRequestId`, as if the service had queued a dial it can never place. Expected: the per-request outcome for a request that cannot be dialled is a `failedRequests` entry carrying the modelled `FailureCode` `InvalidInput` ("The request failed to satisfy the constraints specified by the service"), with the rest of the batch unaffected — the per-request failure channel this operation already uses for a channel the campaign is not configured for.
2. Nothing ever leaves the campaign's buffer. A request that has passed its `expirationTime` still occupies one of the `MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN` slots that produce the `BufferLimitExceeded` failure code, and its `clientToken` keeps answering as an idempotent repeat forever, so a caller that re-submits the same token after the original request expired is handed the stale dial id instead of a new request.

## Evidence

Spec: `OutboundRequest$expirationTime` (target `TimeStamp`, `@required`) is the instant after which the request is no longer dialable — the only reason the member exists and the only reason it is required; `FailureCode` declares `InvalidInput` for "The request failed to satisfy the constraints specified by the service" and `BufferLimitExceeded` for "Request throttled due to large number of pending dial requests", i.e. the buffer is a buffer of PENDING requests, not of every request ever submitted.

Handler/state: src/ops/put_outbound_request_batch.rs:242-247 passes `request.expiration_time.secs()` to `store.take_outbound_request(...)`; src/state.rs `take_outbound_request` writes it into `StoredOutboundRequest { expires_at_epoch_seconds, .. }` and then every other code path (`outbound_requests.get(client_token)` for idempotency, `outbound_requests.len() >= MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN` for the buffer cap, `remove_campaign`) ignores it. `grep` for `expires_at_epoch_seconds` finds the write and no read. The handler validates the requested channel against the campaign's `channelSubtypeConfig` but never looks at the clock (it takes `_ctx` and computes no `now`).

Test coverage: not asserted. tests/all/instance_onboarding_tags_and_outbound_requests.rs (`every_request_of_a_batch_is_reported_on_its_own_and_a_repeat_is_idempotent`, `a_multi_channel_campaign_takes_a_request_on_every_channel_it_configures`) submits only future expiration times and never re-submits a token after expiry, so no test covers either symptom.

## Steps to Reproduce

1. Boot the emulator, onboard instance-1 (PUT /v2/connect-instance/instance-1/onboarding, poll until SUCCEEDED).
2. PUT /v2/campaigns with a telephony channelSubtypeConfig for instance-1 and keep the id; POST /v2/campaigns/{id}/start so the campaign is Running.
3. PUT /v2/campaigns/{id}/outbound-requests with outboundRequests=[{"clientToken":"expired-1","expirationTime":"2020-01-01T00:00:00Z","channelSubtypeParameters":{"telephony":{"destinationPhoneNumber":"+15550100","attributes":{}}}}] -> observe HTTP 200 with the request in successfulRequests and a minted id; expected it in failedRequests with failureCode="InvalidInput" (the batch itself still 200).
4. Submit a second batch with clientToken "expired-1" again (same or different parameters) -> observe the same dial id returned as an idempotent repeat of a request that expired years ago; expected the expired request not to be holding that token (and not to be occupying a buffer slot).

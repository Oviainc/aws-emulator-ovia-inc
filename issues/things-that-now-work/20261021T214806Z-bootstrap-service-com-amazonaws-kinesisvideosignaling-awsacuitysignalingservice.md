Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the Kinesis Video Signaling emulator

_Filed: 2026-10-21T21:48:06Z_
_Operation: service-com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole `Kinesis Video Signaling` data plane (`restJson1`, sigv4 signing name `kinesisvideo`, endpoint prefix `kinesisvideo`) is an unmodified scaffold: both bound operations answer HTTP 501 `NotImplemented` and no state is held. This issue defines the minimal happy-path slice to implement first.

The service binds exactly TWO operations (per OPERATIONS.md), so the first slice is both of them, driven off a signaling-channel registry:

1. **GetIceServerConfig** — `POST /v1/get-ice-server-config`.
   Input: `ChannelARN` (required, len 1..1024, pattern `^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`), `ClientId` (len 1..256, `^[a-zA-Z0-9_.-]+$`), `Service` (enum `Service`, only value `TURN`), `Username`.
   Output: `IceServerList` (list of `IceServer{Uris: list<string>, Username, Password, Ttl: int 30..86400}`).
   Must look the channel up in persisted state and synthesise a per-channel/per-client TURN credential set (deterministic STUN/TURN URIs derived from the channel's region, a `Username` of the form `<expiry>:<ClientId>`-ish, a generated `Password`, and a `Ttl` within the modelled 30..86400 range), then RECORD the issued credential so repeat calls are consistent and the issued username/password can be correlated. Unknown `ChannelARN` -> `ResourceNotFoundException` (404). Malformed ARN / bad `Service` value / pattern or length violation -> `InvalidArgumentException` (400). Missing-or-unknown `ClientId` when the channel requires one -> `InvalidClientException` (400).

2. **SendAlexaOfferToMaster** — `POST /v1/send-alexa-offer-to-master`.
   Input: `ChannelARN` (required), `SenderClientId` (required, len 1..256, `^[a-zA-Z0-9_.-]+$`), `MessagePayload` (required, len 1..10000, base64 pattern `^[a-zA-Z0-9+/=]+$`).
   Output: `Answer` (string, len 1..10000).
   Must resolve the channel from state, append the offer to that channel's per-sender offer log, and return the `Answer` that the registered master for that channel has supplied — i.e. the answer must come OUT OF STATE, not be a constant string nor an echo of `MessagePayload`. Unknown `ChannelARN` -> `ResourceNotFoundException` (404); invalid/non-base64 `MessagePayload` or pattern/length violations -> `InvalidArgumentException` (400).

Because channel creation lives in the Kinesis Video control plane (`CreateSignalingChannel`), the emulator needs a channel registry this crate can read. Implement it so that (a) a channel record created/seeded in shared state is visible to both operations, and (b) an ARN that is not in the registry produces `ResourceNotFoundException` rather than a success. Both operations must compute their responses from that persisted state — no canned bodies, no echoed inputs, no random values that are never stored.

## State Model

Store in `ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState` (reached via `crate::state::service_state(&*self.engine)`):

- `channels: HashMap<String /* ChannelARN */, SignalingChannel>`
- `SignalingChannel { arn: String, name: String, region: String, account: String, ice_servers: Vec<IceServerRecord>, offers: Vec<AlexaOffer>, master_answer: Option<String> }`
- `IceServerRecord { uris: Vec<String>, username: String, password: String, ttl_secs: i32, issued_at: SystemTime, client_id: Option<String> }` — one record per (channel, client) credential issuance; `GetIceServerConfig` creates-or-refreshes it and returns it, so two calls with the same `ClientId` inside `ttl_secs` return the same credentials.
- `AlexaOffer { sender_client_id: String, message_payload: String /* base64 */, received_at: SystemTime, answer: Option<String> }` — appended by `SendAlexaOfferToMaster`; the returned `Answer` is read from `answer` (falling back to the channel's `master_answer`), so the response is a function of stored state.

All state is keyed by `ChannelARN` and lives in the single shared engine so a write by one request is observable by the next.

Done = a client can register/seed a signaling channel, call `GetIceServerConfig` and get a TURN config whose `Ttl` is in range and whose credentials are stable across repeat calls, call `SendAlexaOfferToMaster` and get back the master's answer from state, and get `ResourceNotFoundException` (404) for an ARN that was never created and `InvalidArgumentException` (400) for a malformed ARN.

## Evidence

OPERATIONS.md for this crate lists exactly 2 operations with `restJson1` routes `POST /v1/get-ice-server-config` and `POST /v1/send-alexa-offer-to-master`, their required/optional members with length+pattern constraints, their outputs (`IceServerList`, `Answer`), and their modelled errors (`ClientLimitExceededException` 400, `InvalidArgumentException` 400, `InvalidClientException` 400, `NotAuthorizedException` 401, `ResourceNotFoundException` 404, `SessionExpiredException` 400). `smithy_query` on shape `IceServer` confirms members `Uris`, `Username`, `Password`, `Ttl` (range 30..86400). Both handler files — `src/ops/get_ice_server_config.rs` and `src/ops/send_alexa_offer_to_master.rs` — still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, so no behaviour and no state exist today.

## Steps to Reproduce

1. Start the emulator and create/seed a signaling channel with ARN `arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000`.
2. POST /v1/get-ice-server-config with `{"ChannelARN": "<that ARN>", "ClientId": "viewer-1", "Service": "TURN"}` — expect 200 with a non-empty `IceServerList`, each entry having `Uris`, `Username`, `Password` and a `Ttl` between 30 and 86400. Currently: HTTP 501 NotImplemented.
3. Repeat the same GetIceServerConfig call — expect the same credentials back (issuance persisted in state), not a freshly randomised set.
4. POST /v1/send-alexa-offer-to-master with `{"ChannelARN": "<that ARN>", "SenderClientId": "alexa-1", "MessagePayload": "<base64 SDP offer>"}` — expect 200 with an `Answer` read from the channel's stored master answer, not a constant and not the echoed payload. Currently: HTTP 501.
5. POST either operation with an ARN that was never created — expect `ResourceNotFoundException` with HTTP 404.
6. POST either operation with `ChannelARN` = `not-an-arn` — expect `InvalidArgumentException` with HTTP 400.
7. POST GetIceServerConfig with `"Service": "STUN"` (not a modelled enum value) — expect a 400 validation/`InvalidArgumentException` response, not a 200.

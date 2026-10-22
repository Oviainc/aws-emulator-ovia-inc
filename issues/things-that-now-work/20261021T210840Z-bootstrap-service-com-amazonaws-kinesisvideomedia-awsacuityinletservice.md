Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Kinesis Video Media emulator

_Filed: 2026-10-21T21:08:40Z_
_Operation: service-com-amazonaws-kinesisvideomedia-awsacuityinletservice | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has exactly one bound operation, `GetMedia` (`POST /getMedia`, restJson1, sigv4 signing name `kinesisvideo`), and its handler (`src/ops/get_media.rs`) is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Because the service surface is a single operation, the bootstrap happy path is: seed/stream state -> `GetMedia` with `StartSelector.StartSelectorType=EARLIEST` -> `GetMedia` continuation with `StartSelectorType=CONTINUATION_TOKEN` -> `GetMedia` on an unknown stream returning `ResourceNotFoundException` (404).

Happy-path slice to implement first (3-4 call shapes, all against GetMedia):

1. **GetMedia (EARLIEST)** — Input: `StreamName` (1..256, `^[a-zA-Z0-9_.-]+$`) OR `StreamARN` (1..1024, `^arn:aws:kinesisvideo:...$`), `StartSelector` (required) with `StartSelectorType=EARLIEST`. Output: `ContentType` header (e.g. `video/webm`) and `Payload` (`@httpPayload` blob, required) containing the stored chunks from the oldest fragment onward, concatenated in fragment order. The payload bytes must come from fragments PERSISTED in this crate's state, never a canned/constant blob.
2. **GetMedia (NOW)** — returns only the chunk(s) at/after the newest stored fragment; an empty stream yields an empty payload, not an error.
3. **GetMedia (CONTINUATION_TOKEN)** — the emulator must mint a continuation token when it stops emitting (persisted alongside the fragment cursor, matching `^[a-zA-Z0-9_\.\-]+$`, 1..128) and a follow-up request with `StartSelector.ContinuationToken` must resume exactly after the previously returned fragment — i.e. no duplicated and no skipped fragments across two sequential calls.
4. **GetMedia selector variants / errors** — `FRAGMENT_NUMBER` requires `AfterFragmentNumber` (1..128, `^[0-9]+$`) and starts with the fragment AFTER it; `PRODUCER_TIMESTAMP`/`SERVER_TIMESTAMP` require `StartTimestamp` and start with the fragment CONTAINING that timestamp. A selector missing its required companion field, or a request supplying neither `StreamName` nor `StreamARN` (or both), must fail with `InvalidArgumentException` (HTTP 400). An unknown stream name/ARN must fail with `ResourceNotFoundException` (HTTP 404), and an unresolvable/stale continuation token with `InvalidArgumentException` (400).

Every response must be computed from persisted state: no hardcoded payload, no echoed input, no random continuation token that is not stored and later honoured. Streams and fragments must survive across requests so that two sequential GetMedia calls on the same stream observe the same fragment sequence, and a producer-side write (PutMedia in the sibling kinesisvideo crate, or a test seeding helper on this crate's state) is visible to a subsequent GetMedia.

## State Model

In `src/state.rs` (`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `streams: HashMap<String /* stream name */, MediaStream>` plus `arn_index: HashMap<String /* StreamARN */, String /* stream name */>` so either identifier resolves to the same stream.
- `MediaStream { name: String, arn: String, content_type: String /* default "video/webm" */, fragments: Vec<Fragment>, next_fragment_number: u64 }`
- `Fragment { fragment_number: String /* decimal, monotonically increasing */, producer_timestamp: SystemTime, server_timestamp: SystemTime, chunk: Vec<u8> /* the MKV/opaque chunk bytes returned in Payload */ }`
- `continuations: HashMap<String /* token */, Continuation>` with `Continuation { stream_name: String, next_fragment_index: usize }` — tokens are minted on response and resolved on the next request so resumption is exact.

Resolution rules over this state: EARLIEST -> index 0; NOW -> `fragments.len()-1` (or len() when empty); FRAGMENT_NUMBER -> index after the match on `fragment_number` (404/400 when absent); PRODUCER_TIMESTAMP / SERVER_TIMESTAMP -> first fragment whose timestamp range contains `StartTimestamp`; CONTINUATION_TOKEN -> `continuations[token].next_fragment_index`.

Done = a client can seed/produce fragments for a stream, call GetMedia with EARLIEST and get those exact bytes back with the right `Content-Type`, resume with the returned continuation token and receive only the newer fragments, and receive `ResourceNotFoundException` for an unknown stream. Anything beyond this slice (persistent connection/streaming semantics, ConnectionLimitExceededException / ClientLimitExceededException throttling, InvalidEndpointException redirects, NotAuthorizedException auth checks) stays stubbed and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) lists exactly one operation: `GetMedia`, `POST /getMedia -> 200`, Input `StreamName`/`StreamARN`/`StartSelector` (required), Output `ContentType` (@httpHeader(Content-Type)) and `Payload` (blob, @httpPayload, required), Errors ClientLimitExceededException(400), ConnectionLimitExceededException(400), InvalidArgumentException(400), InvalidEndpointException(400), NotAuthorizedException(401), ResourceNotFoundException(404). The Smithy model's `StartSelector` shape defines `StartSelectorType` (required enum: FRAGMENT_NUMBER, SERVER_TIMESTAMP, PRODUCER_TIMESTAMP, NOW, EARLIEST, CONTINUATION_TOKEN), `AfterFragmentNumber`, `StartTimestamp`, `ContinuationToken`, and its docs state the companion-field requirements and that the continuation token resumes where the previous GetMedia ended. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/` matches `src/ops/get_media.rs:4`, proving the only handler is an unmodified 501 scaffold with zero state; no behavioral tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Seed a stream with three fragments (via the sibling kinesisvideo PutMedia path or a state-seeding test helper) under stream name `my-stream`.
2. POST /getMedia with {"StreamName":"my-stream","StartSelector":{"StartSelectorType":"EARLIEST"}} — expect 200, a `Content-Type` response header, and a Payload containing the seeded fragment bytes in order; actual: HTTP 501 NotImplemented.
3. Capture the continuation token the emulator returns and POST /getMedia again with {"StreamName":"my-stream","StartSelector":{"StartSelectorType":"CONTINUATION_TOKEN","ContinuationToken":"<token>"}} — expect only fragments newer than those already delivered (no duplicates, no gaps).
4. POST /getMedia with {"StreamName":"my-stream","StartSelector":{"StartSelectorType":"FRAGMENT_NUMBER"}} (missing AfterFragmentNumber) — expect InvalidArgumentException, HTTP 400.
5. POST /getMedia with {"StreamName":"no-such-stream","StartSelector":{"StartSelectorType":"NOW"}} — expect ResourceNotFoundException, HTTP 404.

Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the WorkMailMessageFlow emulator

_Filed: 2026-10-21T23:27:55Z_
_Operation: service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice | Class: stub_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole crate is a fresh scaffold: both modeled operations answer HTTP 501 `NotImplemented` and the service holds no state. Implement a first functional slice so a realistic WorkMail message-flow Lambda client works end-to-end.

This service has exactly 2 operations (restJson1, signing name `workmailmessageflow`), so the happy path IS the whole surface. It has no create/delete operation of its own: in-transit messages are created by WorkMail delivery, so the bootstrap must also provide a way to populate state (a seed helper on `ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState`, usable from tests and from any future cross-service wiring).

### Happy path (in order)

1. **Seed an in-transit message** — put a `messageId` (must satisfy the modeled label pattern `^[a-z0-9\-]*$`, len 1..120) and its raw MIME bytes into this service's store. Not an AWS operation; a state helper + a test fixture. Without it neither operation has anything to act on.
2. **GetRawMessageContent** — `GET /messages/{messageId}`. Input: `messageId` (required, @httpLabel). Output: `messageContent` (blob, @httpPayload, required) — must be the EXACT bytes currently stored for that id, streamed as the HTTP payload with no JSON envelope. Unknown id -> `ResourceNotFoundException` (HTTP 404).
3. **PutRawMessageContent** — `POST /messages/{messageId}`. Input: `messageId` (required, @httpLabel) and `content` (`RawMessageContent`, required) whose required member `s3Reference` is an `S3Reference{bucket (required, len 3..63, ^[a-z0-9][a-z0-9\-]*$), key (required, len 1..1024, ^[a-zA-Z0-9\-/]*$), objectVersion (optional)}`. Empty output body, HTTP 200. Must RESOLVE the S3 reference to bytes and REPLACE the stored MIME content for `messageId`, so that a subsequent GetRawMessageContent returns the new content, not the seeded content. Errors, all of which must be reachable: `ResourceNotFoundException` (404) for an unknown `messageId`; `InvalidContentLocation` (400) when the bucket/key (or `objectVersion`) cannot be resolved to an object; `MessageRejected` (400) when the referenced object is not usable MIME content (e.g. empty); `MessageFrozen` (400) when the message is no longer in transit / has been marked frozen.
4. **GetRawMessageContent again** — re-read and observe the content written in step 3. This round trip is the acceptance demo.

No canned or echoed responses: `GetRawMessageContent` must return bytes read out of the store (never a hardcoded MIME blob, never bytes derived from the request), and `PutRawMessageContent` must actually mutate the store rather than return an empty 200 and forget. A 200 from Put followed by a Get that still shows the old bytes is a failure of this issue.

## State Model

In `src/state.rs`, on the service state struct:

- `messages: HashMap<String, InTransitMessage>` — keyed by `messageId`.
- `struct InTransitMessage {`
  - `message_id: String` — the id as given on the URI label,
  - `content: Vec<u8>` — the raw MIME bytes returned verbatim by `GetRawMessageContent` as `messageContent`,
  - `frozen: bool` — when true, `PutRawMessageContent` returns `MessageFrozen` and `GetRawMessageContent` still succeeds,
  - `last_s3_reference: Option<S3Ref>` — the reference accepted by the most recent successful `PutRawMessageContent`, kept so the update is observable/debuggable,
  - `updated_at: SystemTime`
  `}`
- `struct S3Ref { bucket: String, key: String, object_version: Option<String> }` — stored as supplied; `object_version` must be retained (not dropped) when present.
- Content resolution for `PutRawMessageContent`: look the `S3Ref` up through the shared engine (the co-mounted S3 emulator state if reachable); otherwise consult a per-service `s3_objects: HashMap<(String, String, Option<String>), Vec<u8>>` fixture map that tests can seed. A reference that resolves to nothing -> `InvalidContentLocation`; a reference that resolves to empty/non-MIME bytes -> `MessageRejected`.

All state lives in the single shared engine reached via `crate::state::service_state(&*self.engine)` — never a freshly constructed engine inside a handler.

Done = an integration test performs seed -> Get (original bytes) -> Put (new S3 reference) -> Get (new bytes), plus the 404 path for an unknown `messageId` and the 400 `InvalidContentLocation` path for an unresolvable reference, with correct status codes and raw payload framing.

## Evidence

`services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/OPERATIONS.md` lists exactly 2 operations with the routes, members, constraints and errors cited above: `GetRawMessageContent` (`GET /messages/{messageId}` -> 200, output `messageContent` blob @httpPayload required, error `ResourceNotFoundException` 404) and `PutRawMessageContent` (`POST /messages/{messageId}` -> 200, inputs `messageId` + `content: RawMessageContent`, errors `InvalidContentLocation` 400, `MessageFrozen` 400, `MessageRejected` 400, `ResourceNotFoundException` 404). `smithy_query(shape="RawMessageContent,S3Reference")` confirms `RawMessageContent.s3Reference` is required and `S3Reference` requires `bucket` and `key` with optional `objectVersion`. Both handler files — `src/ops/get_raw_message_content.rs` (line 4 marker, line 151 `Err(crate::scaffolded::Scaffolded::not_implemented("GetRawMessageContent"))`) and `src/ops/put_raw_message_content.rs` — still carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and return 501, so no behavior exists and no test asserts any. README.md confirms protocol restJson1 and that stubs deliberately answer 501 rather than a hollow 200.

## Steps to Reproduce

1. Seed an in-transit message in the service store: messageId `00000000-0000-0000-0000-000000000001` with raw MIME bytes (e.g. `From: a@example.com\r\nSubject: original\r\n\r\nbody`).
2. `GET /messages/00000000-0000-0000-0000-000000000001` -> expect HTTP 200 with the seeded MIME bytes as the raw response payload. Today: HTTP 501 NotImplemented.
3. Seed a resolvable S3 object (bucket `mailbucket`, key `updated/message.mime`) containing different MIME bytes (`Subject: rewritten`).
4. `POST /messages/00000000-0000-0000-0000-000000000001` with body `{"s3Reference":{"bucket":"mailbucket","key":"updated/message.mime"}}` -> expect HTTP 200 with empty body. Today: HTTP 501.
5. `GET /messages/00000000-0000-0000-0000-000000000001` again -> expect the REWRITTEN bytes from step 4, proving the Put mutated persisted state.
6. `GET /messages/does-not-exist` -> expect HTTP 404 `ResourceNotFoundException`.
7. `POST /messages/00000000-0000-0000-0000-000000000001` with `{"s3Reference":{"bucket":"mailbucket","key":"missing/object"}}` -> expect HTTP 400 `InvalidContentLocation`.
8. `POST /messages/does-not-exist` with a valid reference -> expect HTTP 404 `ResourceNotFoundException`.

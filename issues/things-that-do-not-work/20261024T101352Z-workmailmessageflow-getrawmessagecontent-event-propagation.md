Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: c677c98df9a3a2556a69e92857be9faa7b7124d4

# No emulator-reachable producer puts a message in transit, so GetRawMessageContent can only ever answer 404

_Filed: 2026-10-24T10:13:52Z_
_Operation: GetRawMessageContent | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `c677c98df9a3a2556a69e92857be9faa7b7124d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

WorkMail Message Flow has no create operation of its own: an in-transit message exists because WorkMail's inbound/outbound email flow put it there, and a Lambda configured with a (synchronous) Run Lambda rule is handed the messageId and calls GetRawMessageContent / PutRawMessageContent on it.

Expected (AWS): for a messageId that WorkMail currently has in transit, `GET /messages/{messageId}` answers HTTP 200 with the raw MIME bytes as the HTTP payload (`messageContent`, @httpPayload streaming blob), and a frozen/redirected message answers PutRawMessageContent with HTTP 400 MessageFrozen.

Actual (emulator): the in-transit message store is only ever written by two in-process Rust helpers — `ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState::seed_in_transit_message` (documented in src/state.rs as "the producer this API does not model; it is NOT an AWS operation") and `replace_message_content`, which itself requires the message to already exist. No HTTP request to any mounted emulated service — WorkMail, SES, or this one — ever inserts a message. Consequently, for every out-of-process/SDK client:
  * GetRawMessageContent always returns HTTP 404 ResourceNotFoundException ("Unknown messageId: <id>") and its 200 success path is unreachable;
  * PutRawMessageContent always returns HTTP 404 for the same reason, so its MessageFrozen / InvalidContentLocation / MessageRejected paths are unreachable too (`frozen` can likewise only be set in-process);
  * the realistic message-flow workflow — message arrives in transit -> Lambda reads the raw content -> Lambda uploads rewritten MIME to S3 -> PutRawMessageContent installs it -> the re-read reports the rewritten bytes — cannot be exercised over the wire at all.

What is needed is a producer reachable from outside the process: the emulated WorkMail service (or SES receipt) placing a message in transit when mail enters a mailbox/email-flow rule, wired through the one shared engine the way other crates already read another service's store, so that the Get/Put round trip has a message to act on. The seed helper was explicitly scoped by the bootstrap issue as a test fixture "usable from tests and from any future cross-service wiring"; that wiring is still absent.

## Evidence

Smithy model: the service binds exactly two operations (GetRawMessageContent GET /messages/{messageId} -> 200 with required @httpPayload `messageContent`; PutRawMessageContent POST /messages/{messageId}) and models no create/delete, so in-transit messages must come from outside this API. PutRawMessageContent's own documentation describes the producer: "Updates to an in-transit message only appear when you call PutRawMessageContent from an AWS Lambda function configured with a synchronous Run Lambda rule" — i.e. WorkMail's email flow is what makes the message exist.

Handler/state: services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/state.rs lines 156-212 — `messages: HashMap<String, InTransitMessage>` is written only by `seed_in_transit_message` (lines 167-183, doc comment: "This is the producer this API does not model; it is NOT an AWS operation") and `replace_message_content` (lines 197-212, which returns false for an absent id). src/ops/get_raw_message_content.rs lines 142-156 return ResourceNotFoundException for any id not in that map; src/ops/put_raw_message_content.rs lines 174-186 and 239-249 do the same via `eligibility()`.

No cross-service producer exists: grep for `in_transit|message_flow|messageflow|InTransit` across services/service-com-amazonaws-workmail-workmailservice/src returns no matches, and this crate reads no other service's store (unlike e.g. services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/list_s3_buckets.rs:119, which reaches S3's own state through the shared engine).

Test coverage: tests/all/raw_message_content.rs reaches success only by calling the in-process helper (`Emulator::seed_message` -> `state.seed_in_transit_message`, lines 112-118); the test module header itself states "The store is reached only to SEED it — to stand in for WorkMail delivery, which puts a message in transit". No test (and no client) reaches a 200 through HTTP alone.

Not covered by the open issues: the S3Reference issue (20261024T015715Z) is about Put's object resolution and explicitly works around message existence by using the seed helper; the bucket-policy and IAM issues concern authorization/location checks, not message existence.

## Steps to Reproduce

1. Start the emulator with the WorkMail, S3 and WorkMailMessageFlow services mounted on the shared engine, and use only HTTP/SDK calls (no in-process state access).
2. Drive any sequence of emulated AWS calls intended to put mail into WorkMail's message flow (e.g. WorkMail organization/user/mailbox setup, or an SES receipt path).
3. Call GetRawMessageContent for the messageId such a flow would hand a Run Lambda rule (or any messageId at all).
4. Expected (AWS): HTTP 200 with the message's raw MIME bytes as the response payload for a message currently in transit.
5. Actual: HTTP 404 with x-amzn-errortype ResourceNotFoundException and body {"message":"Unknown messageId: <id>"} for every id, because nothing outside the process can insert into `state.messages`.
6. Follow with PutRawMessageContent for the same id and any valid s3Reference: expected one of 200 / MessageFrozen 400 / InvalidContentLocation 400 depending on state; actual always HTTP 404 ResourceNotFoundException, so MessageFrozen in particular is unreachable over the wire.

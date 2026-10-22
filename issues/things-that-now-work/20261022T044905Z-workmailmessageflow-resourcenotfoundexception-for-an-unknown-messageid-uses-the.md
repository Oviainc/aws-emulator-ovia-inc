Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workmailmessageflow.get-raw-message-content-unknown-id-not-found workmailmessageflow.put-raw-message-content-unknown-id-not-found workmailmessageflow.get-put-consistent-not-found-for-same-message-id workmailmessageflow.put-raw-message-content-id-not-found-precedes-content-location-validation workmailmessageflow.put-raw-message-content-with-object-version-still-not-found workmailmessageflow.synchronous-lambda-content-round-trip-unautomatable
Issue-Shape-Id: com.amazonaws.workmailmessageflow#GiraffeMessageInTransitService
Issue-Key: workmailmessageflow|GetRawMessageContent|ResourceNotFoundException
Issue-Key: workmailmessageflow|PutRawMessageContent|ResourceNotFoundException
Issue-Key: workmailmessageflow.get-put-consistent-not-found-for-same-message-id|run#5|AssertionError|assert get_error.get("Message") == f"Unknown messageId: {message_id}", (
Issue-Key: workmailmessageflow.put-raw-message-content-id-not-found-precedes-content-location-validation|run#7|AssertionError|assert error.get("Message") == f"Unknown messageId: {message_id}", (
Issue-Key: workmailmessageflow.put-raw-message-content-with-object-version-still-not-found|run#7|AssertionError|assert error.get("Message") == f"Unknown messageId: {message_id}", (
Issue-Key: workmailmessageflow.synchronous-lambda-content-round-trip-unautomatable|run#5|AssertionError|assert get_error.get("Message") == f"Unknown messageId: {message_id}", (
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# workmailmessageflow: ResourceNotFoundException for an unknown messageId uses the wrong message rule on both GetRawMessageContent and PutRawMessageContent

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRawMessageContent

## Description
Cause 1 of 1: the 404 ResourceNotFoundException both in-transit-message operations raise for a messageId that is not in transit is rendered from a different message rule than AWS's, on every path that raises it.

AWS: recorded behavior only — the Smithy model (services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/smithy-model/com-amazonaws-workmailmessageflow-giraffemessageintransitservice.json) declares GetRawMessageContent -> errors [ResourceNotFoundException, httpError 404] and PutRawMessageContent -> errors [InvalidContentLocation, MessageFrozen, MessageRejected, ResourceNotFoundException, httpError 404], and documents ResourceNotFoundException only as "The requested email message is not found." — it states no message text. The recorded responses do state the rule, and they state it identically across six independent recordings using six different messageIds (synthetic `sim2real-wmf-...` ids and a bare UUID alike): the message body is the fixed prefix `Unknown messageId: ` followed verbatim by the messageId from the request. The message is therefore built from the request, naming the offending member's value, so the rule — prefix plus the rejected messageId — is the requirement; the literal recorded strings below are only examples of it. HTTP status 404 and error code `ResourceNotFoundException` are correct in the emulator and are not at issue.

Emulator: both handlers build a different template, `The in-transit message <messageId> was not found.` — e.g. `The in-transit message sim2real-wmf-x-5cabbe533e2d-0b21d79406ff4151aa42c6fa8d23e4a6 was not found.` where AWS returns `Unknown messageId: sim2real-wmf-x-5cabbe533e2d-0b21d79406ff4151aa42c6fa8d23e4a6`. The same wrong rule is used for the read path, the write path, and the independent follow-up read after a rejected write. Everything else these checks assert holds: the code is ResourceNotFoundException on all three calls, a rejected PutRawMessageContent fabricates no visible state (the follow-up GetRawMessageContent still 404s), the messageId-existence check is evaluated before the S3 content-location validation (a real private bucket reference still yields ResourceNotFoundException rather than InvalidContentLocation), and an s3Reference carrying an objectVersion is treated no differently.

Why: a client that classifies or surfaces WorkMail in-transit-message failures by the error's message rule — log scrapers, Lambda email-flow handlers that distinguish "this id was never in transit" from other 404-shaped conditions, and any assertion written against AWS's documented sample responses — reads text that no real WorkMail Message Flow endpoint produces, so the emulator trains and validates clients against a message rule that fails against AWS.

Fix: the ResourceNotFoundException construction in each operation's handler — the not-found branch of GetRawMessageContent::handle and the Missing/refuse_missing branch of PutRawMessageContent::handle — must render the message from the AWS rule (the fixed prefix plus the request's messageId). No state, serializer or validation change is involved; the lookup, the ordering and the status/code are already right.

Traces: workmailmessageflow.get-raw-message-content-unknown-id-not-found; workmailmessageflow.put-raw-message-content-unknown-id-not-found; workmailmessageflow.get-put-consistent-not-found-for-same-message-id; workmailmessageflow.put-raw-message-content-id-not-found-precedes-content-location-validation; workmailmessageflow.put-raw-message-content-with-object-version-still-not-found; workmailmessageflow.synchronous-lambda-content-round-trip-unautomatable. Emulator source: services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/get_raw_message_content.rs lines 150-155 and services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/put_raw_message_content.rs lines 301-314.

## Evidence
Expected (recorded AWS, message rule = `Unknown messageId: ` + request messageId), for GetRawMessageContent, PutRawMessageContent and the follow-up read:
  HTTP 404, Error.Code = "ResourceNotFoundException",
  Error.Message = "Unknown messageId: sim2real-wmf-x-5cabbe533e2d-0b21d79406ff4151aa42c6fa8d23e4a6"
  (and, for the UUID-shaped id, "Unknown messageId: 8e220c71-0e24-437a-98b8-f83d6c64d1c8")

Actual (emulator): HTTP 404, Error.Code = "ResourceNotFoundException" (correct), but
  Error.Message = "The in-transit message sim2real-wmf-x-5cabbe533e2d-0b21d79406ff4151aa42c6fa8d23e4a6 was not found."
  Error.Message = "The in-transit message sim2real-wmf-x-314e2b2d646a-50269417832a45c9afc248c92ae523fd was not found."
  Error.Message = "The in-transit message sim2real-wmf-x-f0f1ff432be4-99acf73d47674ee8b0790175a8bb7306 was not found."
  Error.Message = "The in-transit message sim2real-wmf-x-3755a2a37685-521a038d176b4ae18963fbd6b1d3d02f was not found."
  Error.Message = "The in-transit message 8e220c71-0e24-437a-98b8-f83d6c64d1c8 was not found."
Six checks fail on exactly this assertion (AssertionError: unexpected message '...'), with ids differing per run — the divergence is the template, not any one id.

## Steps to Reproduce
1. Build and boot the emulator and point a botocore client at it (`ctx.client("workmailmessageflow")`).
2. Pick any syntactically valid, never-used messageId (pattern ^[a-z0-9\-]*$, length 1-120) — e.g. str(uuid.uuid4()).
3. Call GetRawMessageContent(messageId=<that id>). It raises ClientError; inspect exc.response["Error"]. Code is ResourceNotFoundException (correct) but Message is "The in-transit message <id> was not found." instead of "Unknown messageId: <id>".
4. Call PutRawMessageContent(messageId=<same id>, content={"s3Reference": {"bucket": "any-bucket", "key": "k"}}) — same wrong Message. Adding objectVersion to the s3Reference, or pointing it at a real private bucket, does not change it.
5. Call GetRawMessageContent(messageId=<same id>) again — the follow-up read shows the same wrong Message.
6. Replay: workmailmessageflow.get-raw-message-content-unknown-id-not-found, workmailmessageflow.put-raw-message-content-unknown-id-not-found, workmailmessageflow.get-put-consistent-not-found-for-same-message-id, workmailmessageflow.put-raw-message-content-id-not-found-precedes-content-location-validation, workmailmessageflow.put-raw-message-content-with-object-version-still-not-found, workmailmessageflow.synchronous-lambda-content-round-trip-unautomatable.

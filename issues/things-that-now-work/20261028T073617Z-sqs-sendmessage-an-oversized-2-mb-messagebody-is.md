Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.max-message-size
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.max-message-size|run#14|AssertionError|assert code == "413", f"expected a 413 (request entity too large) for a 2MB body, got {code}"
Issue-Commit: 6ef54a59763e785e9d514d65e30cf6b5980831b9

# SQS SendMessage: an oversized (2 MB) MessageBody is refused with InvalidParameterValue instead of a transport-level HTTP 413

> _Note: this issue was identified against the code base at commit `6ef54a59763e785e9d514d65e30cf6b5980831b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Cause 1 of 1: SendMessage does not enforce the HTTP request-entity size limit at the transport layer, so an oversized message body is refused by SQS field validation with an unmodeled error code instead of being refused before the operation is ever dispatched. Failing check: sqs.max-message-size.

AWS: SQS accepts a MessageBody up to the 262,144-byte (256 KiB) maximum and round-trips it byte-for-byte. A request whose entity is far above that (here a 2 MiB body) is rejected by the SQS HTTP front end at the transport layer, BEFORE any SQS field validation runs, with HTTP 413 (Request Entity Too Large). Because that refusal is a bare HTTP status and not an AWS error document, botocore surfaces it with Error.Code equal to the status string "413" and no AWS error code. The oversized message is not enqueued. Smithy (services/service-com-amazonaws-sqs-amazonsqs): SendMessage.MessageBody targets String with NO @length constraint, and the only content-related error SendMessage models is InvalidMessageContents (client, httpError 400); InvalidParameterValue is not a modeled SendMessage error at all, which is consistent with the oversize refusal being a transport-layer status rather than a modeled service error. The 413 status is the requirement; there is no fixed AWS error message text for it because the response carries no error document.

Emulator: the request is read in full and dispatched to the SendMessage handler, which refuses it from field validation with error code "InvalidParameterValue" (HTTP 400). The check observed Error.Code == 'InvalidParameterValue' where AWS gives '413'. The accepted-at-the-limit arm (262,144 bytes sent, received back unaltered) passes, so the divergence is confined to the oversize refusal class.

Why: a client cannot tell a transport-level payload-too-large refusal from an ordinary parameter-validation refusal. Code that keys on HTTP 413 to shrink/split a payload or to fall back to an out-of-band store (the pattern the Amazon SQS Extended Client Library implements), and any retry policy that treats 413 differently from a 400 InvalidParameterValue, takes the wrong branch. Generic error handling that switches on Error.Code also sees a code SQS never returns for SendMessage.

Fix: the SQS HTTP front end / request-reading layer must gate the request entity size and answer HTTP 413 with no AWS error document before the SendMessage operation handler and its parameter validation are reached; the SendMessage handler's MessageBody validation must not be the thing that rejects a request that exceeded the transport limit.

Traces: sqs.max-message-size (behavioral check).

## Evidence
Failing check sqs.max-message-size (verdict: mismatch).

Expected (AWS): send_message with MessageBody = 'x' * (2 * 1024 * 1024) is refused at the transport layer; botocore's exc.response['Error']['Code'] == '413' (HTTP 413 Request Entity Too Large, no AWS error document); the message is not delivered.

Actual (emulator): the call is refused by SQS field validation; exc.response['Error']['Code'] == 'InvalidParameterValue'.

Assertion from the replay:
  assert code == "413", f"expected a 413 (request entity too large) for a 2MB body, got {code}"
  AssertionError: expected a 413 (request entity too large) for a 2MB body, got InvalidParameterValue

Control arm that PASSED in the same check: a 262,144-byte body is accepted, delivered, and its Body comes back identical.

Model reference: Smithy SendMessage — MessageBody: String, required, no @length; modeled errors include InvalidMessageContents (400) but NOT InvalidParameterValue.

## Steps to Reproduce
1. sqs.create_queue(QueueName=<unique>) and note the QueueUrl.
2. sqs.send_message(QueueUrl=url, MessageBody='x' * (256 * 1024)) — this must succeed, and a subsequent receive_message must return the body byte-for-byte (this part already works).
3. sqs.send_message(QueueUrl=url, MessageBody='x' * (2 * 1024 * 1024)).
4. Catch the ClientError and read exc.response['Error']['Code'].
   AWS: '413' (HTTP 413, no AWS error document). Emulator: 'InvalidParameterValue' (HTTP 400).
5. receive_message on the queue: the oversized body must not be present (this holds in both).

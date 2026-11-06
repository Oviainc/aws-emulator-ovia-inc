Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.sqs-esm-message-attributes-keep-type-and-bytes lambda__sqs.function-sdk-send-to-unpermitted-queue-denied-as-function-error lambda__sqs.esm-filter-criteria-non-matching-not-invoked
Issue-Key: lambda__sqs.sqs-esm-message-attributes-keep-type-and-bytes|run#13|AssertionError|assert ma[name].get("stringValue") == want["StringValue"], (
Issue-Key: lambda__sqs.function-sdk-send-to-unpermitted-queue-denied-as-function-error|run#14|AssertionError|assert err == "Unhandled", f"the unpermitted SendMessage gave FunctionError {err!r}: {out!r}"
Issue-Key: lambda__sqs.esm-filter-criteria-non-matching-not-invoked|_collect#14|Timeout|resp = sqs.receive_message(
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# lambda__sqs: a Number message attribute is not normalized in the delivered ESM record, an unpermitted SendMessage succeeds, and the filter-criteria scenario never settles (3 causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Three net-new root causes. The open issue 20261105T022225Z-lambda-sqs-an-enabled-sqs-event-source-mapping.md carries another check of this suite (batch-item-failures without FunctionResponseTypes); it does not carry these replay keys.

Cause 1 of 3: a `Number` message attribute is not canonicalized to its minimal decimal form before it reaches the Lambda record.
AWS: SQS treats a Number message attribute as a number, not as the literal string the sender supplied: the delivered value is the canonical decimal of that number, so "0042.50" is delivered as "42.5" (leading zeros AND insignificant trailing zeros removed). Because the delivered attribute set differs from the sent one, md5OfMessageAttributes on the record is the digest of the NORMALIZED values and differs from SendMessage's MD5OfMessageAttributes. Smithy models the carrier (MessageAttributeValue{StringValue, BinaryValue, DataType}; SendMessage returns MD5OfMessageAttributes) but not the normalization rule, which is AWS's documented message-metadata behavior.
Emulator: only the leading zeros are dropped. The record carries stringValue "42.50" where AWS delivers "42.5" — a trailing insignificant zero is preserved, so the normalization is partial. The normalized-digest assertions that follow are built on the same value and cannot be reached.
Why: a consumer that compares an attribute against a canonical numeric string, or that validates md5OfMessageAttributes against its own digest of the delivered attributes, mismatches: the number of characters in the value differs from AWS and so does the MD5.
Fix: the Number-attribute normalization in SQS's message-attribute handling (applied where the attribute is stored/serialized for delivery) and the md5OfMessageAttributes computed over the normalized set in the ESM record builder.
Traces: lambda__sqs.sqs-esm-message-attributes-keep-type-and-bytes.

Cause 2 of 3: an in-function SendMessage to a queue the role does not permit succeeds, and the invoke reports no FunctionError.
AWS: the handler's call is authorized by IAM; with no sqs:SendMessage Allow for that queue it is refused HTTP 403 `AccessDenied` with "User: <principal> is not authorized to perform: sqs:SendMessage on resource: <queue ARN> because no identity-based policy allows the sqs:SendMessage action". The raise propagates out of the handler, so Invoke answers with the x-amz-function-error / FunctionError header set ("Unhandled") and the error payload. SQS's model does not enumerate the authorization error; this is AWS's documented IAM behavior.
Emulator: the send is accepted and the invoke is clean — "the unpermitted SendMessage gave FunctionError None: {'messageId': '<uuid>'}" — i.e. a message is enqueued on a queue no policy allows and the caller gets a success.
Why: the blast radius of a function is unbounded in the emulator; a negative test asserting the FunctionError passes here and fails in AWS, and writes that AWS rejects are visible downstream.
Fix: the SQS SendMessage handler's authorization check plus the execution-role caller-identity plumbing for in-process handler calls.
Traces: lambda__sqs.function-sdk-send-to-unpermitted-queue-denied-as-function-error.

Cause 3 of 3: the ESM filter-criteria scenario never reaches a verdict.
AWS: an SQS event source mapping with FilterCriteria drops messages whose body does not match the pattern — they are deleted from the queue without invoking the function, while a matching message does invoke it. Smithy: CreateEventSourceMapping carries FilterCriteria{Filters[].Pattern}.
Emulator: "Error: Check timed out after 60.0s" — the check is killed before asserting, so neither the drop of the non-matching message nor the invocation for the matching one is observable.
Why: filtering is what keeps a consumer from seeing traffic it must ignore; if the scenario never settles, a wrong or absent filter cannot be detected.
Fix: the SQS event-source-mapping runtime's filter-criteria evaluation.
Traces: lambda__sqs.esm-filter-criteria-non-matching-not-invoked.

## Evidence
Cause 1 — sent MessageAttributes include {"amount": {"DataType": "Number", "StringValue": "0042.50"}}. Expected in the delivered record: dataType "Number", stringValue "42.5" (and md5OfMessageAttributes = digest of the normalized set, different from SendMessage's). Actual: "AssertionError: amount: stringValue '42.50' != '42.5'".

Cause 2 — expected: HTTP 403 AccessDenied inside the handler and FunctionError set on the Invoke response. Actual: "AssertionError: the unpermitted SendMessage gave FunctionError None: {'messageId': 'b16b4e31-...'}" (message id is a per-run value).

Cause 3 — expected: a non-matching message never invokes the function while a matching one does. Actual: "Error: Check timed out after 60.0s" — no verdict.

## Steps to Reproduce
1. Cause 1: SendMessage with MessageAttributes {"text": String, "amount": {Number, "0042.50"}, "blob": Binary, "label": "String.<suffix>"} to a queue with an SQS event source mapping (BatchSize=1); have the handler forward the record and inspect record.messageAttributes.amount.stringValue — it is "42.50", not "42.5".
2. Cause 2: invoke a function whose role has no sqs:SendMessage for the target queue and whose handler sends to it. The invoke returns a messageId with FunctionError unset.
3. Cause 3: create a mapping with FilterCriteria and send a non-matching message. The check is killed at 60 s with no verdict.
4. Replay the three check ids in trace_ids.

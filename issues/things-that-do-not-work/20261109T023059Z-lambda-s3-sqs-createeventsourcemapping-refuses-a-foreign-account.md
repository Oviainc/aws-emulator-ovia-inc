Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.esm-event-source-arn-type-and-account-rejected
Issue-Key: lambda__s3__sqs.esm-event-source-arn-type-and-account-rejected|run#14|AssertionError|foreign[0] == "InvalidParameterValueException"
Issue-Commit: 81c00d0cdf8c8d837a6edaf703850a5ccd4fe73e

# lambda__s3__sqs: CreateEventSourceMapping refuses a foreign-account SQS queue ARN with an emulator-authored ownership sentence instead of AWS's ReceiveMessage/InvalidAddress refusal naming the queue URL

> _Note: this issue was identified against the code base at commit `81c00d0cdf8c8d837a6edaf703850a5ccd4fe73e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One root cause.

Cause 1 of 1: the refusal text for an EventSourceArn whose account field is not the caller's account.

AWS: Lambda does not refuse a cross-account SQS EventSourceArn on an account-field comparison. It resolves the ARN to a queue address and probes it, and surfaces the SQS failure it got back verbatim. Smithy (services/service-com-amazonaws-lambda-awsgirapiservice): CreateEventSourceMapping input member EventSourceArn -> Arn, @pattern ^arn:(aws[a-zA-Z0-9-]*):([a-zA-Z0-9\-])+:((eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1})?:(\d{12})?:(.*)$; modeled error InvalidParameterValueException, errorKind client, httpError 400. The model does not state the message, so its wording is recorded behavior only: HTTP 400, code InvalidParameterValueException, message of the fixed form
  Error occurred while ReceiveMessage. SQS Error Code: InvalidAddress. SQS Error Message: The address <queue URL> is not valid for this endpoint.
The fixed text is everything outside <queue URL>: the literal prefix "Error occurred while ReceiveMessage. SQS Error Code: InvalidAddress. SQS Error Message: The address " and the literal suffix " is not valid for this endpoint.". The one variable part is the address, derived from the request's ARN: an http(s) URL whose host is environment (never a requirement) and whose path is /<account-id from the ARN>/<queue name from the ARN>. The recorded check asserts exactly that — prefix, suffix, and that the span between them parses as an http(s) URL with that path.

Emulator: answers code InvalidParameterValueException (so the code and class are right) with a different sentence entirely — "The event source '<event source arn>' belongs to account '<account from the ARN>' rather than to account '<caller account>', which this request is made in". The account id and the role of each clause are the template; the literal ids in one run are not. Nothing in the message is an address, so the ARN's account field is compared but never resolved to a queue URL.

Why: a client or operator reading this refusal is told the mapping was rejected for ownership, when AWS tells it the queue *address* the ARN resolves to is not serviceable from this endpoint — a different diagnosis, and the only one that names the address to go look at. Tooling that matches AWS's wording, or that parses the queue URL out of the message to report which address failed, gets nothing to parse.

Fix: the SQS event-source resolution/validation inside the lambda CreateEventSourceMapping handler. The ARN must be resolved to a queue address and the ReceiveMessage-probe refusal emitted from that resolution; the current account-equality short-circuit and its hand-written sentence are the defect. The status (400) and the error code are already correct — only the message body and the path that produces it change.

Related open issues cover OTHER CreateEventSourceMapping refusal texts and do not carry this check: 20261030T110325Z-lambda-s3-sqs-createeventsourcemapping-s-unauthorized-execution-role.md (unauthorized execution role), 20261030T113113Z-lambda-sns-sqs-createeventsourcemapping-s-sqs-refusal-messages.md (nonexistent queue reported as a permission failure), 20261103T022510Z-dynamodb-lambda-lambda-createeventsourcemapping-the-unrecognized-event-source-and-missing.md (unrecognized event source). This foreign-account case is net-new.

Traces: the failing check below is an example of it; the same wrong sentence is produced for any EventSourceArn whose 12-digit account field differs from the caller's.

## Evidence
Check lambda__s3__sqs.esm-event-source-arn-type-and-account-rejected, assertion at check_lambda__s3__sqs_esm_event_source_arn_type_and_account_rejected.py line 394.

The check calls CreateEventSourceMapping(EventSourceArn='arn:aws:sqs:<region>:111122223333:<queue name>', FunctionName=<fn>, BatchSize=1, Enabled=True) where 111122223333 is a fabricated account and <queue name> is a queue that really exists in the caller's account.

Expected (AWS): ('InvalidParameterValueException', 'Error occurred while ReceiveMessage. SQS Error Code: InvalidAddress. SQS Error Message: The address <http(s) queue URL with path /111122223333/<queue name>> is not valid for this endpoint.')

Actual (emulator): ('InvalidParameterValueException', "The event source 'arn:aws:sqs:us-west-2:111122223333:<queue name>' belongs to account '111122223333' rather than to account '<caller account>', which this request is made in")

AssertionError reported by the suite: "a queue ARN in another account must be refused with InvalidParameterValueException Error occurred while ReceiveMessage. SQS Error Code: InvalidAddress. SQS Error Message: The address <queue URL>' is not valid for this endpoint.', got ('InvalidParameterValueException', \"The event source '...' belongs to account '111122223333' rather than to account '...', which this request is made in\") -- accepting it means the account field of the ARN was never resolved"

The check's follow-up assertion (same block) additionally requires the span between prefix and suffix to urlsplit into an http(s) URL with path /111122223333/<queue name>; the emulator's message has no URL at all, so that is unreachable.

## Steps to Reproduce
1. Create an SQS queue Q in the caller's account and region, and a Lambda function F whose role grants sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on Q.
2. Call lambda CreateEventSourceMapping with FunctionName=F, BatchSize=1, Enabled=True and EventSourceArn='arn:aws:sqs:<region>:111122223333:<name of Q>' (same queue name, a foreign 12-digit account id).
3. Observe the ClientError: code is InvalidParameterValueException (correct) but the message is the ownership sentence above instead of 'Error occurred while ReceiveMessage. SQS Error Code: InvalidAddress. SQS Error Message: The address <queue URL> is not valid for this endpoint.'
4. Try to parse a queue URL out of the message (the recorded check does): there is none.
5. Confirm no mapping was created with ListEventSourceMappings(FunctionName=F) — the effect-absent half is already correct.

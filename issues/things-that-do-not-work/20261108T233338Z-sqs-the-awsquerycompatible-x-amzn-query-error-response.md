Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.query-compatible-error-carriers-differ-by-consumer-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.query-compatible-error-carriers-differ-by-consumer-rejected|run#59|AssertionError|assert headers_off.get("x-amzn-query-error") == f"{_LEGACY_ABSENT};Sender", (
Issue-Commit: cd606b66f64ef9b01b93d18944e298a6806ba5b8

# sqs: the awsQueryCompatible x-amzn-query-error response header is emitted only when the request opts in with x-amzn-query-mode, so a client that does not send the opt-in gets no legacy Query error code

> _Note: this issue was identified against the code base at commit `cd606b66f64ef9b01b93d18944e298a6806ba5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueUrl

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Context: issue 20261029T180649Z-sqs-awsquerycompatible-error-envelope-is-not-emitted-no.md (status: fixed) made the SQS error serializer emit the awsQueryCompatible envelope at all. That fix, however, gated the header on the REQUEST carrying `x-amzn-query-mode: true`; the behavior below is what is left over, and it is not a refusal regression of that fix (the call is still refused 400 with the right code and message), so it is filed as a new defect in the same component rather than a reopen.

Cause 1 of 1: `x-amzn-query-error` is withheld on an error response whose request did not carry the `x-amzn-query-mode` opt-in header. Failing check: sqs.query-compatible-error-carriers-differ-by-consumer-rejected.

AWS: SQS is an awsJson1_0 service carrying the awsQueryCompatible protocol modifier (Smithy: com.amazonaws.sqs#AmazonSQS, version 2012-11-05, protocols ['awsJson1_0'], protocol_modifiers ['awsQueryCompatible']). Every error response SQS returns carries BOTH error vocabularies unconditionally, independent of anything in the request: the response header `x-amzn-query-error: <legacy AWS-Query code>;<Fault>` and the body member `__type` holding the fully qualified modern shape id. The Fault field is the fixed text `Sender` for a client error and `Receiver` for a server error; the legacy code is the error shape's @awsQueryError code where the model gives one (Smithy: GetQueueUrl errors include QueueDoesNotExist, httpError 400, awsQueryError code `AWS.SimpleQueueService.NonExistentQueue`) and the shape's own name where the model defaults the binding. For the unmodeled Coral framework validation refusals the legacy code is `InvalidParameterValue` / `MissingParameter` (recorded behavior only). Nothing in the model makes either carrier conditional on a request header: `x-amzn-query-mode` is a REQUEST-side opt-in a client (botocore) sends to say which vocabulary it will PREFER, not a switch that turns the response carrier on. Measured against real SQS, the header is present on the refusal of a request that sends no `x-amzn-query-mode` at all.

Emulator: the same refusal, issued without `x-amzn-query-mode` on the request, answers 400 with a correct body (`__type` = `com.amazonaws.sqs#QueueDoesNotExist`, message `The specified queue does not exist.`) but NO `x-amzn-query-error` header. The identical call WITH `x-amzn-query-mode: true` does carry `x-amzn-query-error: AWS.SimpleQueueService.NonExistentQueue;Sender` — so the emulator is keying the response envelope off the request header.

Why: a client that does not send the opt-in cannot recover the legacy code at all. Concretely, the AWS SDK for JavaScript v3 does not send `x-amzn-query-mode`, and for an error SQS does not model as a shape (e.g. SendMessage with DelaySeconds=901, whose modern code `InvalidParameterValueException` has no generated class) v3 falls back to `x-amzn-query-error` to name the error. With the header withheld, v3 reports the MODERN code where real AWS has it report the LEGACY `InvalidParameterValue`, so v3 and botocore disagree about one and the same response and a `catch` branch keyed on the legacy code becomes dead code. Older SDK majors and the CLI's query-compat path lose the error code entirely and degrade the refusal to a generic 400, killing their per-error retry/classify branches.

Fix: the SQS crate's error-response serializer — the single place that renders an error into status + body + headers, the same component the earlier fix touched — must emit `x-amzn-query-error: <legacy code>;<Sender|Receiver>` on EVERY error it renders, with no dependence on the inbound request's `x-amzn-query-mode` header (which must keep having no effect on the response). Not per-operation handler work: the handlers pick the right error class here.

Traces: sqs.query-compatible-error-carriers-differ-by-consumer-rejected (the opt-in-present half of the same check passes, which localizes the defect to the conditioning, not to the header's value).

## Evidence
Failing check: [mismatch] sqs.query-compatible-error-carriers-differ-by-consumer-rejected

AssertionError, verbatim: "SQS omitted x-amzn-query-error when the request carried no x-amzn-query-mode; measured, it sends it regardless. headers {'content-type': 'application/x-amz-json-1.0', 'content-length': '96', 'date': 'Sun, 08 Nov 2026 22:37:50 GMT'}"

Expected (real AWS), for a raw signed POST of AmazonSQS.GetQueueUrl on an absent queue sent WITHOUT `x-amzn-query-mode`:
  HTTP 400
  header  x-amzn-query-error: AWS.SimpleQueueService.NonExistentQueue;Sender
  body    __type: com.amazonaws.sqs#QueueDoesNotExist, message: "The specified queue does not exist."
Fixed text in that header value: the legacy code `AWS.SimpleQueueService.NonExistentQueue`, the `;` separator and the fault `Sender`; nothing in it is taken from the request or from state. The queue name in the request and the `date` / `content-length` headers above are per-run values and are not part of the requirement.

Actual (emulator), same request: HTTP 400 with the correct body, and the response header set shown above — `x-amzn-query-error` is literally absent, not merely different.

Control within the same check: the immediately preceding identical request sent WITH `x-amzn-query-mode: true` asserted `x-amzn-query-error == AWS.SimpleQueueService.NonExistentQueue;Sender`, `__type == com.amazonaws.sqs#QueueDoesNotExist` and the absence of `x-amzn-errortype`, and all three passed. The two calls differ only in that request header, so the header's value and the body envelope are right and only the condition under which the header is emitted diverges.

Downstream assertions the check never reached (they run after this one) are the JS v3 consumer's: that for DelaySeconds=901 v3 reports name `InvalidParameterValue` (the legacy code, read from `x-amzn-query-error`) and constructs `SQSServiceException`, converging with botocore's `Error.Code`. v3 sends no `x-amzn-query-mode`, so the same conditioning is what will break them.

## Steps to Reproduce
1. Create a queue through SQS, and pick a queue name that does not exist.
2. Send a raw, SigV4-signed POST to the SQS endpoint with headers `content-type: application/x-amz-json-1.0` and `x-amz-target: AmazonSQS.GetQueueUrl`, body {"QueueName": "<the absent name>"}, and WITHOUT any `x-amzn-query-mode` header.
3. Inspect the raw response headers: the response is 400 with `__type` = `com.amazonaws.sqs#QueueDoesNotExist`, but carries no `x-amzn-query-error`. Real SQS returns `x-amzn-query-error: AWS.SimpleQueueService.NonExistentQueue;Sender` here.
4. Repeat the identical request with `x-amzn-query-mode: true` added: the header now appears with the correct value — confirming the response envelope is keyed off the request header.
5. Also check an unmodeled refusal: raw AmazonSQS.SendMessage on the existing queue with DelaySeconds=901, without the opt-in header; AWS answers 400 with `x-amzn-query-error: InvalidParameterValue;Sender` and `__type` = `com.amazon.coral.service#InvalidParameterValueException`.
6. Re-replay sqs.query-compatible-error-carriers-differ-by-consumer-rejected; it must pass end to end, including the JS v3 consumer's legacy-code fallback assertions.

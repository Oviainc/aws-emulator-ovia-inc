Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-queue-tags-and-permissions-management
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-queue-tags-and-permissions-management sqs-dead-letter-queue-redrive-configuration
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|CreateQueue|2xx->2xx|$.QueueUrl
Issue-Key: sqs|ListDeadLetterSourceQueues|2xx->2xx|$.queueUrls[*]
Issue-Commit: 29f1e00a7002e451c541eb91ee53a740b213bc0d

# SQS queue URLs are emitted with the emulator listener host instead of the AWS endpoint form (2 causes)

> _Note: this issue was identified against the code base at commit `29f1e00a7002e451c541eb91ee53a740b213bc0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQueue

## Description
Every SQS response member that carries a queue URL is serialized from the emulator's own data-listener socket (scheme + loopback host + ephemeral port) instead of from the service's regional endpoint. Both causes below are the same serializer: the queue-URL builder substitutes the bound listener address for the SQS endpoint, so the path part (/<accountId>/<queueName>) is right while the authority part is wrong. 2 causes.

Cause 1 of 2 — CreateQueue.QueueUrl (fixture sqs-queue-tags-and-permissions-management, step setup#1).
AWS: recorded behavior only — the Smithy model documents CreateQueueResult.QueueUrl only as "The URL of the created Amazon SQS queue" (CreateQueue output member QueueUrl, target String) and states nothing about its form. The recording shows AWS returning the regional SQS endpoint form: scheme https, host sqs.<region>.amazonaws.com, then /<accountId>/<queueName> (recorded example: https://sqs.us-west-2.amazonaws.com/000000000000/<queueName>; the region, account id and queue name are values taken from the request and the caller's credentials, the sqs.<region>.amazonaws.com host and the two-segment path are the fixed part).
Emulator: CreateQueue answers 200 with the correct shape but QueueUrl built from the listener socket — https://127.0.0.1:<port>/<accountId>/<queueName> (recorded actual: https://127.0.0.1:40085/000000000000/<queueName>). The announced port is also not the port the request was served on (the health probe reports the data listener on a different port), so the URL names an address the emulator is not reached on.
Why: a client that stores the returned QueueUrl, derives the queue ARN from it, embeds it in a RedrivePolicy/queue policy Resource, or compares it against a GetQueueUrl/ListQueues result gets a value whose host and port are neither AWS's nor stable across boots; anything that parses the URL to recover region or account (every SDK's ARN/region inference, and IAM policy text built from it) recovers the wrong region and no account-bearing host.
Fix: the queue-URL construction used by the CreateQueue handler's serializer (it must render the service endpoint for the request's region/account rather than the bound socket address).
Traces: sqs-queue-tags-and-permissions-management#setup#1.

Cause 2 of 2 — ListDeadLetterSourceQueues.queueUrls (fixture sqs-dead-letter-queue-redrive-configuration, step task#7).
AWS: recorded behavior only — ListDeadLetterSourceQueues output member queueUrls (required, QueueUrlList, xmlName QueueUrl, flattened) is documented only as the list of source queue URLs. The recording shows each element in the same regional endpoint form, https://sqs.<region>.amazonaws.com/<accountId>/<queueName>, identical to what CreateQueue returned for that queue.
Emulator: the operation answers 200 with the right number of elements, but each element is https://127.0.0.1:<port>/<accountId>/<queueName> (recorded actual element: https://127.0.0.1:40225/000000000000/<sourceQueueName>), so the listing's URLs do not even match the host:port of the listener serving it.
Why: the dead-letter source listing is normally joined against URLs the client already holds; with a per-boot authority the join fails and a redrive consumer cannot identify its own source queues.
Fix: same queue-URL construction, reached from the ListDeadLetterSourceQueues handler's serializer — the list elements must be rendered by the one shared endpoint-aware builder, not per-handler from the socket.
Traces: sqs-dead-letter-queue-redrive-configuration#task#7.

Neither cause is covered by an existing open issue: the three open SQS wire issues from the previous sweep cover the awsQueryCompatible error envelope (20261029T180649Z-sqs-awsquerycompatible-error-envelope-is-not-emitted-no.md), the error class/status/Reason wording of validation refusals (20261029T180649Z-sqs-validation-refusals-carry-the-wrong-error-class.md) and queue-attribute state/response payloads (20261029T180649Z-sqs-queue-attribute-state-and-response-payloads-diverge.md); none of them owns these two steps (the replay reports both with an empty owned_by).

## Evidence
Replay of simparity-wire-aws-v1 (25 fixtures, 254 steps, 208 passed, 46 mismatched):

Cause 1 — divergence index 20, service sqs, CreateQueue, expected 200 / actual 200:
  $.QueueUrl: value_mismatch (expected 'https://sqs.us-west-2.amazonaws.com/000000000000/sim2real-probe-sqs-tagged-queue-c2f83f85', got 'https://127.0.0.1:40085/000000000000/sim2real-probe-sqs-tagged-queue-c2f83f85')
  fixture sqs-queue-tags-and-permissions-management, step setup#1, owned_by: [] , stub_path: ''
  Fixed part of the expected value: scheme https, host sqs.<region>.amazonaws.com, path /<accountId>/<queueName>. Variable part: region (us-west-2), account id (000000000000) and the queue name from the CreateQueue request. The emulator replaces the fixed host with its own 127.0.0.1:<ephemeral port> and keeps the path.

Cause 2 — divergence index 9, service sqs, ListDeadLetterSourceQueues, expected 200 / actual 200:
  $.queueUrls[0]: expected 'https://sqs.us-west-2.amazonaws.com/000000000000/sim2real-probe-sqs-source-queue-e903721d', got 'https://127.0.0.1:40225/000000000000/sim2real-probe-sqs-source-queue-e903721d'
  fixture sqs-dead-letter-queue-redrive-configuration, step task#7, owned_by: [] , stub_path: ''

Smithy (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json): CreateQueue -> CreateQueueResult.QueueUrl: String, doc 'The URL of the created Amazon SQS queue.'; ListDeadLetterSourceQueues -> ListDeadLetterSourceQueuesResult.queueUrls: QueueUrlList, required. The model constrains neither URL's form, so the recorded regional-endpoint form is the requirement (recorded behavior only).

Note the two actual authorities differ from each other and from the emulator's reported data listener port, confirming the value is taken from the bound socket rather than from a configured endpoint.

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 against it.
3. Cause 1: replay fixture sqs-queue-tags-and-permissions-management step setup#1 (CreateQueue with a queue name). Observe 200 with $.QueueUrl whose authority is the emulator's loopback host:port instead of sqs.<region>.amazonaws.com; the path /<accountId>/<queueName> is correct.
4. Cause 2: replay fixture sqs-dead-letter-queue-redrive-configuration step task#7 (ListDeadLetterSourceQueues on the DLQ after a source queue has been wired to it with a RedrivePolicy). Observe 200 with $.queueUrls[0] carrying the same loopback authority.
5. Fixed, both steps return the regional endpoint form https://sqs.<region>.amazonaws.com/<accountId>/<queueName> for the request's region and account.

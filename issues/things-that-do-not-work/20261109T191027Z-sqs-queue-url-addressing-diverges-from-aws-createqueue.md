Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-batch-send-receive-delete-and-purge
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-batch-send-receive-delete-and-purge sqs-queue-create-url-shape sqs-single-message-round-trip sqs-queue-attributes-and-url-address-validation
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|CreateQueue|2xx->2xx|$.QueueUrl
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|$.__type
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|$.message
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|status
Issue-Commit: 9354063c6b655c6b4143ce2300841d23d0deff44

# sqs: queue-URL addressing diverges from AWS — CreateQueue publishes an unserved real-AWS host, and a path-less queue URL is refused QueueDoesNotExist instead of InvalidAddress

> _Note: this issue was identified against the code base at commit `9354063c6b655c6b4143ce2300841d23d0deff44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQueue

## Description
Two net-new root causes in the sqs queue-URL addressing layer: the url the emulator MINTS for a queue, and the refusal it gives for a url it cannot resolve to one. Both must be closed.

Cause 1 of 2 — CreateQueue answers a QueueUrl on a host the emulator does not serve.
AWS: Smithy CreateQueue -> CreateQueueResult.QueueUrl (String); the model does not constrain the url's form, so this is recorded behavior plus the endpoint contract: the QueueUrl AWS returns is an address on the endpoint the request was made against, and it is the ONLY handle every later queue-addressed operation (GetQueueUrl, GetQueueAttributes, SetQueueAttributes, SendMessage, ReceiveMessage, DeleteMessage, PurgeQueue, DeleteQueue) takes as input. A client is required to send the returned string straight back, so whatever host it names must be the host that serves the queue.
Emulator: the url is composed from the region and the queue name alone — a literal production endpoint of the form https://sqs.<region>.amazonaws.com/<account-id>/<queue-name> — with no reference to the endpoint the request arrived on. The replay comparator flags this as host_not_own: the returned QueueUrl's host is not an address this emulator answers on, even though the path part matches the recording.
Why: a client that does the normal thing — keep the QueueUrl from CreateQueue and pass it to the next call — is pointed at real AWS instead of the emulator. Depending on egress it either leaves the sandbox entirely (signing against a real account), or fails name resolution/connect, and in neither case does it reach the queue that was just created. Every create-then-use flow (SDK, CLI, IaC, worker bootstrap) breaks even though CreateQueue itself reported 200.
Fix: the CreateQueue handler's url construction — the shared queue-url builder must derive scheme/host/port from the request's own endpoint (the Host header / configured data listener) and keep only the /<account-id>/<queue-name> path from state, so a minted url is always an address the emulator serves. GetQueueUrl and any other operation that echoes a queue url must use the same builder.

Cause 2 of 2 — a queue url that names no queue path is refused as a nonexistent queue instead of an invalid address.
AWS: Smithy GetQueueAttributes declares BOTH InvalidAddress (com.amazonaws.sqs#InvalidAddress, httpError 404, awsQueryError code InvalidAddress, httpResponseCode 404) and QueueDoesNotExist (httpError 400, awsQueryError code AWS.SimpleQueueService.NonExistentQueue). They classify two different faults: InvalidAddress is for a QueueUrl that is not a valid address for the endpoint at all (per the recording, a url with no /<account-id>/<queue-name> path), QueueDoesNotExist is for a well-formed address whose queue is absent. The recorded refusal is HTTP 404, __type com.amazon`aws.sqs#InvalidAddress, x-amzn-query-error 'InvalidAddress;Sender', message 'The address <queue-url> is not valid for this endpoint.' — fixed text 'The address ' + ' is not valid for this endpoint.', with the one variable part being the QueueUrl exactly as the request supplied it.
Emulator: the same request is answered HTTP 400, __type 'com.amazonaws.sqs#QueueDoesNotExist', x-amzn-query-error 'AWS.SimpleQueueService.NonExistentQueue;Sender', message 'The specified queue does not exist.' — i.e. the address fault is funnelled into the lookup-miss path.
Why: the status (404 vs 400), the error code and the message all differ, so a client cannot distinguish 'you built the url wrong / pointed at the wrong endpoint' from 'the queue is gone'. Retry and error-classification logic keyed on InvalidAddress never triggers, and bootstrap code that creates-on-NonExistentQueue will instead loop creating queues for a url it will never be able to address.
Fix: the queue-url parsing/validation shared by the queue-url-addressed operations (GetQueueAttributes' validation, before the state lookup): a QueueUrl that does not parse into an account-id + queue-name path for this endpoint must raise the modeled InvalidAddress with AWS's wording, and only a parsed-but-absent queue may raise QueueDoesNotExist.

Related but NOT covering these: open issue 20261109T185037Z-sqs-5-net-new-causes-the-query-platform... (sqs: '5 net-new causes: the Query protocol does not answer XML, ...') touches CreateQueue only for stale attribute comparison, and 20261104T125535Z-sqs-getqueueattributes-refuses-an-existing-queue-with-400.md covers GetQueueAttributes on an EXISTING queue; neither carries the replay keys below.

Traces: Cause 1 — sqs-batch-send-receive-delete-and-purge#setup#1 (also sqs-queue-create-url-shape#task#1, sqs-single-message-round-trip#setup#1); source: services/service-com-amazonaws-sqs-amazonsqs/src/ops/create_queue.rs:428 (`state::queue_url(&region, &input.queue_name)`) and the builder it calls in src/state.rs. Cause 2 — sqs-queue-attributes-and-url-address-validation#task#3.

## Evidence
Cause 1 (CreateQueue, expected 200 / actual 200, key $.QueueUrl): divergence detail '$.QueueUrl: host_not_own (expected https://sqs.us-west-2.amazonaws.com/<account>/<queue-name>, got https://sqs.us-west-2.amazonaws.com/<account>/<queue-name>)' — the path matches the recording, but the HOST is a production sqs endpoint rather than the endpoint the emulator is reached on (the replay harness reached the data listener on its own host/port). divergence_key service=sqs|CreateQueue|status=200->200|keys=$.QueueUrl; affected_count 3 (sqs-batch-send-receive-delete-and-purge#setup#1, sqs-queue-create-url-shape#task#1, sqs-single-message-round-trip#setup#1).

Cause 2 (GetQueueAttributes with a queue url carrying no account/queue path):
  expected: HTTP 404; $.__type 'com.amazonaws.sqs#InvalidAddress'; $headers.x-amzn-query-error 'InvalidAddress;Sender'; $.message 'The address <queue-url> is not valid for this endpoint.'
  actual:   HTTP 400; $.__type 'com.amazonaws.sqs#QueueDoesNotExist'; $headers.x-amzn-query-error 'AWS.SimpleQueueService.NonExistentQueue;Sender'; $.message 'The specified queue does not exist.'
divergence_key service=sqs|GetQueueAttributes|status=404->400|keys=$.__type,$.message,$headers.x-amzn-query-error,status.

Smithy citations: CreateQueue output member QueueUrl: String; GetQueueAttributes errors include InvalidAddress (httpError 404, awsQueryError {code: InvalidAddress, httpResponseCode: 404}) and QueueDoesNotExist (httpError 400, awsQueryError {code: AWS.SimpleQueueService.NonExistentQueue, httpResponseCode: 400}).

## Steps to Reproduce
Cause 1: replay fixture sqs-batch-send-receive-delete-and-purge, step setup#1 (or sqs-queue-create-url-shape#task#1). Equivalently: POST CreateQueue with a QueueName to the emulator's data endpoint and read CreateQueueResult.QueueUrl — its scheme/host/port must be the endpoint the request was sent to, not sqs.<region>.amazonaws.com. Then pass that url straight back to GetQueueAttributes over the same connection.

Cause 2: replay fixture sqs-queue-attributes-and-url-address-validation, step task#3. Equivalently: call GetQueueAttributes with QueueUrl set to an endpoint root with no /<account-id>/<queue-name> path (e.g. 'https://sqs.us-west-2.amazonaws.com/') and AttributeNames=[All] — expect HTTP 404 InvalidAddress with 'The address <that url> is not valid for this endpoint.' and x-amzn-query-error 'InvalidAddress;Sender'; the emulator answers 400 QueueDoesNotExist 'The specified queue does not exist.'

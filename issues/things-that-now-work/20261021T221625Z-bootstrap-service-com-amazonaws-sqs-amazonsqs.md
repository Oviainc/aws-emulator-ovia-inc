Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7d7bdea90bc70459c4b589287302dc4eaf39117d

# Bootstrap the SQS emulator

_Filed: 2026-10-21T22:16:25Z_
_Operation: service-com-amazonaws-sqs-amazonsqs | Class: missing_

> _Note: this issue was identified against the code base at commit `7d7bdea90bc70459c4b589287302dc4eaf39117d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole crate is a fresh scaffold: all 23 operations in `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answer HTTP 501 `NotImplemented`, and `ComAmazonawsSqsAmazonsqsState` has no fields at all. Implement a FIRST, SMALL stateful slice: the standard-queue message lifecycle, end to end, over the `awsJson1_0` wire protocol (`POST /`, `X-Amz-Target: AmazonSQS.<Operation>`).

### Happy path to implement (in order)

1. **CreateQueue** — input `QueueName` (required), optional `Attributes` (map<QueueAttributeName,string>) and `tags`. Allocate the queue in state keyed by name, store the attributes supplied plus SQS defaults (`VisibilityTimeout=30`, `DelaySeconds=0`, `MessageRetentionPeriod=345600`, `ReceiveMessageWaitTimeSeconds=0`, `MaximumMessageSize=262144`, a `QueueArn`, `CreatedTimestamp`/`LastModifiedTimestamp`). Return the generated `QueueUrl` (e.g. `http://<host>/<account-id>/<QueueName>`) and reuse the SAME url on every later reference to that queue. Creating a name that already exists with identical attributes is idempotent (same url); with conflicting attributes it is `QueueNameExists`.
2. **SendMessage** — input `QueueUrl` (required), `MessageBody` (required), optional `DelaySeconds`, `MessageAttributes`. Append a message record to that queue's FIFO-of-arrival deque with a freshly generated uuid `MessageId`. Return `MessageId` and `MD5OfMessageBody` **computed as the real MD5 hex digest of the body** (and `MD5OfMessageAttributes` when attributes were supplied) — not a placeholder. Unknown `QueueUrl` => `QueueDoesNotExist`.
3. **ReceiveMessage** — input `QueueUrl` (required), optional `MaxNumberOfMessages` (default 1, cap 10), `VisibilityTimeout`, `WaitTimeSeconds`, `MessageAttributeNames`/`MessageSystemAttributeNames`. Return up to N currently-visible, non-delayed messages as `Messages[]` with `MessageId`, `Body`, `MD5OfBody`, and a NEWLY minted `ReceiptHandle` per receive. Each returned message becomes invisible for the effective visibility timeout (request value else the queue attribute) and its receive count increments; a second immediate ReceiveMessage must return no messages. When the queue is empty return an output with no `Messages` member (not an error).
4. **DeleteMessage** — input `QueueUrl`, `ReceiptHandle` (both required). Remove the message the handle refers to from the queue permanently, so it never reappears when the visibility timeout lapses. Unknown/garbage handle => `ReceiptHandleIsInvalid`; unknown queue => `QueueDoesNotExist`. Returns an empty body.
5. **GetQueueAttributes** — input `QueueUrl`, optional `AttributeNames` (`All` or a subset). Return `Attributes` read from the stored queue, including live counters derived from state: `ApproximateNumberOfMessages` (visible), `ApproximateNumberOfMessagesNotVisible` (in flight), `ApproximateNumberOfMessagesDelayed`, plus `QueueArn` and the configured attributes. Unknown attribute name => `InvalidAttributeName`.
6. **DeleteQueue** — input `QueueUrl` (required). Remove the queue and all its messages from state. Afterwards GetQueueAttributes/SendMessage/ReceiveMessage on that url must fail with `QueueDoesNotExist`, and the url must not appear in ListQueues.

Also wire **ListQueues** (optional `QueueNamePrefix`, returns `QueueUrls` of queues actually in state, prefix-filtered) and **GetQueueUrl** (`QueueName` -> stored `QueueUrl`, else `QueueDoesNotExist`) if cheap — they make the slice verifiable with the plain AWS CLI, but they are secondary to the six above.

### Hard requirements

- NEVER STUBS: every one of these operations must compute its response from persisted engine state reached via `crate::state::service_state(&*self.engine)`. No canned success, no echoing the input back, no minting a MessageId/ReceiptHandle that is never stored. A hollow 200 is worse than the current honest 501.
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line only from the files you actually implement.
- State must survive across requests through the single shared engine (never construct `InMemoryEngine::new()` inside a handler).
- Error paths in this slice: `QueueDoesNotExist` for an unknown queue url/name, `ReceiptHandleIsInvalid` for a bad receipt handle, `QueueNameExists` for a conflicting re-create, `InvalidAttributeName` for an unknown attribute — serialized as the modeled error shapes with their modeled HTTP statuses (400/404) rather than a generic 500.

### State Model

Add to `ComAmazonawsSqsAmazonsqsState`:

```
ComAmazonawsSqsAmazonsqsState {
    queues: HashMap<String /* queue name */, Queue>,
    urls:   HashMap<String /* queue url */, String /* queue name */>, // url -> name lookup
    account_id: String, // e.g. "000000000000"
}

Queue {
    name: String,
    url: String,                       // http://<host>/<account_id>/<name>
    arn: String,                       // arn:aws:sqs:<region>:<account_id>:<name>
    attributes: HashMap<QueueAttributeName, String>, // VisibilityTimeout, DelaySeconds,
                                       // MessageRetentionPeriod, MaximumMessageSize,
                                       // ReceiveMessageWaitTimeSeconds, Created/LastModifiedTimestamp
    tags: HashMap<String, String>,
    messages: VecDeque<Message>,       // arrival order
    inflight: HashMap<String /* receipt handle */, InflightEntry>,
    created_at: SystemTime,
}

Message {
    message_id: String,                // uuid, generated at SendMessage
    body: String,
    md5_of_body: String,               // real MD5 hex of body
    attributes: HashMap<String, MessageAttributeValue>,
    md5_of_message_attributes: Option<String>,
    sent_at: SystemTime,
    visible_at: SystemTime,            // sent_at + DelaySeconds, then extended on receive
    receive_count: u32,
    first_received_at: Option<SystemTime>,
}

InflightEntry { receipt_handle: String, message_id: String, visible_until: SystemTime }
```

Visibility is evaluated lazily against the clock on each ReceiveMessage: entries whose `visible_until` has passed return to the visible set (this is the clock materialising a change already due, not a read mutating state).

Everything else in OPERATIONS.md — the batch APIs, FIFO/dedup semantics, SetQueueAttributes, Purge, tags, permissions, message-move tasks, pagination tokens — stays stubbed at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-sqs-amazonsqs/OPERATIONS.md` lists 23 operations with the exact input/output members cited above (CreateQueue: `QueueName` required + `Attributes`/`tags` -> `QueueUrl`; SendMessage: `QueueUrl`+`MessageBody` required -> `MessageId`/`MD5OfMessageBody`; ReceiveMessage: `MaxNumberOfMessages`/`VisibilityTimeout`/`WaitTimeSeconds` -> `Messages`; DeleteMessage: `QueueUrl`+`ReceiptHandle`, no output; GetQueueAttributes: `AttributeNames` -> `Attributes`; DeleteQueue: `QueueUrl`, no output) and the modeled errors `QueueDoesNotExist (400)`, `ReceiptHandleIsInvalid (404)`, `QueueNameExists (400)`, `InvalidAttributeName (400)`. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 23 handler files (create_queue.rs, send_message.rs, receive_message.rs, delete_message.rs, get_queue_attributes.rs, delete_queue.rs, ...), so every call currently answers HTTP 501. `src/state.rs` contains only `// TODO: add service-specific state fields`, proving no state is held. Visibility-timeout and MD5-of-body semantics are long-established SQS behavior documented in the model's own operation docs.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with the SQS endpoint.
2. CreateQueue QueueName=demo-queue -> expect 200 with a QueueUrl ending in /demo-queue (today: HTTP 501 NotImplemented).
3. ListQueues -> expect QueueUrls to contain that url.
4. SendMessage QueueUrl=<url> MessageBody='hello world' -> expect a MessageId and MD5OfMessageBody equal to the MD5 hex of 'hello world' (5eb63bbbe01eeed093cb22bb8f5acdc3).
5. GetQueueAttributes QueueUrl=<url> AttributeNames=All -> expect ApproximateNumberOfMessages=1.
6. ReceiveMessage QueueUrl=<url> MaxNumberOfMessages=1 -> expect one message with Body='hello world' and a ReceiptHandle.
7. ReceiveMessage again immediately -> expect NO messages (the first copy is in flight for the 30s visibility timeout).
8. GetQueueAttributes AttributeNames=All -> expect ApproximateNumberOfMessages=0 and ApproximateNumberOfMessagesNotVisible=1.
9. DeleteMessage QueueUrl=<url> ReceiptHandle=<handle> -> expect 200 empty body; then ReceiveMessage after the visibility timeout elapses still returns no messages.
10. DeleteMessage with a bogus receipt handle -> expect ReceiptHandleIsInvalid (404), not 500.
11. DeleteQueue QueueUrl=<url> -> expect 200; then GetQueueAttributes on that url -> expect QueueDoesNotExist (400) and ListQueues no longer lists it.
12. SendMessage to a never-created queue url -> expect QueueDoesNotExist (400).

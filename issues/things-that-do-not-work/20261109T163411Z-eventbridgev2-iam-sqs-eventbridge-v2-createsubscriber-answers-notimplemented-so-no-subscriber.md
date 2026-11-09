Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2__iam__sqs.put-events-content-based-deduplication-suppresses-resend eventbridgev2__iam__sqs.subscriber-filters-combine-with-and-across-scopes eventbridgev2__iam__sqs.subscriber-on-failure-configuration-writes-dead-letter-record-to-sqs eventbridgev2__iam__sqs.update-subscriber-replaces-filter-configuration-whole
Issue-Key: eventbridgev2__iam__sqs|CreateSubscriber|NotImplemented
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# EventBridge v2 CreateSubscriber answers NotImplemented, so no subscriber-based delivery, filtering or dead-lettering can be exercised

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscriber

## Description
Cause 1 of 1: the CreateSubscriber handler is an unimplemented scaffold stub, so every subscriber-scoped behaviour of the v2 event bus is unreachable.

AWS: CreateSubscriber creates a subscriber on a bus and answers 200 with the subscriber's identity (ARN/name) and its configuration; the subscriber then receives events from PutEvents subject to its filter configuration, honours its on-failure configuration by writing a dead-letter record to the configured SQS queue, and UpdateSubscriber replaces its filter configuration wholesale. Smithy: eventbridgev2#CreateSubscriber takes the bus, the subscriber name, its delivery target, filter configuration and on-failure configuration, and declares the service's modelled client errors (validation / conflict / not-found / access-denied class). `NotImplemented` is not among them.

Emulator: CreateSubscriber answers the non-AWS error code `NotImplemented` with message `operation CreateSubscriber is scaffolded but not implemented in this emulator`. No AWS response carries that code or message, and no subscriber ever exists.

Why: a client cannot build any v2 subscriber pipeline: content-based deduplication on PutEvents, AND-combined filters across scopes, on-failure dead-lettering and whole-replacement updates are all unobservable. The returned error is also outside the operation's modelled error set, so SDK-level error handling and retry classification cannot match it.

Fix: implement the CreateSubscriber handler and the subscriber state it needs (filter evaluation, delivery and on-failure handling), in the eventbridgev2 create_subscriber operation handler.

Traces: eventbridgev2__iam__sqs.put-events-content-based-deduplication-suppresses-resend, eventbridgev2__iam__sqs.subscriber-filters-combine-with-and-across-scopes, eventbridgev2__iam__sqs.subscriber-on-failure-configuration-writes-dead-letter-record-to-sqs, eventbridgev2__iam__sqs.update-subscriber-replaces-filter-configuration-whole.

## Evidence
Expected: CreateSubscriber returns 200 with the created subscriber.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSubscriber operation: operation CreateSubscriber is scaffolded but not implemented in this emulator

## Steps to Reproduce
1. Create a v2 event bus and an SQS queue to deliver to.
2. Call CreateSubscriber naming the bus, the queue target and a filter configuration.
3. Observe the NotImplemented error.

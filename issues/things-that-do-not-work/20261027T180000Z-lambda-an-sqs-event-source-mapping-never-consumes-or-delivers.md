Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-handler-error-redelivers-whole-batch dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-invalid-item-identifier-rejected-whole-batch-redelivered dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-partial-batch-redelivers-exactly-reported-items lambda__sqs__stepfunctions.sqs-esm-disabled-mapping-no-invocation-negative
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-handler-error-redelivers-whole-batch|_poll_tally#5|Timeout|time.sleep(3)
Issue-Key: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-invalid-item-identifier-rejected-whole-batch-redelivered|_poll_tally#5|Timeout|time.sleep(3)
Issue-Key: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-partial-batch-redelivers-exactly-reported-items|_poll_tally#5|Timeout|time.sleep(3)
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-disabled-mapping-no-invocation-negative|run#21|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 029c92a6d753af7d483a2e508209908fa6f0a80e

# lambda: an SQS event source mapping never consumes or delivers, so every delivery/redelivery flow built on one makes no progress and is killed at the suite's 60 s cap

> _Note: this cause was identified against the code base at commit `029c92a6d753af7d483a2e508209908fa6f0a80e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this was cause 5 of 5 of `20261025T192150Z-lambda-invokewithresponsestream-refused-notimplemented-updatealias-putresourcepolicy-refusal-messages.md`. Causes 2, 3 and 4 of that issue (the `UpdateAlias` not-found sentence, `DeleteEventSourceMapping` accepted mid-`Updating`, and the truncated oversized-value validation message) are fixed and that report now sits under `issues/things-that-now-work/`; this cause is unfixed and is re-filed here so its four checks are not recorded as working. Its four `Issue-Key` lines and its four `Issue-Trace-Ids` moved with it. Cause 1 of that issue (InvokeWithResponseStream execution) is re-filed beside this one as `20261027T180000Z-lambda-invokewithresponsestream-refuses-every-real-streaming-invocation.md`; the two share one blocker, because what a delivery here has to reach is the customer's handler._

> _Three of these four checks are cross-service (their check ids lead with the composite tokens `dynamodb__lambda__sqs` and `lambda__sfn__sqs`, which match no single service token); the diverging component in every one of them is Lambda's event-source-mapping poller, so they are filed here under service 'lambda'._

## Operation
CreateEventSourceMapping, UpdateEventSourceMapping (the poller and delivery path behind them)

## Description
AWS: a Lambda event source mapping on an SQS queue polls the queue and invokes the function. Recorded behavior only for the timings. For a FIFO source with one MessageGroupId per message: every queued message is delivered at least once once the mapping moves to State=Enabled; when the handler throws, the WHOLE batch returns to the queue even with ReportBatchItemFailures enabled, so each member of the failed batch ends at exactly 2 deliveries and messages that succeeded in other batches at exactly 1; a batchItemFailures list naming an identifier that was not in the batch is treated as an invalid response and the whole batch is redelivered; a valid partial-failure report redelivers exactly the reported items. Independently, a mapping created with Enabled=False reports State=Disabled and consumes nothing (the message stays at ApproximateNumberOfMessages=1), and UpdateEventSourceMapping(Enabled=True) moves it to State=Enabled after which the parked message IS delivered.

Emulator: none of these flows reaches a verdict — all four grounding checks report 'Check timed out after 60.0s'. No invocation, no DynamoDB delivery tally, and no result-queue message is ever observable, so neither the whole-batch redelivery rule, nor the invalid-identifier rule, nor the exact partial-batch redelivery set, nor the disabled-mapping positive control can be exercised. The negative half of the disabled case is vacuously satisfied only because nothing is ever delivered at all. The control plane is real — `services/service-com-amazonaws-lambda-awsgirapiservice/src/state.rs` holds the mapping, its `Enabled` setting and its `State`, and the record's own doc comment states the gap ("No poller runs in this emulator, so the record carries no `LastProcessingResult`") — but nothing ever receives from the queue or invokes the function.

Why: the entire failure-handling contract of an SQS-triggered function is invisible. A client cannot verify that a throwing handler redelivers its batch, that a malformed batchItemFailures report is rejected safely, that a correct partial-failure report redelivers exactly the named items, or that Enabled=False actually parks traffic and re-enabling drains it — i.e. every retry/idempotency assumption an SQS-driven architecture rests on is unverifiable here.

The ONE blocker, for whoever picks this up: every one of these rules is decided by what the CUSTOMER'S HANDLER returns — whether it throws, and the `batchItemFailures` list it reports — and this emulator has no in-process runtime that runs a deployment package, so there is no outcome to interpret. That is the same blocker as `20261027T180000Z-lambda-invokewithresponsestream-refuses-every-real-streaming-invocation.md` and `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` (`src/ops/invoke.rs` answers 501 for it). Writing a tally, a redelivery or a `LastProcessingResult` for a handler that never ran would be a status for work nothing performed, so it is not a fix.

What is left to do in THIS crate once an invocation can actually run: the SQS event-source-mapping poller and delivery path — receive from the queue the mapping names (in-process, through the SQS emulator's own store, which this crate already reaches for the queue's configuration), batch by `BatchSize`/`MaximumBatchingWindowInSeconds`, invoke the mapping's function with the SQS event shape, interpret the function's `ReportBatchItemFailures` response (an identifier that was not in the batch invalidates the whole report), and drive redelivery/visibility accordingly — plus the Enabled/Disabled gate on polling, so a mapping that is `Disabled` consumes nothing and one re-enabled by `UpdateEventSourceMapping(Enabled=True)` drains what was parked.

Related but NOT covering: open issues `20261024T043150Z-lambda-sqs-event-source-mapping-never-consumes-messages.md` and `20261025T022656Z-dynamodb-lambda-sqs-createeventsourcemapping-accepts-a-queue-whose.md` describe adjacent ESM gaps (visibility-timeout extension; VisibilityTimeout-vs-Timeout validation and the DLQ/permission-revocation flow); neither carries the replay keys of the four checks listed here.

Traces: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-handler-error-redelivers-whole-batch, dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-invalid-item-identifier-rejected-whole-batch-redelivered, dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-partial-batch-redelivers-exactly-reported-items, lambda__sfn__sqs.sqs-esm-disabled-mapping-no-invocation-negative.

## Evidence
From the simparity-behavioral-ga-prod-v1 replay against the emulator built from this checkout (all four checks):

```
Error: Check timed out after 60.0s
```

No invocation, no DynamoDB delivery tally and no result-queue message is produced at any point.

Expected: after UpdateEventSourceMapping(Enabled=True) moves the mapping to State=Enabled, the queued messages are delivered; a throwing handler's whole batch is redelivered (tally 2 for every member of that batch, 1 for every message that succeeded elsewhere); an invalid batchItemFailures identifier redelivers the whole batch; a valid partial report redelivers exactly the reported items; and for a mapping created Enabled=False, ApproximateNumberOfMessages stays 1 until it is re-enabled, after which the parked message is delivered and the queue drains to 0.

## Steps to Reproduce
1. Create a FIFO queue, a DynamoDB tally table and a function, then CreateEventSourceMapping(EventSourceArn=<queue>, FunctionName=<fn>, BatchSize=10, Enabled=False, FunctionResponseTypes=['ReportBatchItemFailures']).
2. Send ten messages, each with its own MessageGroupId.
3. UpdateEventSourceMapping(Enabled=True) and poll the tally table — nothing is ever written.
4. Separately: create a mapping with Enabled=False, send one message, confirm no invocation and ApproximateNumberOfMessages=1 (this half passes vacuously), then UpdateEventSourceMapping(Enabled=True) and expect the parked message to be delivered — it never is.

Replay: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-handler-error-redelivers-whole-batch, dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-invalid-item-identifier-rejected-whole-batch-redelivered, dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-partial-batch-redelivers-exactly-reported-items, lambda__sfn__sqs.sqs-esm-disabled-mapping-no-invocation-negative.

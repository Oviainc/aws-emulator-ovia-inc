Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.alias-async-record-reports-the-alias-target-version-and-follows-a-repoint lambda__sqs.alias-event-invoke-config-overrides-the-unqualified-config-for-alias-invokes lambda__sqs.async-timeout-is-a-function-error-recorded-on-failure-with-the-timeout-message lambda__sqs.dead-letter-error-message-attribute-keeps-only-the-first-kilobyte lambda__sqs.dead-letter-queue-and-on-failure-destination-both-receive-the-failed-event lambda__sqs.dead-letter-queue-receives-the-original-event-with-request-id-and-error-attributes lambda__sqs.dead-letter-queue-withholds-successful-and-synchronous-failures lambda__sqs.deleted-event-invoke-config-stops-destination-delivery-until-put-again lambda__sqs.destination-record-carries-non-object-request-and-response-payloads-verbatim lambda__sqs.dry-run-invoke-produces-no-destination-record lambda__sqs.on-success-destination-function-is-invoked-with-the-invocation-record lambda__sqs.published-version-failure-uses-its-publish-time-dead-letter-queue lambda__sqs.published-version-without-its-own-event-invoke-config-gets-no-latest-destination lambda__sqs.put-event-invoke-config-without-destinations-clears-the-previous-ones lambda__sqs.update-event-invoke-config-redirects-on-success-and-keeps-on-failure lambda__sqs.xsvc-lambda-3-async-onsuccess-destination-record-shape lambda__sqs.xsvc-lambda-3-sync-invoke-produces-no-onsuccess-destination-record lambda__sqs.zero-reserved-concurrency-sends-async-events-to-on-failure-without-running-them
Issue-Key: lambda__sqs.alias-async-record-reports-the-alias-target-version-and-follows-a-repoint|_await_messages#16|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.alias-event-invoke-config-overrides-the-unqualified-config-for-alias-invokes|_await_messages#17|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.async-timeout-is-a-function-error-recorded-on-failure-with-the-timeout-message|_await_messages#9|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.dead-letter-error-message-attribute-keeps-only-the-first-kilobyte|_await_messages#10|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.dead-letter-queue-and-on-failure-destination-both-receive-the-failed-event|_await_messages#11|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.dead-letter-queue-receives-the-original-event-with-request-id-and-error-attributes|_await_messages#10|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.dead-letter-queue-withholds-successful-and-synchronous-failures|_await_messages#17|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.deleted-event-invoke-config-stops-destination-delivery-until-put-again|_await_messages#12|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.destination-record-carries-non-object-request-and-response-payloads-verbatim|_await_messages#13|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.dry-run-invoke-produces-no-destination-record|_await_messages#14|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.on-success-destination-function-is-invoked-with-the-invocation-record|_await_messages#13|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.published-version-failure-uses-its-publish-time-dead-letter-queue|_await_messages#19|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.published-version-without-its-own-event-invoke-config-gets-no-latest-destination|_await_messages#13|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.put-event-invoke-config-without-destinations-clears-the-previous-ones|_await_messages#15|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.update-event-invoke-config-redirects-on-success-and-keeps-on-failure|_await_messages#14|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.xsvc-lambda-3-async-onsuccess-destination-record-shape|_await_message#14|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Key: lambda__sqs.xsvc-lambda-3-sync-invoke-produces-no-onsuccess-destination-record|_await_message#17|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Key: lambda__sqs.zero-reserved-concurrency-sends-async-events-to-on-failure-without-running-them|_await_messages#13|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# Asynchronous invocation destinations and DeadLetterConfig never deliver a record

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionEventInvokeConfig

## Description
Cause 1 of 1: no asynchronous invocation ever produces an invocation record at its OnSuccess or OnFailure destination or at its DeadLetterConfig target, so the entire async destination plane is silently inert.

AWS: recorded behavior only. For an Invoke with InvocationType=Event, Lambda writes exactly one invocation record to the EventInvokeConfig's OnSuccess destination when the handler returns and to the OnFailure destination (and the function's DeadLetterConfig target, when both are set) once MaximumRetryAttempts is exhausted; the record carries the request and response payloads verbatim, the version the alias resolved to, and the timeout message for a timed-out event. The configuration is per qualifier: an alias's EventInvokeConfig overrides the unqualified one, a published version uses its publish-time DLQ, a version without its own config gets no $LATEST destination, PutFunctionEventInvokeConfig without destinations clears the previous ones, DeleteFunctionEventInvokeConfig stops delivery until it is put again, an update redirects OnSuccess while keeping OnFailure, synchronous and DryRun invokes produce no record, and a zero reserved-concurrency function sends events to OnFailure without running them. Every one of these eighteen recordings first waits on a destination record (in several cases an explicitly labelled "barrier" control) before asserting its own rule.

Emulator: no record ever appears. All eighteen checks fail on the same empty-destination detail after the full polling window, e.g. `AssertionError: the OnSuccess destination record: nothing arrived within 150s`, `AssertionError: the dead-lettered event: 0 of 1 message(s) arrived within 150s`, `AssertionError: the OnFailure record of the timed-out event: 0 of 1 message(s) arrived within 150s`, `AssertionError: the alias record while live -> one: 0 of 1 message(s) arrived within 150s`, `AssertionError: the async control's OnSuccess record (the barrier): nothing arrived within 150s`, and `Error: Check timed out after 180.0s` for the retry-budget legs.

Why: an async invoke is accepted with 202 and then disappears. A client that depends on destinations or a DLQ to not lose events loses every one of them, with no error, no retry signal and no record of the payload — so failures are unrecoverable and undetectable. Equally, every negative rule in this area (no record for sync/DryRun invokes, cleared destinations, per-version scoping) passes vacuously, so a later regression that *does* deliver wrongly would not be caught.

Fix: the Lambda asynchronous invocation pipeline — the worker that executes an Event invoke, applies MaximumRetryAttempts/MaximumEventAgeInSeconds, and then dispatches the invocation record to the qualifier-resolved EventInvokeConfig destinations and the function's DeadLetterConfig target.

Traces: the eighteen checks listed in trace_ids.

## Evidence
Expected: exactly one invocation record at the configured OnSuccess/OnFailure destination (and the DeadLetterConfig target) per asynchronous invocation, within the recordings' polling windows.
Actual: `0 of 1 message(s) arrived within 150s` / `nothing arrived within 150s` on every destination and DLQ leg, and `Error: Check timed out after 180.0s` on the retry-budget legs.

## Steps to Reproduce
1. CreateFunction and PutFunctionEventInvokeConfig with OnSuccess and OnFailure SQS destinations (and DeadLetterConfig pointing at a queue).
2. Invoke with InvocationType=Event, once with a handler that returns and once with a handler that raises (and once with a handler that exceeds Timeout).
3. ReceiveMessage on the destination and DLQ queues — nothing ever arrives.

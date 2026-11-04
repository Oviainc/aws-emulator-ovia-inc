Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka__lambda.msk-lambda-esm-filter-criteria-delivers-only-matching-records-and-a-cleared-filter-releases-the-rest kafka__lambda.msk-lambda-event-source-mapping-disabled-delivers-nothing-then-reenabled-delivers-the-missed-records
Issue-Key: kafka__lambda.msk-lambda-esm-filter-criteria-delivers-only-matching-records-and-a-cleared-filter-releases-the-rest|_await_data_plane#1|Timeout|time.sleep(2)
Issue-Key: kafka__lambda.msk-lambda-event-source-mapping-disabled-delivers-nothing-then-reenabled-delivers-the-missed-records|_await_data_plane#3|Timeout|time.sleep(2)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# MSK event source mappings never deliver: filter criteria and disable/re-enable are unobservable

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: a Lambda event source mapping on an MSK cluster topic must deliver only the records that match FilterCriteria (and release the rest once the filter is cleared), and must deliver nothing while Enabled=false and then deliver the records it missed once re-enabled.
AWS: recorded behavior only — both recordings create the mapping over an MSK topic, produce records, and then read the handler's own log lines to see which records arrived in which order; on AWS each settles inside the suite's 60s budget, including the "missed records are delivered after re-enable" leg which depends on the mapping resuming from its stored offset.
Emulator: both checks are cut off with "Error: Check timed out after 60.0s". CreateEventSourceMapping/UpdateEventSourceMapping are accepted and the mapping reports its Enabled/filter state, but no batch is ever handed to the function, so no record is ever observed on either leg.
Why: an MSK-triggered function silently consumes nothing while its mapping reports healthy; neither FilterCriteria semantics nor the offset-retaining disable/re-enable behavior is observable, so a client cannot tell a filter that drops everything from a consumer that never runs.
Fix: the Lambda event-source-mapping poller for MSK/self-managed Kafka — topic/partition consumption, offset retention across Enabled transitions, FilterCriteria evaluation and the invocation of the target function.
Traces: kafka__lambda.msk-lambda-esm-filter-criteria-delivers-only-matching-records-and-a-cleared-filter-releases-the-rest, .msk-lambda-event-source-mapping-disabled-delivers-nothing-then-reenabled-delivers-the-missed-records.

## Evidence
Expected (AWS): only matching records reach the handler, and clearing the filter releases the rest; a disabled mapping delivers nothing and, once re-enabled, delivers the records produced while it was off.
Actual (emulator): both checks end with 'Error: Check timed out after 60.0s' — no record is ever delivered.

## Steps to Reproduce
1. Create an MSK cluster and topic, and a Lambda with an event source mapping on that topic (with FilterCriteria).
2. Produce matching and non-matching records and poll the function's log group. Observe nothing is delivered.
3. UpdateEventSourceMapping Enabled=false, produce records, re-enable, and poll again. Observe nothing is delivered within 60s.

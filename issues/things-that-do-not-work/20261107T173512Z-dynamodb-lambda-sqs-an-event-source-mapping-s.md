Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked
Issue-Key: dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked|run#21|AssertionError|assert bad == [], (
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# dynamodb__lambda__sqs: an event source mapping's FilterCriteria is not applied to DynamoDB stream records — a MODIFY record reaches a function filtered to eventName=[INSERT]

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: FilterCriteria on a DynamoDB-stream event source mapping is accepted but never evaluated, so non-matching records are delivered.
AWS: Smithy: lambda CreateEventSourceMapping member FilterCriteria (Filters[].Pattern, echoed back on the EventSourceMappingConfiguration output). For a DynamoDB stream the poller evaluates each record against the patterns and invokes the function ONLY with the records that match; a pattern of {"eventName": ["INSERT"]} means a MODIFY or REMOVE record is filtered out and never appears in any invocation event. Filtering happens on the poller side, so a non-matching record does not even count as a delivered batch.
Emulator: the mapping is created with the filter and the filter is ignored — a MODIFY record produced by overwriting an existing item is delivered to the function: `FilterCriteria eventName=[INSERT] let 1 MODIFY record(s) invoke the function: [{'eventName': 'MODIFY', 'pk': 'row-1'}]`.
Why: a client that uses FilterCriteria to keep a handler from seeing irrelevant mutations gets them anyway — handlers run on events they were explicitly configured not to receive, which duplicates work, can re-trigger side effects, and makes insert-only semantics unenforceable.
Fix: the lambda event-source-mapping poller's record filter — FilterCriteria patterns must be evaluated against each DynamoDB stream record before the batch is assembled and the function invoked.
Traces: dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked. Related open issue that does not carry this check's replay key: 20261107T125044Z-dynamodb-events-lambda-an-event-source-mapping-s.md.

## Evidence
Expected: with FilterCriteria {"Filters": [{"Pattern": "{\"eventName\":[\"INSERT\"]}"}]} on a DynamoDB-stream ESM, only INSERT records are delivered; a MODIFY record produces no invocation. Actual: `AssertionError: FilterCriteria eventName=[INSERT] let 1 MODIFY record(s) invoke the function: [{'eventName': 'MODIFY', 'pk': 'row-1'}]`.

## Steps to Reproduce
Create a table with a stream, create a function, CreateEventSourceMapping with FilterCriteria pattern {"eventName":["INSERT"]}, PutItem a new key (INSERT) then overwrite the same key (MODIFY), and inspect which records the handler recorded. Observe the MODIFY record present. Re-replay dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked.

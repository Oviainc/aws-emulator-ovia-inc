Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.deep-kinesis-resharding-efo-subscription-on-split-parent-ends-with-child-shards kinesis.delete-stream-with-active-consumer-requires-enforce-flag kinesis.e2e-enhanced-fanout-consumer-lifecycle kinesis.e2e-stream-consumer-fanout-lifecycle kinesis.e2e-stream-consumer-fanout-subscribe-flow kinesis.list-stream-consumers-expired-token-unreachable kinesis.list-stream-consumers-malformed-next-token-rejected kinesis.list-stream-consumers-pagination-preserves-order kinesis.register-duplicate-consumer-name-fails kinesis.register-stream-consumer-lifecycle kinesis.register-stream-consumer-same-name-different-streams-allowed kinesis.stream-summary-consumer-count-tracks-registered-consumers kinesis.subscribe-to-shard-delivers-records-in-order kinesis.subscribe-to-shard-delivers-records-in-sequence-order kinesis.subscribe-to-shard-delivers-records-in-write-order kinesis.subscribe-to-shard-dry-run-does-not-deliver-records kinesis.subscribe-to-shard-dry-run-rejected kinesis.subscribe-to-shard-dry-run-skips-execution kinesis.subscribe-to-shard-event-stream-framing-over-a-hand-signed-http2-request kinesis.subscribe-to-shard-expires-after-five-minutes kinesis.subscribe-to-shard-honors-starting-position-latest kinesis.subscribe-to-shard-lease-expires-after-5-minutes kinesis.subscribe-to-shard-missing-permission-denied kinesis.subscribe-to-shard-second-subscription-rejected kinesis.subscribe-to-shard-subscription-expires-after-max-duration kinesis.subscribe-to-shard-with-shard-id-from-another-stream-rejected kinesis.subscribe-to-shard-without-permission-denied kinesis.subscribe-to-shard-without-permission-must-be-denied kinesis.two-enhanced-fanout-consumers-each-receive-all-records
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|RegisterStreamConsumer|NotImplemented
Issue-Commit: d5c75e29b1e90f1499e967e91f7af40c6cd43d58

# Implement kinesis RegisterStreamConsumer: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d5c75e29b1e90f1499e967e91f7af40c6cd43d58`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterStreamConsumer

## Description
`RegisterStreamConsumer` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/register_stream_consumer.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

29 recorded unit(s) fail on their `RegisterStreamConsumer` call, so each one unblocks only when `RegisterStreamConsumer` works: `kinesis.deep-kinesis-resharding-efo-subscription-on-split-parent-ends-with-child-shards`, `kinesis.delete-stream-with-active-consumer-requires-enforce-flag`, `kinesis.e2e-enhanced-fanout-consumer-lifecycle`, `kinesis.e2e-stream-consumer-fanout-lifecycle`, `kinesis.e2e-stream-consumer-fanout-subscribe-flow`, `kinesis.list-stream-consumers-expired-token-unreachable`, `kinesis.list-stream-consumers-malformed-next-token-rejected`, `kinesis.list-stream-consumers-pagination-preserves-order`, `kinesis.register-duplicate-consumer-name-fails`, `kinesis.register-stream-consumer-lifecycle`, `kinesis.register-stream-consumer-same-name-different-streams-allowed`, `kinesis.stream-summary-consumer-count-tracks-registered-consumers`, `kinesis.subscribe-to-shard-delivers-records-in-order`, `kinesis.subscribe-to-shard-delivers-records-in-sequence-order`, `kinesis.subscribe-to-shard-delivers-records-in-write-order`, `kinesis.subscribe-to-shard-dry-run-does-not-deliver-records`, `kinesis.subscribe-to-shard-dry-run-rejected`, `kinesis.subscribe-to-shard-dry-run-skips-execution`, `kinesis.subscribe-to-shard-event-stream-framing-over-a-hand-signed-http2-request`, `kinesis.subscribe-to-shard-expires-after-five-minutes` (+9 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/register_stream_consumer.rs`. Replaying the corpus, the emulator refused the `RegisterStreamConsumer` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterStreamConsumer operation: operation RegisterStreamConsumer is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis emulator
Call RegisterStreamConsumer and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.closed-parent-shard-drains-to-null-next-iterator kinesis.deep-kinesis-resharding-list-shards-timestamp-filters-resolve-split-lineage kinesis.deep-kinesis-resharding-merge-with-closed-parent-rejected kinesis.deep-kinesis-resharding-split-children-route-partition-keys-to-md5-owner kinesis.hot-key-skew-across-split-shard-with-in-flight-traffic kinesis.list-shards-shard-filter-resolves-the-resharding-lineage kinesis.split-shard-children-partition-parent-hash-range-and-records kinesis.split-shard-creates-child-shards kinesis.update-stream-mode-while-resharding-blocked
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|SplitShard|NotImplemented
Issue-Commit: d5c75e29b1e90f1499e967e91f7af40c6cd43d58

# Implement kinesis SplitShard: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d5c75e29b1e90f1499e967e91f7af40c6cd43d58`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SplitShard

## Description
`SplitShard` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/split_shard.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `SplitShard` call, so each one unblocks only when `SplitShard` works: `kinesis.closed-parent-shard-drains-to-null-next-iterator`, `kinesis.deep-kinesis-resharding-list-shards-timestamp-filters-resolve-split-lineage`, `kinesis.deep-kinesis-resharding-merge-with-closed-parent-rejected`, `kinesis.deep-kinesis-resharding-split-children-route-partition-keys-to-md5-owner`, `kinesis.hot-key-skew-across-split-shard-with-in-flight-traffic`, `kinesis.list-shards-shard-filter-resolves-the-resharding-lineage`, `kinesis.split-shard-children-partition-parent-hash-range-and-records`, `kinesis.split-shard-creates-child-shards`, `kinesis.update-stream-mode-while-resharding-blocked`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/split_shard.rs`. Replaying the corpus, the emulator refused the `SplitShard` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SplitShard operation: operation SplitShard is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis emulator
Call SplitShard and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

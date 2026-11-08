Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.describe-topic-partitions-nonexistent-topic-not-found kafka.topic-describe-partitions-matches-count kafka.topic-lifecycle-create-describe-update-delete kafka.topic-partition-order-preserved-after-partition-increase kafka.topic-retention-ms-numeric-precision-preserved
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DescribeTopicPartitions|NotImplemented
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# Implement kafka DescribeTopicPartitions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTopicPartitions

## Description
`DescribeTopicPartitions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/describe_topic_partitions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `DescribeTopicPartitions` call, so each one unblocks only when `DescribeTopicPartitions` works: `kafka.describe-topic-partitions-nonexistent-topic-not-found`, `kafka.topic-describe-partitions-matches-count`, `kafka.topic-lifecycle-create-describe-update-delete`, `kafka.topic-partition-order-preserved-after-partition-increase`, `kafka.topic-retention-ms-numeric-precision-preserved`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/describe_topic_partitions.rs`. Replaying the corpus, the emulator refused the `DescribeTopicPartitions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeTopicPartitions operation: operation DescribeTopicPartitions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kafka emulator
Call DescribeTopicPartitions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

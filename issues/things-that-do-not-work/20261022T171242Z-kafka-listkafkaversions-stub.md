Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.account-level-catalog-listings kafka.batch-disassociate-scram-secret-malformed-arn-rejected kafka.bootstrap-brokers-multi-auth-endpoint-accuracy kafka.channel-invalid-name-pattern-rejected kafka.cluster-policy-json-unicode-round-trip kafka.cluster-policy-sequential-overwrite-latest-wins kafka.compatible-kafka-versions-no-downgrade-offered kafka.concurrent-vpc-connection-duplicate-create-race kafka.configuration-create-rejected-for-unsupported-kafka-version kafka.configuration-partial-invalid-kafka-versions-rejected kafka.configuration-properties-base64-round-trip kafka.configuration-unsupported-kafka-version-rejected kafka.deploy-msk-cluster-cfn-bootstrap-brokers-reachable kafka.kafka-versions-catalog-and-compatibility kafka.kafka-versions-pagination-is-a-stable-partition kafka.scram-associate-invalid-arn-rejected kafka.server-side-cluster-policy-cross-account-enforcement kafka.server-side-scram-secret-kms-encryption-required kafka.tag-unicode-and-empty-value-preserved kafka.topic-replication-factor-exceeds-broker-count-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|ListKafkaVersions|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement kafka ListKafkaVersions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListKafkaVersions

## Description
`ListKafkaVersions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/list_kafka_versions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `ListKafkaVersions` call, so each one unblocks only when `ListKafkaVersions` works: `kafka.account-level-catalog-listings`, `kafka.batch-disassociate-scram-secret-malformed-arn-rejected`, `kafka.bootstrap-brokers-multi-auth-endpoint-accuracy`, `kafka.channel-invalid-name-pattern-rejected`, `kafka.cluster-policy-json-unicode-round-trip`, `kafka.cluster-policy-sequential-overwrite-latest-wins`, `kafka.compatible-kafka-versions-no-downgrade-offered`, `kafka.concurrent-vpc-connection-duplicate-create-race`, `kafka.configuration-create-rejected-for-unsupported-kafka-version`, `kafka.configuration-partial-invalid-kafka-versions-rejected`, `kafka.configuration-properties-base64-round-trip`, `kafka.configuration-unsupported-kafka-version-rejected`, `kafka.deploy-msk-cluster-cfn-bootstrap-brokers-reachable`, `kafka.kafka-versions-catalog-and-compatibility`, `kafka.kafka-versions-pagination-is-a-stable-partition`, `kafka.scram-associate-invalid-arn-rejected`, `kafka.server-side-cluster-policy-cross-account-enforcement`, `kafka.server-side-scram-secret-kms-encryption-required`, `kafka.tag-unicode-and-empty-value-preserved`, `kafka.topic-replication-factor-exceeds-broker-count-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/list_kafka_versions.rs`. Replaying the corpus, the emulator refused the `ListKafkaVersions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListKafkaVersions operation: operation ListKafkaVersions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kafka emulator
Call ListKafkaVersions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

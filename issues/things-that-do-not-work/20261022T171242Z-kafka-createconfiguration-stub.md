Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-arn-region-scope-enforced kafka.configuration-create-describe-lifecycle kafka.configuration-delete-in-use-conflict kafka.configuration-delete-lifecycle kafka.configuration-description-empty-vs-absent-preserved kafka.configuration-duplicate-name-rejected kafka.configuration-invalid-server-property-rejected kafka.configuration-list-and-revisions kafka.configuration-name-over-length-rejected kafka.configuration-numeric-property-precision-preserved kafka.configuration-property-value-type-accepted kafka.configuration-reserved-broker-property-rejected kafka.configuration-revision-not-found kafka.configuration-revisions-append-only-immutable kafka.configuration-server-properties-multiline-round-trip kafka.configuration-server-properties-round-trip kafka.configuration-unicode-properties-round-trip kafka.configuration-update-rejected-while-deleting kafka.configuration-update-revision-increment kafka.configuration-update-revisions-are-immutable kafka.configuration-update-server-properties-unicode-preserved kafka.create-with-invalid-input-should-fail kafka.list-configurations-pagination-consistency kafka.update-cluster-configuration-triggers-operation
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateConfiguration|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement kafka CreateConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguration

## Description
`CreateConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

24 recorded unit(s) fail on their `CreateConfiguration` call, so each one unblocks only when `CreateConfiguration` works: `kafka.configuration-arn-region-scope-enforced`, `kafka.configuration-create-describe-lifecycle`, `kafka.configuration-delete-in-use-conflict`, `kafka.configuration-delete-lifecycle`, `kafka.configuration-description-empty-vs-absent-preserved`, `kafka.configuration-duplicate-name-rejected`, `kafka.configuration-invalid-server-property-rejected`, `kafka.configuration-list-and-revisions`, `kafka.configuration-name-over-length-rejected`, `kafka.configuration-numeric-property-precision-preserved`, `kafka.configuration-property-value-type-accepted`, `kafka.configuration-reserved-broker-property-rejected`, `kafka.configuration-revision-not-found`, `kafka.configuration-revisions-append-only-immutable`, `kafka.configuration-server-properties-multiline-round-trip`, `kafka.configuration-server-properties-round-trip`, `kafka.configuration-unicode-properties-round-trip`, `kafka.configuration-update-rejected-while-deleting`, `kafka.configuration-update-revision-increment`, `kafka.configuration-update-revisions-are-immutable` (+4 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs`. Replaying the corpus, the emulator refused the `CreateConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConfiguration operation: operation CreateConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kafka emulator
Call CreateConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

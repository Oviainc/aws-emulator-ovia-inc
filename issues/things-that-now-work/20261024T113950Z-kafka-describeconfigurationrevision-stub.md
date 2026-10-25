Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-numeric-property-precision-preserved
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DescribeConfigurationRevision|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement kafka DescribeConfigurationRevision: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationRevision

## Description

> _Status update: `DescribeConfigurationRevision` is NO LONGER a stub. It was implemented, and its `//! SCAFFOLDED, NOT IMPLEMENTED:` census line deleted, by the fix for `20261024T113950Z-kafka-updateconfiguration-stub.md`: the revision reader is what proves an appended revision persisted, so the update fix could not be verified without it. The handler now reports a stored revision's ARN, instant, description, number and `server.properties` body from the service's state (see `services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration_revision.rs` and the suites in `services/service-com-amazonaws-kafka-kafka/tests/all/configuration_revisions.rs`). What remains for whoever picks this report up is to confirm the recorded unit below now replays, and then to move this report to `issues/things-that-now-work/`._

`DescribeConfigurationRevision` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration_revision.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeConfigurationRevision` call, so each one unblocks only when `DescribeConfigurationRevision` works: `kafka.configuration-numeric-property-precision-preserved`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration_revision.rs`. Replaying the corpus, the emulator refused the `DescribeConfigurationRevision` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeConfigurationRevision operation: operation DescribeConfigurationRevision is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kafka emulator
Call DescribeConfigurationRevision and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-delete-in-use-conflict kafka.delete-nonexistent-resources-should-fail
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DeleteConfiguration|NotImplemented
Issue-Key: kafka.delete-nonexistent-resources-should-fail|run#9|AssertionError|assert observed[label] == expected, f"{label}: expected {expected!r}, got {observed[label]!r}"
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement kafka DeleteConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConfiguration

## Description
`DeleteConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/delete_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteConfiguration` call, so each one unblocks only when `DeleteConfiguration` works: `kafka.configuration-delete-in-use-conflict`, `kafka.delete-nonexistent-resources-should-fail`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/delete_configuration.rs`. Replaying the corpus, the emulator refused the `DeleteConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteConfiguration operation: operation DeleteConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kafka emulator
Call DeleteConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

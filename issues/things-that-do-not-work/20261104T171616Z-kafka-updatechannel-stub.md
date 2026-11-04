Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.channel-concurrent-update-race-rejected-or-serialized kafka.channel-update-mutation-reflected-in-describe
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.channel-concurrent-update-race-rejected-or-serialized|run#15|AssertionError|assert first == ("ok", first_value), (
Issue-Key: kafka|UpdateChannel|NotImplemented
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# Implement kafka UpdateChannel: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChannel

## Description
`UpdateChannel` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/update_channel.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateChannel` call, so each one unblocks only when `UpdateChannel` works: `kafka.channel-concurrent-update-race-rejected-or-serialized`, `kafka.channel-update-mutation-reflected-in-describe`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/update_channel.rs`. Replaying the corpus, the emulator refused the `UpdateChannel` call: File "run.py", line 348, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_kafka_channel_concurrent_update_race_rejected_or_serialized.py", line 91, in run
    assert first == ("ok", first_value), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: the first UpdateChannel on an ACTIVE channel must be accepted, got ('error', 'NotImplemented', 'operation UpdateChannel is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kafka emulator
Call UpdateChannel and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.channel-duplicate-name-conflict
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.channel-duplicate-name-conflict|run#13|AssertionError|assert list_code == _NOT_FOUND, f"ListChannels: expected NotFoundException, got {list_code}"
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# Implement kafka ListChannels: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
`ListChannels` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/list_channels.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListChannels` call, so each one unblocks only when `ListChannels` works: `kafka.channel-duplicate-name-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/list_channels.rs`. Replaying the corpus, the emulator refused the `ListChannels` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_kafka_channel_duplicate_name_conflict.py", line 106, in run
    assert list_code == _NOT_FOUND, f"ListChannels: expected NotFoundException, got {list_code}"
           ^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ListChannels: expected NotFoundException, got ('NotImplemented', 'operation ListChannels is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kafka emulator
Call ListChannels and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.reboot-broker-missing-permission-denied
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.reboot-broker-missing-permission-denied|run#18|AssertionError|assert restricted == expected_denial, f"expected {expected_denial!r}, got {restricted!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement kafka RebootBroker: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootBroker

## Description
`RebootBroker` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/reboot_broker.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RebootBroker` call, so each one unblocks only when `RebootBroker` works: `kafka.reboot-broker-missing-permission-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/reboot_broker.rs`. Replaying the corpus, the emulator refused the `RebootBroker` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_kafka_reboot_broker_missing_permission_denied.py", line 74, in run
    assert restricted == expected_denial, f"expected {expected_denial!r}, got {restricted!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ('AccessDeniedException', 'User: arn:aws:sts::000000000000:assumed-role/sim2real-kafka-deny-47acc886ee8d/sim2real-behav-reboot-broker is not authorized to perform: kafka:RebootBroker on resource: arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-bogus-ba364362a8a2/3d53cb19-d09e-4261-ba34-07fb0f13738c-1 with an explicit deny in an identity-based policy'), got ('NotImplemented', 'operation RebootBroker is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kafka emulator
Call RebootBroker and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.update-rebalancing-server-enforced
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.update-rebalancing-server-enforced|run#8|AssertionError|assert real_err == _INVALID_PARAMETERS, (
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# Implement kafka UpdateRebalancing: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRebalancing

## Description
`UpdateRebalancing` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/update_rebalancing.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateRebalancing` call, so each one unblocks only when `UpdateRebalancing` works: `kafka.update-rebalancing-server-enforced`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/update_rebalancing.rs`. Replaying the corpus, the emulator refused the `UpdateRebalancing` call: File "run.py", line 354, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_kafka_update_rebalancing_server_enforced.py", line 155, in run
    assert real_err == _INVALID_PARAMETERS, (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: UpdateRebalancing on this check's OWN cluster (arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-behav-rebalance-7af89fd8f436/12b98e4e-c2fe-4d82-ab85-368cadc18b2a-1): expected ('BadRequestException', 'One or more of the parameters are not valid.'), got ('NotImplemented', 'operation UpdateRebalancing is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kafka emulator
Call UpdateRebalancing and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

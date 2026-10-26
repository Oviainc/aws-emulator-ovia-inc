Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.delete-nonexistent-resources-should-fail
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.delete-nonexistent-resources-should-fail|run#9|AssertionError|assert observed[label] == expected, f"{label}: expected {expected!r}, got {observed[label]!r}"
Issue-Commit: 851c7b8e55a8e0663fe932e2290fb361fc961d0e

# Implement kafka DeleteClusterPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `851c7b8e55a8e0663fe932e2290fb361fc961d0e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteClusterPolicy

## Description
`DeleteClusterPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/delete_cluster_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteClusterPolicy` call, so each one unblocks only when `DeleteClusterPolicy` works: `kafka.delete-nonexistent-resources-should-fail`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/delete_cluster_policy.rs`. Replaying the corpus, the emulator refused the `DeleteClusterPolicy` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_kafka_delete_nonexistent_resources_should_fail.py", line 103, in run
    assert observed[label] == expected, f"{label}: expected {expected!r}, got {observed[label]!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: DeleteClusterPolicy: expected ('NotFoundException', 'The requested resource doesn’t exist.'), got ('NotImplemented', 'operation DeleteClusterPolicy is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kafka emulator
Call DeleteClusterPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

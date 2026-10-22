Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.tag-resource-nonexistent-arn-not-found kafka.tag-untag-nonexistent-resource-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.tag-resource-nonexistent-arn-not-found|run#5|AssertionError|assert tag_err == expected, f"TagResource: expected {expected!r}, got {tag_err!r}"
Issue-Key: kafka.tag-untag-nonexistent-resource-rejected|run#5|AssertionError|assert got == expected, f"{op} on a nonexistent ARN: expected {expected!r}, got {got!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement kafka TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `kafka.tag-resource-nonexistent-arn-not-found`, `kafka.tag-untag-nonexistent-resource-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_kafka_tag_resource_nonexistent_arn_not_found.py", line 53, in run
    assert tag_err == expected, f"TagResource: expected {expected!r}, got {tag_err!r}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: TagResource: expected ('NotFoundException', 'Resource was not found'), got ('NotImplemented', 'operation TagResource is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kafka emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

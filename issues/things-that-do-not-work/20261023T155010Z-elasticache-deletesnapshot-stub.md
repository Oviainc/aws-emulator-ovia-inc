Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.snapshot-not-found-semantics elasticache.snapshot-of-a-single-node-cluster-copies-and-deletes
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.snapshot-not-found-semantics|_assert_error#5|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code} ({actual!r})"
Issue-Key: elasticache|DeleteSnapshot|NotImplemented
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# Implement elasticache DeleteSnapshot: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSnapshot

## Description
`DeleteSnapshot` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_snapshot.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteSnapshot` call, so each one unblocks only when `DeleteSnapshot` works: `elasticache.snapshot-not-found-semantics`, `elasticache.snapshot-of-a-single-node-cluster-copies-and-deletes`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_snapshot.rs`. Replaying the corpus, the emulator refused the `DeleteSnapshot` call: File "check_elasticache_snapshot_not_found_semantics.py", line 44, in run
    _assert_error(
    ~~~~~~~~~~~~~^
        lambda: ec.delete_snapshot(SnapshotName=bogus),
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        "SnapshotNotFoundFault",
        ^^^^^^^^^^^^^^^^^^^^^^^^
        f"Snapshot with name {bogus} not found",
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "check_elasticache_snapshot_not_found_semantics.py", line 27, in _assert_error
    assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code} ({actual!r})"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected SnapshotNotFoundFault, got NotImplemented ('operation DeleteSnapshot is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the elasticache emulator
Call DeleteSnapshot and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

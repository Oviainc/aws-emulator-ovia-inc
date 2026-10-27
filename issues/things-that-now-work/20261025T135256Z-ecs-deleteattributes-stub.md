Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.tagging-attribute-ops-on-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.tagging-attribute-ops-on-missing-cluster-not-found|run#29|AssertionError|assert error == not_found, f"DeleteAttributes: got {error!r}"
Issue-Commit: eccc631279b459acd1f5ae10e81d9e1b78dcc975

# Implement ecs DeleteAttributes: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eccc631279b459acd1f5ae10e81d9e1b78dcc975`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAttributes

## Description
`DeleteAttributes` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/delete_attributes.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteAttributes` call, so each one unblocks only when `DeleteAttributes` works: `ecs.tagging-attribute-ops-on-missing-cluster-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/delete_attributes.rs`. Replaying the corpus, the emulator refused the `DeleteAttributes` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ecs_tagging_attribute_ops_on_missing_cluster_not_found.py", line 106, in run
    assert error == not_found, f"DeleteAttributes: got {error!r}"
           ^^^^^^^^^^^^^^^^^^
AssertionError: DeleteAttributes: got ('NotImplemented', 'operation DeleteAttributes is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ecs emulator
Call DeleteAttributes and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

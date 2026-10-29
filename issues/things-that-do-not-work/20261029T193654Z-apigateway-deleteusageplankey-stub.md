Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.concurrent-delete-usage-plan-key-double-delete-rejected apigateway.op-contract-delete-usage-plan-key apigateway.op-contract-deleteUsagePlanKey apigateway.usage-plan-key-association-lifecycle
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.concurrent-delete-usage-plan-key-double-delete-rejected|run#14|AssertionError|assert len(successes) == 1, f"exactly one racer should win the delete, got outcomes={outcomes}"
Issue-Key: api-gateway|DeleteUsagePlanKey|NotImplemented
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# Implement apigateway DeleteUsagePlanKey: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteUsagePlanKey

## Description
`DeleteUsagePlanKey` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_usage_plan_key.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `DeleteUsagePlanKey` call, so each one unblocks only when `DeleteUsagePlanKey` works: `apigateway.concurrent-delete-usage-plan-key-double-delete-rejected`, `apigateway.op-contract-delete-usage-plan-key`, `apigateway.op-contract-deleteUsagePlanKey`, `apigateway.usage-plan-key-association-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_usage_plan_key.rs`. Replaying the corpus, the emulator refused the `DeleteUsagePlanKey` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_apigateway_concurrent_delete_usage_plan_key_double_delete_rejected.py", line 49, in run
    assert len(successes) == 1, f"exactly one racer should win the delete, got outcomes={outcomes}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: exactly one racer should win the delete, got outcomes=[('NotImplemented', 'operation DeleteUsagePlanKey is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation DeleteUsagePlanKey is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation DeleteUsagePlanKey is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation DeleteUsagePlanKey is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation DeleteUsagePlanKey is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation DeleteUsagePlanKey is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the apigateway emulator
Call DeleteUsagePlanKey and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

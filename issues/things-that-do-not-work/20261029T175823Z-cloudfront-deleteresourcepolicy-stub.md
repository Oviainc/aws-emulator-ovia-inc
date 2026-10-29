Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-deleteResourcePolicy cloudfront.resource-policy-delete-in-use-blocked
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-deleteResourcePolicy|run#15|AssertionError|assert delete_code == "InvalidArgument", (
Issue-Key: cloudfront.resource-policy-delete-in-use-blocked|run#16|AssertionError|assert delete_code == "InvalidArgument", f"expected InvalidArgument, got {delete_code!r}: {delete_message}"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Implement cloudfront DeleteResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResourcePolicy

## Description
`DeleteResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteResourcePolicy` call, so each one unblocks only when `DeleteResourcePolicy` works: `cloudfront.op-contract-deleteResourcePolicy`, `cloudfront.resource-policy-delete-in-use-blocked`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_resource_policy.rs`. Replaying the corpus, the emulator refused the `DeleteResourcePolicy` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_cloudfront_op_contract_deleteResourcePolicy.py", line 68, in run
    assert delete_code == "InvalidArgument", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: DeleteResourcePolicy: expected InvalidArgument, got NotImplemented: operation DeleteResourcePolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call DeleteResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

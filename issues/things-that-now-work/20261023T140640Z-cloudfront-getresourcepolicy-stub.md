Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-deleteResourcePolicy
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-deleteResourcePolicy|run#12|AssertionError|assert get_code == "InvalidArgument", f"GetResourcePolicy: expected InvalidArgument, got {get_code}: {get_msg}"
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement cloudfront GetResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
`GetResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/get_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetResourcePolicy` call, so each one unblocks only when `GetResourcePolicy` works: `cloudfront.op-contract-deleteResourcePolicy`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/get_resource_policy.rs`. Replaying the corpus, the emulator refused the `GetResourcePolicy` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudfront_op_contract_deleteResourcePolicy.py", line 58, in run
    assert get_code == "InvalidArgument", f"GetResourcePolicy: expected InvalidArgument, got {get_code}: {get_msg}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: GetResourcePolicy: expected InvalidArgument, got NotImplemented: operation GetResourcePolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call GetResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

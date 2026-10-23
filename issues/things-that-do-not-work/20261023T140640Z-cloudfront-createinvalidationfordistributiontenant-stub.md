Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-createInvalidationForDistributionTenant
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-createInvalidationForDistributionTenant|run#4|AssertionError|assert code == "EntityNotFound", f"expected EntityNotFound, got {code}: {message}"
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement cloudfront CreateInvalidationForDistributionTenant: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvalidationForDistributionTenant

## Description
`CreateInvalidationForDistributionTenant` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_invalidation_for_distribution_tenant.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateInvalidationForDistributionTenant` call, so each one unblocks only when `CreateInvalidationForDistributionTenant` works: `cloudfront.op-contract-createInvalidationForDistributionTenant`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_invalidation_for_distribution_tenant.rs`. Replaying the corpus, the emulator refused the `CreateInvalidationForDistributionTenant` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudfront_op_contract_createInvalidationForDistributionTenant.py", line 46, in run
    assert code == "EntityNotFound", f"expected EntityNotFound, got {code}: {message}"
           ^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected EntityNotFound, got NotImplemented: operation CreateInvalidationForDistributionTenant is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call CreateInvalidationForDistributionTenant and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

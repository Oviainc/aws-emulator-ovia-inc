Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehubv2.test-creation-requires-multi-az-rto-on-services-policy
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2.test-creation-requires-multi-az-rto-on-services-policy|run#13|AssertionError|assert code == "ValidationException", (
Issue-Commit: 82bf3da4596cf523466b02fd09cd706f8136ec0c

# Implement resiliencehubv2 CreateTest: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `82bf3da4596cf523466b02fd09cd706f8136ec0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTest

## Description
`CreateTest` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/create_test.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateTest` call, so each one unblocks only when `CreateTest` works: `resiliencehubv2.test-creation-requires-multi-az-rto-on-services-policy`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/create_test.rs`. Replaying the corpus, the emulator refused the `CreateTest` call: File "run.py", line 348, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_resiliencehubv2_test_creation_requires_multi_az_rto_on_services_policy.py", line 97, in run
    assert code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: CreateTest with no multi-AZ RTO on the service's policy must be refused with ValidationException, got NotImplemented: 'operation CreateTest is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the resiliencehubv2 emulator
Call CreateTest and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

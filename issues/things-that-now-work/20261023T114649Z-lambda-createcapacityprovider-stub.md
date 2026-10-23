Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.capacity-provider-lifecycle lambda.capacity-provider-limit-enforcement lambda.capacity-provider-list-and-versions lambda.capacity-provider-scaling-update-triggers-version-rescale lambda.capacity-provider-scaling-value-precision-round-trip lambda.capacity-provider-unicode-name-round-trip lambda.capacity-provider-version-list-ordering lambda.delete-capacity-provider-with-attached-function-rejected lambda.e2e-capacity-provider-scaling-flow lambda.get-function-scaling-config-default-when-unset lambda.list-capacity-providers-stable-ordering lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected lambda.list-function-versions-by-capacity-provider-nonexistent-provider-rejected lambda.put-function-scaling-config-invalid-values-rejected lambda.put-function-scaling-config-then-capacity-provider-list-reflects lambda.scaling-config-lifecycle lambda.scaling-config-roundtrip lambda.update-capacity-provider-applies-and-persists lambda.update-capacity-provider-concurrent-conflicting-writes lambda.update-capacity-provider-conflict-during-delete-rejected lambda.update-capacity-provider-invalid-params-rejected lambda.update-capacity-provider-nonexistent-rejected lambda.update-capacity-provider-without-permission-denied
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateCapacityProvider|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement lambda CreateCapacityProvider: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
`CreateCapacityProvider` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_capacity_provider.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

23 recorded unit(s) fail on their `CreateCapacityProvider` call, so each one unblocks only when `CreateCapacityProvider` works: `lambda.capacity-provider-lifecycle`, `lambda.capacity-provider-limit-enforcement`, `lambda.capacity-provider-list-and-versions`, `lambda.capacity-provider-scaling-update-triggers-version-rescale`, `lambda.capacity-provider-scaling-value-precision-round-trip`, `lambda.capacity-provider-unicode-name-round-trip`, `lambda.capacity-provider-version-list-ordering`, `lambda.delete-capacity-provider-with-attached-function-rejected`, `lambda.e2e-capacity-provider-scaling-flow`, `lambda.get-function-scaling-config-default-when-unset`, `lambda.list-capacity-providers-stable-ordering`, `lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected`, `lambda.list-function-versions-by-capacity-provider-nonexistent-provider-rejected`, `lambda.put-function-scaling-config-invalid-values-rejected`, `lambda.put-function-scaling-config-then-capacity-provider-list-reflects`, `lambda.scaling-config-lifecycle`, `lambda.scaling-config-roundtrip`, `lambda.update-capacity-provider-applies-and-persists`, `lambda.update-capacity-provider-concurrent-conflicting-writes`, `lambda.update-capacity-provider-conflict-during-delete-rejected` (+3 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_capacity_provider.rs`. Replaying the corpus, the emulator refused the `CreateCapacityProvider` call: File "check_lambda_capacity_provider_lifecycle.py", line 20, in run
    name, cp_arn = _create_capacity_provider(ctx)
                   ~~~~~~~~~~~~~~~~~~~~~~~~~^^^^^
  File "check_lambda_capacity_provider_lifecycle.py", line 205, in _create_capacity_provider
    raise AssertionError(f"Lambda capacity provider prerequisites unavailable: {e}") from e
AssertionError: Lambda capacity provider prerequisites unavailable: An error occurred (NotImplemented) when calling the CreateCapacityProvider operation: operation CreateCapacityProvider is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lambda emulator
Call CreateCapacityProvider and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

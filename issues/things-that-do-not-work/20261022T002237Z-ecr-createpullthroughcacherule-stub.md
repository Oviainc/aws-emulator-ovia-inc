Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.concurrent-pull-through-cache-rule-update-race ecr.create-pull-through-cache-rule-duplicate-rejected ecr.e2e-pull-through-cache-duplicate-rule-rejected ecr.e2e-pull-through-cache-full-flow ecr.e2e-pull-through-cache-rule-consumption-flow ecr.e2e-pull-through-cache-validate-update-cycle ecr.e2e-repository-full-lifecycle-scan-and-replicate ecr.op-contract-deletePullThroughCacheRule ecr.pull-through-cache-duplicate-rejected ecr.pull-through-cache-rule-create-defaults-and-describe-pagination ecr.pull-through-cache-rule-custom-role-arn-persists ecr.pull-through-cache-rule-lifecycle ecr.pull-through-cache-rule-update-lifecycle ecr.pull-through-cache-rule-update-persists-custom-role ecr.pull-through-cache-rule-update-round-trip ecr.update-pull-through-cache-rule-invalid-upstream-rejected ecr.update-pull-through-cache-rule-missing-secret-rejected ecr.update-pull-through-cache-rule-nonexistent-rejected ecr.update-pull-through-cache-rule-secret-not-found-rejected ecr.validate-pull-through-cache-rule-confirms-configured-rule ecr.validate-pull-through-cache-rule-nonexistent-rejected ecr.validate-pull-through-cache-rule-secret-not-found-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|CreatePullThroughCacheRule|NotImplemented
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement ecr CreatePullThroughCacheRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePullThroughCacheRule

## Description
`CreatePullThroughCacheRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/create_pull_through_cache_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

22 recorded unit(s) fail on their `CreatePullThroughCacheRule` call, so each one unblocks only when `CreatePullThroughCacheRule` works: `ecr.concurrent-pull-through-cache-rule-update-race`, `ecr.create-pull-through-cache-rule-duplicate-rejected`, `ecr.e2e-pull-through-cache-duplicate-rule-rejected`, `ecr.e2e-pull-through-cache-full-flow`, `ecr.e2e-pull-through-cache-rule-consumption-flow`, `ecr.e2e-pull-through-cache-validate-update-cycle`, `ecr.e2e-repository-full-lifecycle-scan-and-replicate`, `ecr.op-contract-deletePullThroughCacheRule`, `ecr.pull-through-cache-duplicate-rejected`, `ecr.pull-through-cache-rule-create-defaults-and-describe-pagination`, `ecr.pull-through-cache-rule-custom-role-arn-persists`, `ecr.pull-through-cache-rule-lifecycle`, `ecr.pull-through-cache-rule-update-lifecycle`, `ecr.pull-through-cache-rule-update-persists-custom-role`, `ecr.pull-through-cache-rule-update-round-trip`, `ecr.update-pull-through-cache-rule-invalid-upstream-rejected`, `ecr.update-pull-through-cache-rule-missing-secret-rejected`, `ecr.update-pull-through-cache-rule-nonexistent-rejected`, `ecr.update-pull-through-cache-rule-secret-not-found-rejected`, `ecr.validate-pull-through-cache-rule-confirms-configured-rule` (+2 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/create_pull_through_cache_rule.rs`. Replaying the corpus, the emulator refused the `CreatePullThroughCacheRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePullThroughCacheRule operation: operation CreatePullThroughCacheRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr emulator
Call CreatePullThroughCacheRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

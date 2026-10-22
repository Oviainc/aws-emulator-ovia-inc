Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.list-pull-time-exclusions-pagination-stable-order ecr.list-pull-time-update-exclusions-pagination-order ecr.pull-time-exclusion-window-expires-then-pulls-resume-updates ecr.pull-time-update-exclusion-lifecycle ecr.pull-time-update-exclusion-window-expires ecr.register-pull-time-update-exclusion-duplicate-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|RegisterPullTimeUpdateExclusion|NotImplemented
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement ecr RegisterPullTimeUpdateExclusion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterPullTimeUpdateExclusion

## Description
`RegisterPullTimeUpdateExclusion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/register_pull_time_update_exclusion.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `RegisterPullTimeUpdateExclusion` call, so each one unblocks only when `RegisterPullTimeUpdateExclusion` works: `ecr.list-pull-time-exclusions-pagination-stable-order`, `ecr.list-pull-time-update-exclusions-pagination-order`, `ecr.pull-time-exclusion-window-expires-then-pulls-resume-updates`, `ecr.pull-time-update-exclusion-lifecycle`, `ecr.pull-time-update-exclusion-window-expires`, `ecr.register-pull-time-update-exclusion-duplicate-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/register_pull_time_update_exclusion.rs`. Replaying the corpus, the emulator refused the `RegisterPullTimeUpdateExclusion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterPullTimeUpdateExclusion operation: operation RegisterPullTimeUpdateExclusion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr emulator
Call RegisterPullTimeUpdateExclusion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

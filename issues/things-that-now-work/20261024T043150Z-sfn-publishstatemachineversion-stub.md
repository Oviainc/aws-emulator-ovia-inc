Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.alias-create-describe-update-delete-lifecycle stepfunctions.alias-list-ordering-consistent stepfunctions.alias-routing-config-invalid-version-rejected stepfunctions.alias-routing-config-multiple-versions stepfunctions.e2e-alias-weighted-routing-traffic-split-observed stepfunctions.list-aliases-pagination stepfunctions.publish-version-service-quota-exceeded stepfunctions.sm-alias-update-routing-weights stepfunctions.sm-version-publish-list-delete stepfunctions.state-machine-version-list-ordered-newest-first stepfunctions.update-alias-concurrent-modification-converges stepfunctions.update-state-machine-alias-shifts-routing
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: sfn|PublishStateMachineVersion|NotImplemented
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# Implement sfn PublishStateMachineVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishStateMachineVersion

## Description
`PublishStateMachineVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/publish_state_machine_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `PublishStateMachineVersion` call, so each one unblocks only when `PublishStateMachineVersion` works: `sfn.alias-create-describe-update-delete-lifecycle`, `sfn.alias-list-ordering-consistent`, `sfn.alias-routing-config-invalid-version-rejected`, `sfn.alias-routing-config-multiple-versions`, `sfn.e2e-alias-weighted-routing-traffic-split-observed`, `sfn.list-aliases-pagination`, `sfn.publish-version-service-quota-exceeded`, `sfn.sm-alias-update-routing-weights`, `sfn.sm-version-publish-list-delete`, `sfn.state-machine-version-list-ordered-newest-first`, `sfn.update-alias-concurrent-modification-converges`, `sfn.update-state-machine-alias-shifts-routing`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/publish_state_machine_version.rs`. Replaying the corpus, the emulator refused the `PublishStateMachineVersion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PublishStateMachineVersion operation: operation PublishStateMachineVersion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sfn emulator
Call PublishStateMachineVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

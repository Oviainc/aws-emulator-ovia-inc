Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.experiment-definition-full-lifecycle appconfig.experiment-definition-lifecycle-create-active-update-delete
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|UpdateExperimentDefinition|NotImplemented
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# Implement appconfig UpdateExperimentDefinition: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateExperimentDefinition

## Description
`UpdateExperimentDefinition` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_experiment_definition.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateExperimentDefinition` call, so each one unblocks only when `UpdateExperimentDefinition` works: `appconfig.experiment-definition-full-lifecycle`, `appconfig.experiment-definition-lifecycle-create-active-update-delete`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_experiment_definition.rs`. Replaying the corpus, the emulator refused the `UpdateExperimentDefinition` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateExperimentDefinition operation: operation UpdateExperimentDefinition is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the appconfig emulator
Call UpdateExperimentDefinition and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

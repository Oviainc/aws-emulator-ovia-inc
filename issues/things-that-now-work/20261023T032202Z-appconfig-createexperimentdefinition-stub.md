Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.concurrent-experiment-definition-update-optimistic-lock appconfig.create-experiment-definition-duplicate-name-conflict-rejected appconfig.experiment-definition-crud-lifecycle appconfig.experiment-definition-delete-then-get-404 appconfig.experiment-definition-full-lifecycle appconfig.experiment-definition-get-after-delete-not-found appconfig.experiment-definition-lifecycle-create-active-update-delete appconfig.experiment-definition-treatment-config-round-trip appconfig.experiment-definition-unicode-name-description-round-trip appconfig.get-experiment-definition-not-found appconfig.list-experiment-definitions-reflects-lifecycle appconfig.list-experiment-definitions-scoped-to-application appconfig.update-experiment-definition-after-delete-rejected appconfig.update-experiment-definition-invalid-schedule-rejected appconfig.update-experiment-definition-invalid-treatment-rejected
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|CreateExperimentDefinition|NotImplemented
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# Implement appconfig CreateExperimentDefinition: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExperimentDefinition

## Description
`CreateExperimentDefinition` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/create_experiment_definition.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

15 recorded unit(s) fail on their `CreateExperimentDefinition` call, so each one unblocks only when `CreateExperimentDefinition` works: `appconfig.concurrent-experiment-definition-update-optimistic-lock`, `appconfig.create-experiment-definition-duplicate-name-conflict-rejected`, `appconfig.experiment-definition-crud-lifecycle`, `appconfig.experiment-definition-delete-then-get-404`, `appconfig.experiment-definition-full-lifecycle`, `appconfig.experiment-definition-get-after-delete-not-found`, `appconfig.experiment-definition-lifecycle-create-active-update-delete`, `appconfig.experiment-definition-treatment-config-round-trip`, `appconfig.experiment-definition-unicode-name-description-round-trip`, `appconfig.get-experiment-definition-not-found`, `appconfig.list-experiment-definitions-reflects-lifecycle`, `appconfig.list-experiment-definitions-scoped-to-application`, `appconfig.update-experiment-definition-after-delete-rejected`, `appconfig.update-experiment-definition-invalid-schedule-rejected`, `appconfig.update-experiment-definition-invalid-treatment-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/create_experiment_definition.rs`. Replaying the corpus, the emulator refused the `CreateExperimentDefinition` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateExperimentDefinition operation: operation CreateExperimentDefinition is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the appconfig emulator
Call CreateExperimentDefinition and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

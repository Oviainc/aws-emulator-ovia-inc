Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.concurrent-update-experiment-definition-optimistic-lock appconfig.delete-experiment-definition-with-active-run-conflict-rejected appconfig.e2e-experiment-lifecycle-full-flow appconfig.experiment-run-event-payload-round-trip appconfig.experiment-run-events-chronological-ordering appconfig.experiment-run-managed-computation-not-client-controlled appconfig.experiment-run-scheduled-end-time-auto-stop appconfig.experiment-run-start-to-complete-lifecycle appconfig.experiment-run-treatment-weights-numeric-precision-preserved appconfig.get-experiment-run-invalid-id-not-found appconfig.list-experiment-runs-ordered-by-start-time appconfig.update-experiment-definition-while-running-rejected appconfig.update-experiment-run-invalid-state-transition-rejected
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|StartExperimentRun|NotImplemented
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# Implement appconfig StartExperimentRun: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExperimentRun

## Description
`StartExperimentRun` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/start_experiment_run.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `StartExperimentRun` call, so each one unblocks only when `StartExperimentRun` works: `appconfig.concurrent-update-experiment-definition-optimistic-lock`, `appconfig.delete-experiment-definition-with-active-run-conflict-rejected`, `appconfig.e2e-experiment-lifecycle-full-flow`, `appconfig.experiment-run-event-payload-round-trip`, `appconfig.experiment-run-events-chronological-ordering`, `appconfig.experiment-run-managed-computation-not-client-controlled`, `appconfig.experiment-run-scheduled-end-time-auto-stop`, `appconfig.experiment-run-start-to-complete-lifecycle`, `appconfig.experiment-run-treatment-weights-numeric-precision-preserved`, `appconfig.get-experiment-run-invalid-id-not-found`, `appconfig.list-experiment-runs-ordered-by-start-time`, `appconfig.update-experiment-definition-while-running-rejected`, `appconfig.update-experiment-run-invalid-state-transition-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/start_experiment_run.rs`. Replaying the corpus, the emulator refused the `StartExperimentRun` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartExperimentRun operation: operation StartExperimentRun is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the appconfig emulator
Call StartExperimentRun and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

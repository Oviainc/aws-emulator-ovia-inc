Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.e2e-experiment-trial-component-association-tracks-run sagemaker.e2e-trial-component-association-lineage-flow
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|ListTrialComponents|NotImplemented
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Implement sagemaker ListTrialComponents: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTrialComponents

## Description
`ListTrialComponents` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/list_trial_components.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListTrialComponents` call, so each one unblocks only when `ListTrialComponents` works: `sagemaker.e2e-experiment-trial-component-association-tracks-run`, `sagemaker.e2e-trial-component-association-lineage-flow`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/list_trial_components.rs`. Replaying the corpus, the emulator refused the `ListTrialComponents` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTrialComponents operation: operation ListTrialComponents is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sagemaker emulator
Call ListTrialComponents and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

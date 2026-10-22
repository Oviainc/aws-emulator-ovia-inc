Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.add-tags-and-list-tags sagemaker.add-tags-overwrites-existing-key sagemaker.associate-trial-component-concurrent-limit-enforcement sagemaker.create-experiment-duplicate-name-fails sagemaker.create-experiment-malformed-name-fails sagemaker.delete-tags-removes-key sagemaker.describe-experiment-not-found sagemaker.e2e-experiment-trial-component-association-tracks-run sagemaker.e2e-experiment-trial-training-job-metrics-flow sagemaker.e2e-trial-component-association-lineage-flow sagemaker.e2e-trial-component-association-nonexistent-trial-rejected sagemaker.experiment-create-describe-delete sagemaker.experiment-trial-component-association-flow sagemaker.get-search-suggestions-matches-created-resource sagemaker.get-search-suggestions-matches-resource-schema sagemaker.get-search-suggestions-reflects-created-resource sagemaker.get-search-suggestions-returns-results-for-existing-resource sagemaker.search-and-in-filter-semantics sagemaker.search-sort-order-returns-sequence-not-set sagemaker.search-suggestions-reflects-created-resource-properties sagemaker.search-suggestions-return-relevant-property-names sagemaker.search-unknown-filter-name-rejected sagemaker.trial-and-trial-component-association-lifecycle sagemaker.trial-component-associate-disassociate-lifecycle sagemaker.trial-component-association-lifecycle sagemaker.trial-create-describe-delete sagemaker.trial-create-describe-delete-lifecycle sagemaker.trial-creation-timestamp-precision-preserved sagemaker.trial-delete-with-attached-component-rejected sagemaker.trial-lifecycle-create-describe-delete
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateExperiment|NotImplemented
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker CreateExperiment: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExperiment

## Description
`CreateExperiment` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_experiment.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

30 recorded unit(s) fail on their `CreateExperiment` call, so each one unblocks only when `CreateExperiment` works: `sagemaker.add-tags-and-list-tags`, `sagemaker.add-tags-overwrites-existing-key`, `sagemaker.associate-trial-component-concurrent-limit-enforcement`, `sagemaker.create-experiment-duplicate-name-fails`, `sagemaker.create-experiment-malformed-name-fails`, `sagemaker.delete-tags-removes-key`, `sagemaker.describe-experiment-not-found`, `sagemaker.e2e-experiment-trial-component-association-tracks-run`, `sagemaker.e2e-experiment-trial-training-job-metrics-flow`, `sagemaker.e2e-trial-component-association-lineage-flow`, `sagemaker.e2e-trial-component-association-nonexistent-trial-rejected`, `sagemaker.experiment-create-describe-delete`, `sagemaker.experiment-trial-component-association-flow`, `sagemaker.get-search-suggestions-matches-created-resource`, `sagemaker.get-search-suggestions-matches-resource-schema`, `sagemaker.get-search-suggestions-reflects-created-resource`, `sagemaker.get-search-suggestions-returns-results-for-existing-resource`, `sagemaker.search-and-in-filter-semantics`, `sagemaker.search-sort-order-returns-sequence-not-set`, `sagemaker.search-suggestions-reflects-created-resource-properties` (+10 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_experiment.rs`. Replaying the corpus, the emulator refused the `CreateExperiment` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateExperiment operation: operation CreateExperiment is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sagemaker emulator
Call CreateExperiment and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

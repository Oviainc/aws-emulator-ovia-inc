Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.concurrent-quota-share-updates-optimistic-lock batch.concurrent-service-job-terminate-and-update-race-consistent batch.create-quota-share-duplicate-name-rejected batch.create-quota-share-invalid-share-identifier-fails batch.create-quota-share-malformed-share-identifier-rejected batch.create-quota-share-negative-weight-value-rejected batch.delete-quota-share-referenced-by-active-scheduling-policy-fails batch.delete-quota-share-while-enabled-rejected batch.delete-service-environment-lifecycle batch.describe-unknown-quota-share-fails batch.e2e-service-job-lifecycle-through-queue-snapshot batch.e2e-service-job-quota-share-consumption-flow batch.e2e-service-job-submit-monitor-terminate-cleanup batch.job-queue-snapshot-includes-service-job-entries batch.job-queue-snapshot-reflects-quota-share-usage batch.jq-create-with-unknown-scheduling-policy-fails batch.jq-service-queue-rejects-compute-environment-order batch.jq-snapshot-and-list-jobs-status-filter batch.list-quota-shares-reflects-created batch.list-service-jobs-filters-by-queue-and-status batch.op-contract-createServiceEnvironment batch.op-contract-deleteServiceEnvironment batch.op-contract-describeServiceEnvironments batch.op-contract-update-service-environment batch.op-contract-updateserviceenvironment batch.quota-share-delete-then-describe-fails batch.quota-share-delete-while-enabled-rejected batch.quota-share-lifecycle-create-active-delete batch.quota-share-lifecycle-crud batch.quota-share-unicode-name-rejected batch.quota-share-unicode-name-round-trip batch.quota-share-update-active-share-modifies-quota-value batch.service-environment-delete-while-attached-to-queue-fails batch.service-environment-delete-while-enabled-fails batch.service-environment-lifecycle batch.service-job-lifecycle-submit-to-terminal batch.service-job-submit-to-disabled-queue-fails batch.service-job-submit-to-unknown-queue-fails batch.service-job-terminate-terminal-is-idempotent batch.service-job-terminate-transitions-to-terminal batch.service-job-update-persists-across-describe batch.service-jobs-listing-empty-for-fresh-queue batch.submit-service-job-missing-quota-share-name-rejected batch.submit-service-job-unknown-service-environment-fails batch.terminate-service-job-without-permission-denied batch.update-service-job-in-terminal-state-rejected batch.update-service-job-modifies-attributes-and-persists batch.update-service-job-terminal-state-blocked batch.update-service-job-terminal-state-rejected
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|CreateServiceEnvironment|NotImplemented
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Implement batch CreateServiceEnvironment: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceEnvironment

## Description
`CreateServiceEnvironment` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/create_service_environment.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

49 recorded unit(s) fail on their `CreateServiceEnvironment` call, so each one unblocks only when `CreateServiceEnvironment` works: `batch.concurrent-quota-share-updates-optimistic-lock`, `batch.concurrent-service-job-terminate-and-update-race-consistent`, `batch.create-quota-share-duplicate-name-rejected`, `batch.create-quota-share-invalid-share-identifier-fails`, `batch.create-quota-share-malformed-share-identifier-rejected`, `batch.create-quota-share-negative-weight-value-rejected`, `batch.delete-quota-share-referenced-by-active-scheduling-policy-fails`, `batch.delete-quota-share-while-enabled-rejected`, `batch.delete-service-environment-lifecycle`, `batch.describe-unknown-quota-share-fails`, `batch.e2e-service-job-lifecycle-through-queue-snapshot`, `batch.e2e-service-job-quota-share-consumption-flow`, `batch.e2e-service-job-submit-monitor-terminate-cleanup`, `batch.job-queue-snapshot-includes-service-job-entries`, `batch.job-queue-snapshot-reflects-quota-share-usage`, `batch.jq-create-with-unknown-scheduling-policy-fails`, `batch.jq-service-queue-rejects-compute-environment-order`, `batch.jq-snapshot-and-list-jobs-status-filter`, `batch.list-quota-shares-reflects-created`, `batch.list-service-jobs-filters-by-queue-and-status` (+29 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/create_service_environment.rs`. Replaying the corpus, the emulator refused the `CreateServiceEnvironment` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateServiceEnvironment operation: operation CreateServiceEnvironment is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the batch emulator
Call CreateServiceEnvironment and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

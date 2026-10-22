Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.concurrent-update-sampling-rule-conflicting-writes xray.create-sampling-rule-attributes-exceed-max-rejected xray.create-sampling-rule-defaults xray.create-sampling-rule-duplicate-name-fails xray.create-sampling-rule-invalid-fixed-rate-fails xray.delete-sampling-rule-lifecycle xray.e2e-sampling-rule-affects-target-then-cleanup xray.e2e-sampling-rule-drives-target-allocation xray.e2e-sampling-rule-lifecycle-affects-sampling-targets xray.get-sampling-rules-returns-priority-ordered xray.get-sampling-statistic-summaries-numeric-precision-preserved xray.get-sampling-statistic-summaries-returns-list xray.get-sampling-targets-fixed-rate-and-unknown-rule xray.get-sampling-targets-for-created-rule xray.get-sampling-targets-partial-failure xray.get-sampling-targets-reservoir-ttl-honored xray.sampling-reservoir-quota-allocated-from-reported-statistics xray.sampling-rule-fixed-rate-precision-preserved xray.sampling-rule-numeric-precision-preserved xray.sampling-rules-priority-ordering-affects-targets xray.sampling-targets-ttl-refresh-interval xray.update-sampling-rule-fields xray.update-sampling-rule-negative-priority-rejected xray.update-sampling-rule-nonexistent-name-rejected xray.update-sampling-rule-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|CreateSamplingRule|NotImplemented
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Implement xray CreateSamplingRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSamplingRule

## Description
`CreateSamplingRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-xray-awsxray/src/ops/create_sampling_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

25 recorded unit(s) fail on their `CreateSamplingRule` call, so each one unblocks only when `CreateSamplingRule` works: `xray.concurrent-update-sampling-rule-conflicting-writes`, `xray.create-sampling-rule-attributes-exceed-max-rejected`, `xray.create-sampling-rule-defaults`, `xray.create-sampling-rule-duplicate-name-fails`, `xray.create-sampling-rule-invalid-fixed-rate-fails`, `xray.delete-sampling-rule-lifecycle`, `xray.e2e-sampling-rule-affects-target-then-cleanup`, `xray.e2e-sampling-rule-drives-target-allocation`, `xray.e2e-sampling-rule-lifecycle-affects-sampling-targets`, `xray.get-sampling-rules-returns-priority-ordered`, `xray.get-sampling-statistic-summaries-numeric-precision-preserved`, `xray.get-sampling-statistic-summaries-returns-list`, `xray.get-sampling-targets-fixed-rate-and-unknown-rule`, `xray.get-sampling-targets-for-created-rule`, `xray.get-sampling-targets-partial-failure`, `xray.get-sampling-targets-reservoir-ttl-honored`, `xray.sampling-reservoir-quota-allocated-from-reported-statistics`, `xray.sampling-rule-fixed-rate-precision-preserved`, `xray.sampling-rule-numeric-precision-preserved`, `xray.sampling-rules-priority-ordering-affects-targets` (+5 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-xray-awsxray/src/ops/create_sampling_rule.rs`. Replaying the corpus, the emulator refused the `CreateSamplingRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSamplingRule operation: operation CreateSamplingRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the xray emulator
Call CreateSamplingRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

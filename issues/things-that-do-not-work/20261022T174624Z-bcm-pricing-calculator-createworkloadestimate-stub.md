Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-pricing-calculator.batch-create-workload-estimate-usage-exceeds-max-entries-rejected bcm-pricing-calculator.bill-scenario-create-get-delete-lifecycle bcm-pricing-calculator.concurrent-create-bill-scenario-same-client-token-dedupes bcm-pricing-calculator.get-workload-estimate-after-delete-not-found-rejected bcm-pricing-calculator.list-workload-estimates-pagination-includes-all-created bcm-pricing-calculator.tag-resource-and-list-tags-roundtrip bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected bcm-pricing-calculator.untag-resource-removes-tag bcm-pricing-calculator.workload-estimate-create-get-delete-lifecycle bcm-pricing-calculator.workload-estimate-usage-add-reaches-terminal-state bcm-pricing-calculator.workload-estimate-usage-delete-removes-item bcm-pricing-calculator.workload-estimate-usage-update-changes-quantity
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|NotImplemented
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Implement bcm-pricing-calculator CreateWorkloadEstimate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkloadEstimate

## Description
`CreateWorkloadEstimate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/create_workload_estimate.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `CreateWorkloadEstimate` call, so each one unblocks only when `CreateWorkloadEstimate` works: `bcm-pricing-calculator.batch-create-workload-estimate-usage-exceeds-max-entries-rejected`, `bcm-pricing-calculator.bill-scenario-create-get-delete-lifecycle`, `bcm-pricing-calculator.concurrent-create-bill-scenario-same-client-token-dedupes`, `bcm-pricing-calculator.get-workload-estimate-after-delete-not-found-rejected`, `bcm-pricing-calculator.list-workload-estimates-pagination-includes-all-created`, `bcm-pricing-calculator.tag-resource-and-list-tags-roundtrip`, `bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected`, `bcm-pricing-calculator.untag-resource-removes-tag`, `bcm-pricing-calculator.workload-estimate-create-get-delete-lifecycle`, `bcm-pricing-calculator.workload-estimate-usage-add-reaches-terminal-state`, `bcm-pricing-calculator.workload-estimate-usage-delete-removes-item`, `bcm-pricing-calculator.workload-estimate-usage-update-changes-quantity`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/create_workload_estimate.rs`. Replaying the corpus, the emulator refused the `CreateWorkloadEstimate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateWorkloadEstimate operation: operation CreateWorkloadEstimate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bcm-pricing-calculator emulator
Call CreateWorkloadEstimate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: savingsplans.describe-savings-plans-offering-rates-basic-fields savingsplans.describe-savings-plans-offering-rates-filtered-by-product-type savingsplans.describe-savings-plans-offering-rates-pagination-distinct-pages
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans|DescribeSavingsPlansOfferingRates|NotImplemented
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# Implement savingsplans DescribeSavingsPlansOfferingRates: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSavingsPlansOfferingRates

## Description
`DescribeSavingsPlansOfferingRates` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/describe_savings_plans_offering_rates.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DescribeSavingsPlansOfferingRates` call, so each one unblocks only when `DescribeSavingsPlansOfferingRates` works: `savingsplans.describe-savings-plans-offering-rates-basic-fields`, `savingsplans.describe-savings-plans-offering-rates-filtered-by-product-type`, `savingsplans.describe-savings-plans-offering-rates-pagination-distinct-pages`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/describe_savings_plans_offering_rates.rs`. Replaying the corpus, the emulator refused the `DescribeSavingsPlansOfferingRates` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeSavingsPlansOfferingRates operation: operation DescribeSavingsPlansOfferingRates is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the savingsplans emulator
Call DescribeSavingsPlansOfferingRates and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

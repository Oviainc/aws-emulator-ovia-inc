Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-agreement.list-billing-adjustment-requests-status-filter-structure
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|ListBillingAdjustmentRequests|NotImplemented
Issue-Commit: 7545d0b8b2e4fb4a41c801fcbb48b523234ffec8

# Implement marketplace-agreement ListBillingAdjustmentRequests: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7545d0b8b2e4fb4a41c801fcbb48b523234ffec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBillingAdjustmentRequests

## Description
`ListBillingAdjustmentRequests` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/list_billing_adjustment_requests.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListBillingAdjustmentRequests` call, so each one unblocks only when `ListBillingAdjustmentRequests` works: `marketplace-agreement.list-billing-adjustment-requests-status-filter-structure`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/list_billing_adjustment_requests.rs`. Replaying the corpus, the emulator refused the `ListBillingAdjustmentRequests` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListBillingAdjustmentRequests operation: operation ListBillingAdjustmentRequests is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the marketplace-agreement emulator
Call ListBillingAdjustmentRequests and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

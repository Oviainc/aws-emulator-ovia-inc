Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-data-exports.list-executions-max-results-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports|ListExecutions|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement bcm-data-exports ListExecutions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListExecutions

## Description
`ListExecutions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/ops/list_executions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListExecutions` call, so each one unblocks only when `ListExecutions` works: `bcm-data-exports.list-executions-max-results-out-of-range-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/ops/list_executions.rs`. Replaying the corpus, the emulator refused the `ListExecutions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListExecutions operation: operation ListExecutions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bcm-data-exports emulator
Call ListExecutions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

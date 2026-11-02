Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicecatalog.launch-constraint-role-governs-plan-execution servicecatalog.provision-invalid-template-parameter-fails-asynchronously
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: service-catalog|DescribeProvisioningParameters|NotImplemented
Issue-Commit: d5912a3a34a2e2faa23a606635408f9fea83c969

# Implement service-catalog DescribeProvisioningParameters: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d5912a3a34a2e2faa23a606635408f9fea83c969`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProvisioningParameters

## Description
`DescribeProvisioningParameters` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/ops/describe_provisioning_parameters.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DescribeProvisioningParameters` call, so each one unblocks only when `DescribeProvisioningParameters` works: `service-catalog.launch-constraint-role-governs-plan-execution`, `service-catalog.provision-invalid-template-parameter-fails-asynchronously`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/ops/describe_provisioning_parameters.rs`. Replaying the corpus, the emulator refused the `DescribeProvisioningParameters` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeProvisioningParameters operation: operation DescribeProvisioningParameters is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the service-catalog emulator
Call DescribeProvisioningParameters and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
